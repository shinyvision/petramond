//! The in-process server thread.
//!
//! [`spawn`] moves a fully-constructed [`ServerGame`] onto its OWN thread
//! ("petramond-server", NORMAL priority — it IS the sim, not a background
//! worker), self-clocked at 20 TPS, and returns the transport-agnostic
//! [`ServerHandle`] the client holds (`crate::net::handle`). The client talks
//! to it exclusively over channels of protocol MESSAGE VALUES — no
//! serialization; `Arc` payloads are refcount bumps (the same messages a
//! remote connection ships over TCP).
//!
//! Lifecycle:
//! - [`ControlMsg::Shutdown`] (sent by [`ServerHandle::shutdown_and_join`])
//!   makes the thread save everything and exit; the join returns after the
//!   save queued (the world's save thread flushes on drop).
//! - A PANIC anywhere in the loop drops the world WITHOUT saving — mid-tick
//!   state may be inconsistent, and persisting it risks a corrupt save;
//!   autosave bounds the loss to ~30 s (the `GenOutput::*Failed` fail-loud
//!   philosophy). The crash is surfaced through [`ServerHandle::is_crashed`].
//! - The client vanishing (its channel endpoints dropped without a Shutdown)
//!   is treated as Shutdown-with-save.

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::Ordering;
use std::sync::mpsc::{Receiver, Sender, TryRecvError};
use std::time::{Duration, Instant};

use crate::events::tick::TICK_DT;
use crate::net::handle::{ControlMsg, ServerEnd, ServerHandle};
use crate::net::protocol::{ClientToServer, ServerToClient};
use crate::player::PlayerId;

use super::game::ServerGame;
use super::remote::RemoteHub;

/// Move `server` onto its own self-clocked thread and return the handle.
/// The `ServerGame` is constructed on the caller's thread (mods initialized)
/// and handed over whole.
pub fn spawn(server: ServerGame) -> ServerHandle {
    ServerHandle::spawn_with(move |end: ServerEnd, crash_flag| {
        std::thread::Builder::new()
            .name("petramond-server".to_string())
            .spawn(move || {
                // The whole loop under one catch_unwind: a panic sets the
                // crash flag and drops the world WITHOUT saving (see the
                // module docs), then the thread exits.
                let result = catch_unwind(AssertUnwindSafe(move || {
                    server_main(server, end.inbox, end.outbox, end.control)
                }));
                if result.is_err() {
                    crash_flag.store(true, Ordering::SeqCst);
                    log::error!(
                        "server thread panicked; world dropped WITHOUT saving \
                         (mid-tick state may be corrupt; autosave bounds the loss)"
                    );
                }
            })
            .expect("spawn server thread")
    })
}

/// Max sleep between loop iterations: bounds the latency of message drains
/// and streaming installs between tick edges (a frame is ~16 ms; 5 ms keeps
/// input latching well under one frame).
// 2 ms (was 5): each stage boundary of the streaming ladder (gen result →
// section jobs → light bake → light drain → ship) waits for the next pump,
// so the interval multiplies directly into world-join latency. The extra
// idle wakeups (~500/s) cost nothing measurable.
const POLL_INTERVAL: Duration = Duration::from_millis(2);

