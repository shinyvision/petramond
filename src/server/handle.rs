use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::Ordering;
use std::sync::mpsc::{Receiver, Sender, TryRecvError};
use std::time::{Duration, Instant};

use crate::net::handle::{ControlMsg, ServerEnd, ServerHandle};
use crate::net::protocol::{ClientToServer, ServerToClient};
use crate::player::PlayerId;

use super::game::ServerGame;
use super::remote::RemoteHub;

pub fn spawn(server: ServerGame) -> ServerHandle {
    ServerHandle::spawn_with(move |end: ServerEnd, crash_flag| {
        std::thread::Builder::new()
            .name("petramond-server".to_string())
            .spawn(move || {
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

const POLL_INTERVAL: Duration = Duration::from_millis(2);

fn server_main(
    mut server: ServerGame,
    inbox: Receiver<ClientToServer>,
    outbox: Sender<ServerToClient>,
    control: Receiver<ControlMsg>,
) {
    server.mods.host().install_thread_ai_nodes();

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
                        server.clock.open_to_lan();
                    }
                    let _ = reply.send(result);
                }
                #[cfg(any(test, feature = "test-support"))]
                Ok(ControlMsg::PanicForTest) => panic!("server loop panic injected by test"),
                #[cfg(any(test, feature = "test-support"))]
                Ok(ControlMsg::UnthrottleForTest) => unthrottled = true,
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    hub.shutdown();
                    server.close_sessions_and_save();
                    return;
                }
            }
        }

        debug_assert!(msgs.is_empty(), "pump drains its inbox");
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

        hub.pump(&mut server, &mut msgs, &outbox);

        let now = Instant::now();
        let dt = (now - last).as_secs_f32();
        last = now;
        #[cfg(any(test, feature = "test-support"))]
        let dt = if unthrottled {
            crate::events::tick::TICK_DT
        } else {
            dt
        };
        let headroom = hub.send_headroom();
        let out = server.pump_tagged(dt, &mut msgs, &headroom);
        for msg in out.msgs {
            if outbox.send(msg).is_err() {
                disconnected = true;
                break;
            }
        }
        hub.route(out.remote);
        hub.kick(out.kicked, &outbox);
        if disconnected {
            hub.shutdown();
            server.close_sessions_and_save();
            return;
        }

        #[cfg(any(test, feature = "test-support"))]
        if unthrottled {
            std::thread::yield_now();
            continue;
        }
        let until_tick = Duration::from_secs_f32(server.clock.until_next_tick());
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

    const SERVER_THREAD_DEADLINE: Duration = Duration::from_secs(30);

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
                Some(_) => continue,
                None => return None,
            }
        }
        None
    }

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

        let first = recv_tick(&handle, SERVER_THREAD_DEADLINE).expect("a TickUpdate arrives");
        let second = recv_tick(&handle, SERVER_THREAD_DEADLINE).expect("ticks keep coming");
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

    #[test]
    fn pause_stops_ticks_and_resume_does_not_fast_forward() {
        let server = server_game();
        let update = player_update(&server);
        let mut handle = spawn(server);
        handle
            .send(ClientToServer::PlayerUpdate(update))
            .expect("live server");
        let _ = recv_tick(&handle, SERVER_THREAD_DEADLINE).expect("running before the pause");

        handle
            .send(ClientToServer::Pause(true))
            .expect("live server");
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

        assert!(
            recv_tick(&handle, Duration::from_millis(150)).is_none(),
            "no TickUpdates while paused"
        );
        handle.send(ClientToServer::KeepAlive).expect("still alive");
        assert!(!handle.is_crashed());

        handle
            .send(ClientToServer::Pause(false))
            .expect("live server");
        let resumed = recv_tick(&handle, SERVER_THREAD_DEADLINE).expect("ticks resume");
        if let Some(last) = last_tick {
            assert!(resumed.tick > last, "the world advances again");
            assert!(
                resumed.tick - last <= u64::from(super::super::game::MAX_TICKS_PER_FRAME),
                "resume fast-forwarded: tick jumped {} -> {}",
                last,
                resumed.tick
            );
        }
        handle.shutdown_and_join();
    }

    #[test]
    fn panicking_server_crashes_loud_and_saves_nothing() {
        let dir = petramond_util::test_dirs::TestScratchDir::new("handle-panic");

        let mut server = server_game();
        let opened = crate::save::open_at(dir.to_path_buf()).expect("temp save opens");
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
    }
}
