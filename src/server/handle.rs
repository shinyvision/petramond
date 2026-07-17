//! The server thread and its client-side handle.
//!
//! [`ServerHandle::spawn`] moves a fully-constructed [`ServerGame`] onto its
//! OWN thread ("petramond-server", NORMAL priority — it IS the sim, not a
//! background worker), self-clocked at 20 TPS. The client talks to it
//! exclusively over std::sync::mpsc channels of protocol MESSAGE VALUES — no
//! serialization; `Arc` payloads are refcount bumps (the same messages a
//! remote connection ships over TCP).
//!
//! Lifecycle:
//! - [`ControlMsg::Shutdown`] (sent by [`ServerHandle::shutdown_and_join`])
//!   makes the thread save everything (exactly what `Game::save_all` used to
//!   do) and exit; the join returns after the save queued (the world's save
//!   thread flushes on drop).
//! - A PANIC anywhere in the loop drops the world WITHOUT saving — mid-tick
//!   state may be inconsistent, and persisting it risks a corrupt save;
//!   autosave bounds the loss to ~30 s (the `GenOutput::*Failed` fail-loud
//!   philosophy). The crash is surfaced through [`ServerHandle::is_crashed`].
//! - The client vanishing (its channel endpoints dropped without a Shutdown)
//!   is treated as Shutdown-with-save.

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::game::tick::TICK_DT;
use crate::net::connection::TcpClientConn;
use crate::net::protocol::{ClientToServer, ServerToClient};
use crate::server::player::PlayerId;

use super::game::ServerGame;
use super::remote::RemoteHub;

/// Client→server lifecycle requests that are NOT gameplay messages (those ride
/// the `ClientToServer` channel). Internal — never on the wire.
pub(crate) enum ControlMsg {
    /// Save everything and exit the thread.
    Shutdown,
    /// Save everything now (the app's suspend/exit hook); the thread keeps
    /// running.
    SaveAll,
    /// Execute one gameplay-visible dedicated-server console command.
    Command(String),
    /// "Open to LAN": bind a TCP listener into the running server. Port 0 =
    /// ephemeral; the reply carries the actual bound port. A successful open
    /// force-unpauses and makes the pause gate permanent.
    OpenToLan {
        port: u16,
        reply: Sender<std::io::Result<u16>>,
    },
    /// Panic the server loop, for the crash-policy tests.
    #[cfg(test)]
    PanicForTest,
}

/// The client's handle to the server: message senders/receiver, the control
/// channel, and the join handle. The server end is EITHER the in-process
/// server thread (`spawn`) or a remote server's TCP connection threads
/// (`from_remote`) — everything above the handle is agnostic.
pub(crate) struct ServerHandle {
    to_server: Sender<ClientToServer>,
    from_server: Receiver<ServerToClient>,
    control: Sender<ControlMsg>,
    join: Option<JoinHandle<()>>,
    crashed: Arc<AtomicBool>,
    /// The TCP connection when this handle fronts a REMOTE server (its
    /// reader/writer threads feed the channels above); `None` in-process.
    /// `crashed` then means "connection lost".
    remote: Option<TcpClientConn>,
}

impl ServerHandle {
    /// Move `server` onto its own self-clocked thread and return the handle.
    /// The `ServerGame` is constructed exactly as before (on the caller's
    /// thread, mods initialized) and handed over whole.
    pub(crate) fn spawn(server: ServerGame) -> ServerHandle {
        let (to_server, inbox_rx) = mpsc::channel::<ClientToServer>();
        let (outbox_tx, from_server) = mpsc::channel::<ServerToClient>();
        let (control, control_rx) = mpsc::channel::<ControlMsg>();
        let crashed = Arc::new(AtomicBool::new(false));
        let crash_flag = Arc::clone(&crashed);
        let join = std::thread::Builder::new()
            .name("petramond-server".to_string())
            .spawn(move || {
                // The whole loop under one catch_unwind: a panic sets the
                // crash flag and drops the world WITHOUT saving (see the
                // module docs), then the thread exits.
                let result = catch_unwind(AssertUnwindSafe(move || {
                    server_main(server, inbox_rx, outbox_tx, control_rx)
                }));
                if result.is_err() {
                    crash_flag.store(true, Ordering::SeqCst);
                    log::error!(
                        "server thread panicked; world dropped WITHOUT saving \
                         (mid-tick state may be corrupt; autosave bounds the loss)"
                    );
                }
            })
            .expect("spawn server thread");
        ServerHandle {
            to_server,
            from_server,
            control,
            join: Some(join),
            crashed,
            remote: None,
        }
    }

