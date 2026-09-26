//! The client's transport-agnostic handle to a server.
//!
//! A [`ServerHandle`] is a pair of protocol-message channels plus a lifecycle
//! control channel. What sits on the far end is invisible to the holder: the
//! in-process listen server's thread (`server::handle` spawns it), a remote
//! server's TCP connection threads ([`ServerHandle::from_remote`]), or a test
//! harness pumping the loopback pipe synchronously
//! ([`ServerHandle::loopback`]). Messages are plain values — no serialization
//! in-process; `Arc` payloads are refcount bumps.
//!
//! Lifecycle as seen from the handle:
//! - [`ControlMsg::Shutdown`] (sent by [`ServerHandle::shutdown_and_join`])
//!   makes an in-process server save everything and exit; the join returns
//!   after the save queued.
//! - A crashed server (or a lost remote connection) surfaces through
//!   [`ServerHandle::is_crashed`], and further sends fail with [`ServerGone`].
//! - Dropping the handle without a shutdown is treated as shutdown-with-save.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use crate::net::connection::TcpClientConn;
use crate::net::protocol::{ClientToServer, ServerToClient};

/// Client→server lifecycle requests that are NOT gameplay messages (those ride
/// the `ClientToServer` channel). Internal — never on the wire.
pub enum ControlMsg {
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
    #[cfg(any(test, feature = "test-support"))]
    PanicForTest,
    /// Drop real-clock pacing: the loop advances one fixed tick per iteration
    /// as fast as the machine pumps, for tests that wait on many sim ticks
    /// over real channels (they must not race the wall clock under load).
    #[cfg(any(test, feature = "test-support"))]
    UnthrottleForTest,
}

/// The server thread is gone (crashed or shut down): the sending half of the
/// gameplay channel has no receiver. Carried as an error so callers surface a
/// lost connection instead of silently dropping messages.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct ServerGone;

/// The client's handle to the server: message senders/receiver, the control
/// channel, and the join handle. The server end is EITHER an in-process
/// server thread or a remote server's TCP connection threads
/// (`from_remote`) — everything above the handle is agnostic.
pub struct ServerHandle {
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

/// The server end of an in-process pipe: what a server loop drains and feeds.
pub struct ServerEnd {
    pub inbox: Receiver<ClientToServer>,
    pub outbox: Sender<ServerToClient>,
    pub control: Receiver<ControlMsg>,
}

impl ServerHandle {
    /// A fresh in-process channel set: the client half (no thread attached
    /// yet) and the server end a loop services.
    fn pipe() -> (ServerHandle, ServerEnd) {
        let (to_server, inbox) = mpsc::channel::<ClientToServer>();
        let (outbox, from_server) = mpsc::channel::<ServerToClient>();
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
            ServerEnd {
                inbox,
                outbox,
                control: control_rx,
            },
        )
    }

    /// A handle whose server end runs on a thread the caller spawns: `run`
    /// receives the server end plus the crash flag it must raise if its loop
    /// dies, and returns the thread's join handle. The in-process listen
    /// server (`server::handle::spawn`) is the one production caller.
    pub fn spawn_with(
        run: impl FnOnce(ServerEnd, Arc<AtomicBool>) -> JoinHandle<()>,
    ) -> ServerHandle {
        let (mut handle, end) = Self::pipe();
        handle.join = Some(run(end, Arc::clone(&handle.crashed)));
        handle
    }