/// The server thread's main loop: self-clocked 20 TPS over the wall clock.
/// Every iteration drains control + gameplay messages (local pipe + any
/// remote connections through the [`RemoteHub`]: accepts, pre-join
/// handshakes, leaves) and runs one `ServerGame::pump_tagged` (fixed ticks
/// when due — `MAX_TICKS_PER_FRAME` bounds catch-up; streaming/autosave every
/// iteration), then routes each recipient's messages to its connection.
/// Returning farewells the remotes and saves everything; a panic propagates
/// to the `catch_unwind` in [`spawn`] (no save).
fn server_main(
    mut server: ServerGame,
    inbox: Receiver<ClientToServer>,
    outbox: Sender<ServerToClient>,
    control: Receiver<ControlMsg>,
) {
    // The scripted AI-node dispatch registry is thread-local (test isolation);
    // mods were initialized on the constructing thread, so install here too.
    server.mods.install_thread_ai_nodes();

    let mut hub = RemoteHub::default();
    let mut msgs: Vec<(PlayerId, ClientToServer)> = Vec::new();
    let mut last = Instant::now();
    #[cfg(any(test, feature = "test-support"))]
    let mut unthrottled = false;
    loop {
        loop {
            match control.try_recv() {
                Ok(ControlMsg::Shutdown) => {
                    hub.shutdown();
                    server.close_sessions_and_save();
                    return;
                }
                Ok(ControlMsg::SaveAll) => server.save_all(),
                Ok(ControlMsg::Command(text)) => server.execute_console_command(&text),
                Ok(ControlMsg::OpenToLan { port, reply }) => {
                    let result = hub.open_to_lan(port);
                    if result.is_ok() {
                        // The pause gate becomes real and PERMANENT: remote
                        // players may exist (or reappear) from here on.
                        server.lan_ever_opened = true;
                        server.paused = false;
                    }
                    let _ = reply.send(result);
                }
                #[cfg(any(test, feature = "test-support"))]
                Ok(ControlMsg::PanicForTest) => panic!("server loop panic injected by test"),
                #[cfg(any(test, feature = "test-support"))]
                Ok(ControlMsg::UnthrottleForTest) => unthrottled = true,
                Err(TryRecvError::Empty) => break,
                // Control sender dropped = the client handle is gone entirely
                // (no clean Shutdown reached us): save and exit.
                Err(TryRecvError::Disconnected) => {
                    hub.shutdown();
                    server.close_sessions_and_save();
                    return;
                }
            }
        }

        debug_assert!(msgs.is_empty(), "pump drains its inbox");
        // Headless servers have no local session: the handle's gameplay pipe
        // exists but nothing meaningful arrives on it — drain and drop (the
        // Disconnected arm still means "the handle's owner is gone").
        let local_id = server.local_session_id();
        let mut disconnected = false;
        loop {
            match inbox.try_recv() {
                Ok(msg) => {
                    if let Some(id) = local_id {
                        msgs.push((id, msg));
                    }
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    disconnected = true;
                    break;
                }
            }
        }

        // Remote transport: accepts, pre-join handshakes, leaves, and the
        // joined connections' inbound messages (tagged by PlayerId).
        hub.pump(&mut server, &mut msgs, &outbox);

        // The wall clock lives HERE: dt is real elapsed time per iteration;
        // the pump's accumulator turns it into fixed ticks.
        let now = Instant::now();
        let dt = (now - last).as_secs_f32();
        last = now;
        // A test may pin the clock (`UnthrottleForTest`, see the control arm):
        // exactly one fixed tick per iteration, compute-bound. Production
        // builds always take the real clock.
        #[cfg(any(test, feature = "test-support"))]
        let dt = if unthrottled { TICK_DT } else { dt };
        let headroom = hub.send_headroom();
        let out = server.pump_tagged(dt, &mut msgs, &headroom);
        for msg in out.msgs {
            if outbox.send(msg).is_err() {
                disconnected = true;
                break;
            }
        }
        hub.route(out.remote);
        // Local client gone (endpoints dropped): in singleplayer the app's
        // quit path joins us first, so this is a crashed/aborted client —
        // farewell the remotes, save, and exit.
        if disconnected {
            hub.shutdown();
            server.close_sessions_and_save();
            return;
        }

        // Sleep to min(next tick edge, POLL_INTERVAL); sub-millisecond
        // remainders just yield so the tick edge isn't overslept. The pinned
        // test clock never sleeps — its iterations are compute-bound.
        #[cfg(any(test, feature = "test-support"))]
        if unthrottled {
            std::thread::yield_now();
            continue;
        }
        let until_tick = Duration::from_secs_f32((TICK_DT - server.tick_accumulator).max(0.0));
        let sleep = until_tick.min(POLL_INTERVAL);
        if sleep > Duration::from_millis(1) {
            std::thread::sleep(sleep);
        } else {
            std::thread::yield_now();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::protocol::PlayerUpdate;
    use petramond_math::math::Vec3;

    /// A real, fully-built ServerGame (no save attached), as `Game::new`
    /// builds it.
    fn server_game() -> crate::server::game::ServerGame {
        crate::server::session_build::build_server_inline("", 1, 1)
    }

    fn player_update(server: &crate::server::game::ServerGame) -> PlayerUpdate {
        let p = &server.sessions[0].player;
        PlayerUpdate {
            transform: crate::net::protocol::Transform {
                pos: p.pos,
                vel: Vec3::ZERO,
                yaw: 0.0,
                pitch: 0.0,
            },
            on_ground: true,
            sneak: false,
            gameplay: true,
            break_held: false,
            use_held: false,
            target: None,
            hotbar_slot: 0,
            held_rotation: 0,
            wishdir: Vec3::ZERO,
            jump: false,
            sprint: false,
        }
    }

    fn recv_tick(
        handle: &ServerHandle,
        timeout: Duration,
    ) -> Option<Box<crate::net::protocol::TickUpdate>> {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            match handle.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
                Some(ServerToClient::Tick(update)) => return Some(update),
                Some(_) => continue, // terrain / other messages
                None => return None,
            }
        }
        None
    }

    /// Spawn → exchange messages with the self-clocked thread → clean
    /// shutdown joins. The full server-thread lifecycle over the real thread
    /// and channels.
    #[test]
    fn spawned_server_ticks_answers_and_shuts_down_cleanly() {
        let server = server_game();
        let update = player_update(&server);
        let mut handle = spawn(server);
        handle.unthrottle_for_test();

        handle
            .send(ClientToServer::PlayerUpdate(update))
            .expect("live server accepts messages");
        handle.send(ClientToServer::KeepAlive).expect("live server");

        let first = recv_tick(&handle, petramond_util::test_time::TEST_HARD_DEADLINE)
            .expect("a TickUpdate arrives");
        let second = recv_tick(&handle, petramond_util::test_time::TEST_HARD_DEADLINE)
            .expect("ticks keep coming");
        assert!(
            second.tick > first.tick,
            "the self-clocked loop advances the world tick"
        );
        assert!(!handle.is_crashed());

        handle.shutdown_and_join();
        assert!(!handle.is_crashed(), "a clean shutdown is not a crash");
        assert!(
            handle.send(ClientToServer::KeepAlive).is_err(),
            "the channel is closed after shutdown"
        );
    }

    /// Pause stops the fixed ticks (no TickUpdates), keeps the channel alive,
    /// and resume does NOT fast-forward the banked pause time.
    #[test]
    fn pause_stops_ticks_and_resume_does_not_fast_forward() {
        let server = server_game();
        let update = player_update(&server);
        let mut handle = spawn(server);
        handle
            .send(ClientToServer::PlayerUpdate(update))
            .expect("live server");
        let _ = recv_tick(&handle, petramond_util::test_time::TEST_HARD_DEADLINE)
            .expect("running before the pause");

        handle
            .send(ClientToServer::Pause(true))
            .expect("live server");
        // Let the pause land and drain any in-flight updates from before it.
        // Real-time clock (not unthrottled): pause is edge-triggered on the
        // gameplay inbox, and an unthrottled loop can still deliver a tick
        // that was already banked before the flag flips.
        std::thread::sleep(Duration::from_millis(50));
        let mut drained = Vec::new();
        handle.drain(&mut drained);
        let last_tick = drained
            .iter()
            .filter_map(|m| match m {
                ServerToClient::Tick(u) => Some(u.tick),
                _ => None,
            })
            .max();

        // A few tick periods of silence: the sim is frozen…
        assert!(
            recv_tick(&handle, Duration::from_millis(150)).is_none(),
            "no TickUpdates while paused"
        );
        // …but the connection is alive (message drain continues server-side).
        handle.send(ClientToServer::KeepAlive).expect("still alive");
        assert!(!handle.is_crashed());

        handle
            .send(ClientToServer::Pause(false))
            .expect("live server");
        let resumed = recv_tick(&handle, petramond_util::test_time::TEST_HARD_DEADLINE)
            .expect("ticks resume");
        if let Some(last) = last_tick {
            assert!(resumed.tick > last, "the world advances again");
            // Pausing must not bank catch-up ticks (the accumulator is pinned).
            assert!(
                resumed.tick - last <= u64::from(super::super::game::MAX_TICKS_PER_FRAME),
                "resume fast-forwarded: tick jumped {} -> {}",
                last,
                resumed.tick
            );
        }
        handle.shutdown_and_join();
    }

    /// A panicking server loop drops the world WITHOUT saving and surfaces
    /// through `is_crashed`.
    #[test]
    fn panicking_server_crashes_loud_and_saves_nothing() {
        let dir =
            std::env::temp_dir().join(format!("petramond-handle-panic-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);

        let mut server = server_game();
        let opened = crate::save::open_at(dir.clone()).expect("temp save opens");
        server.world.attach_save(opened.save, opened.saved);

        let mut handle = spawn(server);
        handle.panic_for_test();
        handle.join_for_test();

        assert!(handle.is_crashed(), "the crash flag is set");
        assert!(
            handle.send(ClientToServer::KeepAlive).is_err(),
            "the pipe is dead after the crash"
        );
        assert!(
            !dir.join("level.dat").exists(),
            "a crashed server must NOT save (mid-tick state may be corrupt)"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}
