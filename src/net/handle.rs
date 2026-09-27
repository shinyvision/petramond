use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use crate::net::connection::TcpClientConn;
use crate::net::protocol::{ClientToServer, ServerToClient};

pub enum ControlMsg {
    Shutdown,
    SaveAll,
    Command(String),
    OpenToLan {
        port: u16,
        reply: Sender<std::io::Result<u16>>,
    },
    #[cfg(any(test, feature = "test-support"))]
    PanicForTest,
    #[cfg(any(test, feature = "test-support"))]
    UnthrottleForTest,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct ServerGone;

pub struct ServerHandle {
    to_server: Sender<ClientToServer>,
    from_server: Receiver<ServerToClient>,
    control: Sender<ControlMsg>,
    join: Option<JoinHandle<()>>,
    crashed: Arc<AtomicBool>,
    remote: Option<TcpClientConn>,
    serverless: bool,
}

pub struct ServerEnd {
    pub inbox: Receiver<ClientToServer>,
    pub outbox: Sender<ServerToClient>,
    pub control: Receiver<ControlMsg>,
}

impl ServerHandle {
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
                serverless: false,
            },
            ServerEnd {
                inbox,
                outbox,
                control: control_rx,
            },
        )
    }

    pub fn spawn_with(
        run: impl FnOnce(ServerEnd, Arc<AtomicBool>) -> JoinHandle<()>,
    ) -> ServerHandle {
        let (mut handle, end) = Self::pipe();
        handle.join = Some(run(end, Arc::clone(&handle.crashed)));
        handle
    }

    pub fn from_remote(mut conn: TcpClientConn) -> ServerHandle {
        ServerHandle {
            to_server: conn.sender(),
            from_server: conn.take_receiver(),
            control: mpsc::channel().0,
            join: None,
            crashed: conn.lost_flag(),
            remote: Some(conn),
            serverless: false,
        }
    }

    pub fn serverless() -> ServerHandle {
        ServerHandle {
            to_server: mpsc::channel().0,
            from_server: mpsc::channel().1,
            control: mpsc::channel().0,
            join: None,
            crashed: Arc::new(AtomicBool::new(false)),
            remote: None,
            serverless: true,
        }
    }

    pub fn is_serverless(&self) -> bool {
        self.serverless
    }

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

    pub fn send(&self, msg: ClientToServer) -> Result<(), ServerGone> {
        if self.serverless {
            return Ok(());
        }
        self.to_server.send(msg).map_err(|_| ServerGone)
    }

    pub fn drain(&mut self, into: &mut Vec<ServerToClient>) {
        if self.serverless {
            return;
        }
        while let Ok(msg) = self.from_server.try_recv() {
            into.push(msg);
        }
    }

    pub fn save_all(&self) {
        if self.remote.is_none() {
            let _ = self.control.send(ControlMsg::SaveAll);
        }
    }

    pub fn command(&self, text: String) {
        if self.remote.is_none() {
            let _ = self.control.send(ControlMsg::Command(text));
        }
    }

    #[inline]
    pub fn is_crashed(&self) -> bool {
        self.crashed.load(Ordering::SeqCst)
    }

    pub fn shutdown_and_join(&mut self) {
        if self.serverless {
            return;
        }
        if self.remote.is_some() {
            self.to_server = mpsc::channel().0;
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

    #[cfg(any(test, feature = "test-support"))]
    pub fn panic_for_test(&self) {
        let _ = self.control.send(ControlMsg::PanicForTest);
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn unthrottle_for_test(&self) {
        let _ = self.control.send(ControlMsg::UnthrottleForTest);
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn join_for_test(&mut self) {
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn recv_timeout(&self, timeout: Duration) -> Option<ServerToClient> {
        self.from_server.recv_timeout(timeout).ok()
    }
}

impl Drop for ServerHandle {
    fn drop(&mut self) {
        self.shutdown_and_join();
    }
}

#[cfg(any(test, feature = "test-support"))]
pub struct LoopbackServer {
    pub inbox: Receiver<ClientToServer>,
    pub outbox: Sender<ServerToClient>,
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