    /// A handle whose server end is a REMOTE server over TCP: the connection's
    /// reader/writer threads present the same channel pair the server thread
    /// does. Built by the connect worker after `client_handshake`
    /// succeeded and `TcpClientConn::spawn` installed the id remap.
    pub fn from_remote(mut conn: TcpClientConn) -> ServerHandle {
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
    /// synchronously. Same channels, same messages, no thread.
    #[cfg(any(test, feature = "test-support"))]
    pub fn loopback() -> (ServerHandle, LoopbackServer) {
        let (handle, end) = Self::pipe();
        (
            handle,
            LoopbackServer {
                inbox: end.inbox,
                outbox: end.outbox,
                control: end.control,
            },
        )
    }

    /// Open the running server to LAN on `port` (0 = ephemeral); blocks up to
    /// 5 s for the bind result, whose `Ok` carries the actual port. The E2
    /// pause-menu action calls this.
    pub fn open_to_lan(&self, port: u16) -> std::io::Result<u16> {
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
    pub fn send(&self, msg: ClientToServer) -> Result<(), ServerGone> {
        self.to_server.send(msg).map_err(|_| ServerGone)
    }

    /// Drain every pending server→client message, in order, into `into`.
    pub fn drain(&mut self, into: &mut Vec<ServerToClient>) {
        while let Ok(msg) = self.from_server.try_recv() {
            into.push(msg);
        }
    }

    /// Ask the server to save everything now (it keeps running). A REMOTE
    /// server saves autonomously — nothing to ask.
    pub fn save_all(&self) {
        if self.remote.is_none() {
            let _ = self.control.send(ControlMsg::SaveAll);
        }
    }

    /// Execute one unprefixed command on a local/headless server.
    pub fn command(&self, text: String) {
        if self.remote.is_none() {
            let _ = self.control.send(ControlMsg::Command(text));
        }
    }

    #[inline]
    pub fn is_crashed(&self) -> bool {
        self.crashed.load(Ordering::SeqCst)
    }

    /// Send Shutdown and join: the thread saves everything before exiting.
    /// Idempotent — a second call (e.g. the `Drop` safety net) is a no-op.
    /// A REMOTE handle instead closes the connection: dropping the last
    /// message sender makes the writer thread flush a farewell `Disconnect`
    /// before the socket closes; the remote server saves our player on leave.
    pub fn shutdown_and_join(&mut self) {
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
    #[cfg(any(test, feature = "test-support"))]
    pub fn panic_for_test(&self) {
        let _ = self.control.send(ControlMsg::PanicForTest);
    }

    /// Pin the server loop's clock: one fixed tick per iteration, no
    /// real-time sleeps. Tests that wait on many sim ticks over the real
    /// channels (a TCP join, a long fall, heavy streaming) run compute-bound
    /// instead of racing the wall clock under machine load.
    #[cfg(any(test, feature = "test-support"))]
    pub fn unthrottle_for_test(&self) {
        let _ = self.control.send(ControlMsg::UnthrottleForTest);
    }

    /// Wait for the server thread to exit WITHOUT requesting a save-shutdown —
    /// for tests that made it exit another way (panic).
    #[cfg(any(test, feature = "test-support"))]
    pub fn join_for_test(&mut self) {
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }

    /// Blocking receive with a deadline, for tests awaiting a message.
    #[cfg(any(test, feature = "test-support"))]
    pub fn recv_timeout(&self, timeout: Duration) -> Option<ServerToClient> {
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
/// drains `inbox` into the server's pump and forwards the pump's messages
/// through `outbox`, standing in for the thread.
#[cfg(any(test, feature = "test-support"))]
pub struct LoopbackServer {
    pub inbox: Receiver<ClientToServer>,
    pub outbox: Sender<ServerToClient>,
    /// Kept alive so the handle's `Drop` (which sends `Shutdown`) never
    /// errors; the synchronous harness has no loop to control.
    #[allow(dead_code)]
    pub control: Receiver<ControlMsg>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loopback_carries_messages_both_ways_in_order() {
        let (mut handle, server) = ServerHandle::loopback();
        handle.send(ClientToServer::KeepAlive).expect("pipe open");
        handle.send(ClientToServer::Pause(true)).expect("pipe open");
        assert!(matches!(
            server.inbox.try_recv(),
            Ok(ClientToServer::KeepAlive)
        ));
        assert!(matches!(
            server.inbox.try_recv(),
            Ok(ClientToServer::Pause(true))
        ));

        server
            .outbox
            .send(ServerToClient::KeepAlive)
            .expect("handle alive");
        let mut got = Vec::new();
        handle.drain(&mut got);
        assert_eq!(got.len(), 1);
        assert!(!handle.is_crashed());
    }

    #[test]
    fn a_dropped_server_end_surfaces_as_server_gone() {
        let (handle, server) = ServerHandle::loopback();
        drop(server);
        assert_eq!(handle.send(ClientToServer::KeepAlive), Err(ServerGone));
    }

    #[test]
    fn spawn_with_joins_the_callers_thread_on_shutdown() {
        let mut handle = ServerHandle::spawn_with(|end, _crashed| {
            std::thread::spawn(move || {
                // A minimal loop: exit on Shutdown like the real server.
                while let Ok(msg) = end.control.recv() {
                    if matches!(msg, ControlMsg::Shutdown) {
                        break;
                    }
                }
            })
        });
        handle.shutdown_and_join();
        assert!(!handle.is_crashed(), "a clean shutdown is not a crash");
    }
}