    /// A handle whose server end is a REMOTE server over TCP: the connection's
    /// reader/writer threads present the same channel pair the server thread
    /// does. Built by the connect worker after `client_handshake`
    /// succeeded and `TcpClientConn::spawn` installed the id remap.
    pub(crate) fn from_remote(mut conn: TcpClientConn) -> ServerHandle {
        ServerHandle {
            to_server: conn.sender(),
            from_server: conn.take_receiver(),
            // Dangling on purpose: control requests are for the in-process
            // server thread; every `send` is best-effort (`let _ =`).
            control: mpsc::channel().0,
            join: None,
            crashed: conn.lost_flag(),
            remote: Some(conn),
        }
    }

    /// A handle whose server end is serviced by the CALLER instead of a
    /// thread — the deterministic loopback the game test harness pumps
    /// synchronously (`src/game/tests/common.rs`). Same channels, same
    /// messages, no thread.
    #[cfg(test)]
    pub(crate) fn loopback() -> (ServerHandle, LoopbackServer) {
        let (to_server, inbox_rx) = mpsc::channel::<ClientToServer>();
        let (outbox_tx, from_server) = mpsc::channel::<ServerToClient>();
        let (control, control_rx) = mpsc::channel::<ControlMsg>();
        (
            ServerHandle {
                to_server,
                from_server,
                control,
                join: None,
                crashed: Arc::new(AtomicBool::new(false)),
                remote: None,
            },
            LoopbackServer {
                inbox: inbox_rx,
                outbox: outbox_tx,
                control: control_rx,
            },
        )
    }

    /// Open the running server to LAN on `port` (0 = ephemeral); blocks up to
    /// 5 s for the bind result, whose `Ok` carries the actual port. The E2
    /// pause-menu action calls this.
    pub(crate) fn open_to_lan(&self, port: u16) -> std::io::Result<u16> {
        use std::io::{Error, ErrorKind};
        let (reply, result) = mpsc::channel();
        self.control
            .send(ControlMsg::OpenToLan { port, reply })
            .map_err(|_| Error::new(ErrorKind::BrokenPipe, "the server is gone"))?;
        result
            .recv_timeout(Duration::from_secs(5))
            .map_err(|_| Error::new(ErrorKind::TimedOut, "the server did not answer"))?
    }

    /// Send one gameplay message. `Err` = the server is gone (crashed or shut
    /// down); the caller surfaces it as a lost connection.
    pub(crate) fn send(&self, msg: ClientToServer) -> Result<(), ()> {
        self.to_server.send(msg).map_err(|_| ())
    }

    /// Drain every pending server→client message, in order, into `into`.
    pub(crate) fn drain(&mut self, into: &mut Vec<ServerToClient>) {
        while let Ok(msg) = self.from_server.try_recv() {
            into.push(msg);
        }
    }

    /// Ask the server to save everything now (it keeps running). A REMOTE
    /// server saves autonomously — nothing to ask.
    pub(crate) fn save_all(&self) {
        if self.remote.is_none() {
            let _ = self.control.send(ControlMsg::SaveAll);
        }
    }

    /// Execute one unprefixed command on a local/headless server.
    pub(crate) fn command(&self, text: String) {
        if self.remote.is_none() {
            let _ = self.control.send(ControlMsg::Command(text));
        }
    }

    #[inline]
    pub(crate) fn is_crashed(&self) -> bool {
        self.crashed.load(Ordering::SeqCst)
    }

    /// Send Shutdown and join: the thread saves everything before exiting.
    /// Idempotent — a second call (e.g. the `Drop` safety net) is a no-op.
    /// A REMOTE handle instead closes the connection: dropping the last
    /// message sender makes the writer thread flush a farewell `Disconnect`
    /// before the socket closes; the remote server saves our player on leave.
    pub(crate) fn shutdown_and_join(&mut self) {
        if self.remote.is_some() {
            self.to_server = mpsc::channel().0; // drop our sender clone
            self.remote = None;
            return;
        }
        let _ = self.control.send(ControlMsg::Shutdown);
        if let Some(join) = self.join.take() {
            if join.join().is_err() {
                log::error!("server thread join failed (panicked during shutdown)");
            }
        }
    }

    /// Panic the server loop (crash-policy tests).
    #[cfg(test)]
    pub(crate) fn panic_for_test(&self) {
        let _ = self.control.send(ControlMsg::PanicForTest);
    }

    /// Wait for the server thread to exit WITHOUT requesting a save-shutdown —
    /// for tests that made it exit another way (panic).
    #[cfg(test)]
    pub(crate) fn join_for_test(&mut self) {
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }

    /// Blocking receive with a deadline, for tests awaiting a message.
    #[cfg(test)]
    pub(crate) fn recv_timeout(&self, timeout: Duration) -> Option<ServerToClient> {
        self.from_server.recv_timeout(timeout).ok()
    }
}

impl Drop for ServerHandle {
    /// Safety net: an un-shutdown handle (a dropped `Game` outside the quit
    /// path) still saves and joins, mirroring `WorldSave`'s drop contract.
    fn drop(&mut self) {
        self.shutdown_and_join();
    }
}

/// The server end of a [`ServerHandle::loopback`] pipe: the test harness
/// drains `inbox` into `ServerGame::pump` and forwards the pump's messages
/// through `outbox`, standing in for the thread.
#[cfg(test)]
pub(crate) struct LoopbackServer {
    pub(crate) inbox: Receiver<ClientToServer>,
    pub(crate) outbox: Sender<ServerToClient>,
    /// Kept alive so the handle's `Drop` (which sends `Shutdown`) never
    /// errors; the synchronous harness has no loop to control.
    #[allow(dead_code)]
    pub(crate) control: Receiver<ControlMsg>,
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
/// to the `catch_unwind` in [`ServerHandle::spawn`] (no save).
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
                #[cfg(test)]
                Ok(ControlMsg::PanicForTest) => panic!("server loop panic injected by test"),
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
        // remainders just yield so the tick edge isn't overslept.
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
    use crate::mathh::Vec3;
    use crate::net::protocol::PlayerUpdate;

    /// A real, fully-built ServerGame (no save attached), as `Game::new`
    /// builds it.
    fn server_game() -> crate::server::game::ServerGame {
        crate::game::session::build_session("", 1, 1).0
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
        let mut handle = ServerHandle::spawn(server);

        handle
            .send(ClientToServer::PlayerUpdate(update))
            .expect("live server accepts messages");
        handle.send(ClientToServer::KeepAlive).expect("live server");

        let first = recv_tick(&handle, Duration::from_secs(5)).expect("a TickUpdate arrives");
        let second = recv_tick(&handle, Duration::from_secs(5)).expect("ticks keep coming");
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
        let mut handle = ServerHandle::spawn(server);
        handle
            .send(ClientToServer::PlayerUpdate(update))
            .expect("live server");
        let _ = recv_tick(&handle, Duration::from_secs(5)).expect("running before the pause");

        handle
            .send(ClientToServer::Pause(true))
            .expect("live server");
        // Let the pause land and drain any in-flight updates from before it.
        std::thread::sleep(Duration::from_millis(200));
        let mut drained = Vec::new();
        handle.drain(&mut drained);
        let last_tick = drained
            .iter()
            .filter_map(|m| match m {
                ServerToClient::Tick(u) => Some(u.tick),
                _ => None,
            })
            .max();

        // Several tick periods of silence: the sim is frozen…
        assert!(
            recv_tick(&handle, Duration::from_millis(300)).is_none(),
            "no TickUpdates while paused"
        );
        // …but the connection is alive (message drain continues server-side).
        handle.send(ClientToServer::KeepAlive).expect("still alive");
        assert!(!handle.is_crashed());

        handle
            .send(ClientToServer::Pause(false))
            .expect("live server");
        let resumed = recv_tick(&handle, Duration::from_secs(5)).expect("ticks resume");
        if let Some(last) = last_tick {
            assert!(resumed.tick > last, "the world advances again");
            // ~500 ms of pause would be ~10 banked ticks; resuming must not
            // replay them (the accumulator is pinned while paused).
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
        server.world.attach_save(opened.save);

        let mut handle = ServerHandle::spawn(server);
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
