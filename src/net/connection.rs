//! Per-connection TCP reader/writer threads (multiplayer Phase E).
//!
//! Both sides present the SAME shape the in-process pipe does — mpsc channels
//! of protocol message values — so nothing above the transport knows whether
//! a connection is local or TCP:
//!
//! - [`TcpServerConn`] (one per remote client, owned by the server thread's
//!   `RemoteHub`): the reader decodes `ClientToServer` into an unbounded
//!   inbound channel the server loop drains; the writer drains a BOUNDED
//!   queue of `ServerToClient` — a full queue (a client slower than the
//!   server produces) marks the connection dead instead of ever blocking the
//!   server tick.
//! - [`TcpClientConn`] (the remote client's pipe, wrapped by
//!   `ServerHandle::from_remote`): the writer applies `remap_to_server`
//!   before encoding and sends a farewell `Disconnect` when its channel
//!   closes; the reader applies `remap_to_client` right after decode — the
//!   `IdRemap` is built from `JoinData::tables` BEFORE the threads spawn, so
//!   no message ever crosses un-remapped.
//!
//! Liveness: writers send `KeepAlive` after [`KEEPALIVE_AFTER`] of outbound
//! silence; readers run under a [`READ_TIMEOUT`] socket timeout, so a peer
//! silent for that long reads as a lost connection. Sockets are NODELAY.
//! Threads exit on their own (reader: socket error/shutdown; writer: channel
//! close, after draining + flushing farewells) and are never joined.

use std::io::{self, BufReader, BufWriter, Write};
use std::net::{Shutdown, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, SyncSender, TrySendError};
use std::sync::Arc;
use std::time::Duration;

use super::framing::{read_msg, write_msg};
use super::protocol::{ClientToServer, ServerToClient};
use super::remap::IdRemap;

/// Writer keepalive: send a `KeepAlive` frame after this much outbound silence.
const KEEPALIVE_AFTER: Duration = Duration::from_secs(2);

/// Reader deadline: no frame for this long = the connection is lost (the
/// peer's writer keepalives every 2 s, so this is five missed heartbeats).
const READ_TIMEOUT: Duration = Duration::from_secs(10);

/// Writer stall deadline: a peer that accepts no bytes for this long is dead.
const WRITE_TIMEOUT: Duration = Duration::from_secs(10);

/// Server→client writer queue depth (messages). TickUpdates + a terrain
/// backlog fit comfortably; a client that lets 4096 messages pile up is not
/// keeping up and gets disconnected.
const SERVER_QUEUE_MSGS: usize = 4096;

fn configure(stream: &TcpStream) -> io::Result<()> {
    stream.set_nodelay(true)?;
    stream.set_read_timeout(Some(READ_TIMEOUT))?;
    stream.set_write_timeout(Some(WRITE_TIMEOUT))?;
    Ok(())
}

/// The shared writer-thread loop: drain the queue (batching a backlog into one
/// flush), keepalive on idle, farewell + flush when the channel closes, exit
/// on any write error. `map` rewrites a message in place before encoding (the
/// transport-boundary hook: client-side id remap, server-side light strip).
fn write_loop<T: serde::Serialize>(
    w: &mut impl Write,
    rx: &Receiver<T>,
    keepalive: T,
    farewell: Option<T>,
    map: impl Fn(&mut T),
) {
    loop {
        match rx.recv_timeout(KEEPALIVE_AFTER) {
            Ok(mut msg) => {
                map(&mut msg);
                if write_msg(w, &msg).is_err() {
                    return;
                }
                while let Ok(mut msg) = rx.try_recv() {
                    map(&mut msg);
                    if write_msg(w, &msg).is_err() {
                        return;
                    }
                }
                if w.flush().is_err() {
                    return;
                }
            }
            Err(RecvTimeoutError::Timeout) => {
                if write_msg(w, &keepalive).is_err() || w.flush().is_err() {
                    return;
                }
            }
            Err(RecvTimeoutError::Disconnected) => {
                if let Some(msg) = farewell {
                    let _ = write_msg(w, &msg);
                }
                let _ = w.flush();
                return;
            }
        }
    }
}

/// The server side of one remote client's connection.
pub(crate) struct TcpServerConn {
    tx: SyncSender<ServerToClient>,
    rx: Receiver<ClientToServer>,
    dead: Arc<AtomicBool>,
    stream: TcpStream,
    peer: String,
}

impl TcpServerConn {
    /// Take ownership of an accepted socket and spawn its reader/writer.
    pub(crate) fn spawn(stream: TcpStream) -> io::Result<TcpServerConn> {
        configure(&stream)?;
        let peer = stream
            .peer_addr()
            .map(|a| a.to_string())
            .unwrap_or_else(|_| "?".to_string());
        let dead = Arc::new(AtomicBool::new(false));
        let (tx, out_rx) = mpsc::sync_channel::<ServerToClient>(SERVER_QUEUE_MSGS);
        let (in_tx, rx) = mpsc::channel::<ClientToServer>();

        let reader = stream.try_clone()?;
        let flag = Arc::clone(&dead);
        std::thread::Builder::new()
            .name("llamacraft-conn-read".to_string())
            .spawn(move || {
                let mut r = BufReader::new(reader);
                while let Ok(msg) = read_msg::<ClientToServer, _>(&mut r) {
                    if in_tx.send(msg).is_err() {
                        break;
                    }
                }
                flag.store(true, Ordering::SeqCst);
            })
            .expect("spawn connection reader");

        let writer = stream.try_clone()?;
        let flag = Arc::clone(&dead);
        std::thread::Builder::new()
            .name("llamacraft-conn-write".to_string())
            .spawn(move || {
                let mut w = BufWriter::new(writer);
                // Light crosses TCP too: the replica never bakes its own, so
                // the seeded cubes (and follow-up `LightData`) are the client's
                // ONLY light source. Mostly-uniform bytes — the frame
                // compressor crushes them.
                write_loop(&mut w, &out_rx, ServerToClient::KeepAlive, None, |_| {});
                flag.store(true, Ordering::SeqCst);
            })
            .expect("spawn connection writer");

        Ok(TcpServerConn {
            tx,
            rx,
            dead,
            stream,
            peer,
        })
    }

    /// Queue one message. `false` = the connection is dead or its queue is
    /// FULL (a slow client): the caller runs the leave path — the server tick
    /// never blocks on a socket.
    pub(crate) fn send(&self, msg: ServerToClient) -> bool {
        match self.tx.try_send(msg) {
            Ok(()) => true,
            Err(TrySendError::Full(_)) => {
                log::warn!("client {} outran its send queue; disconnecting", self.peer);
                self.dead.store(true, Ordering::SeqCst);
                false
            }
            Err(TrySendError::Disconnected(_)) => {
                self.dead.store(true, Ordering::SeqCst);
                false
            }
        }
    }

    pub(crate) fn try_recv(&self) -> Option<ClientToServer> {
        self.rx.try_recv().ok()
    }

    #[inline]
    pub(crate) fn is_dead(&self) -> bool {
        self.dead.load(Ordering::SeqCst)
    }

    pub(crate) fn peer(&self) -> &str {
        &self.peer
    }
}

impl Drop for TcpServerConn {
    fn drop(&mut self) {
        // Unblock the reader; the writer keeps the write half until it has
        // drained + flushed any farewell frames (HelloReject/JoinReject/
        // ServerClosing), then exits — the socket closes with its last clone.
        let _ = self.stream.shutdown(Shutdown::Read);
    }
}

/// The client side of a remote-server connection, spawned over a
/// post-handshake stream. `ServerHandle::from_remote` splits it into the
/// same sender/receiver pair the in-process server thread presents.
pub(crate) struct TcpClientConn {
    to_server: Sender<ClientToServer>,
    from_server: Option<Receiver<ServerToClient>>,
    lost: Arc<AtomicBool>,
    stream: TcpStream,
}

impl TcpClientConn {
    /// Spawn the reader/writer threads. `remap` comes from
    /// `IdRemap::build(&join.tables)` (identity when the registries match) and
    /// is installed in both threads before any message crosses.
    pub(crate) fn spawn(stream: TcpStream, remap: IdRemap) -> io::Result<TcpClientConn> {
        configure(&stream)?;
        let remap = Arc::new(remap);
        let lost = Arc::new(AtomicBool::new(false));
        let (to_server, out_rx) = mpsc::channel::<ClientToServer>();
        let (in_tx, from_server) = mpsc::channel::<ServerToClient>();

        let reader = stream.try_clone()?;
        let flag = Arc::clone(&lost);
        let map = Arc::clone(&remap);
        std::thread::Builder::new()
            .name("llamacraft-conn-read".to_string())
            .spawn(move || {
                let mut r = BufReader::new(reader);
                loop {
                    match read_msg::<ServerToClient, _>(&mut r) {
                        Ok(mut msg) => {
                            map.remap_to_client(&mut msg);
                            if in_tx.send(msg).is_err() {
                                break;
                            }
                        }
                        Err(_) => break,
                    }
                }
                flag.store(true, Ordering::SeqCst);
            })
            .expect("spawn connection reader");

        let writer = stream.try_clone()?;
        let flag = Arc::clone(&lost);
        std::thread::Builder::new()
            .name("llamacraft-conn-write".to_string())
            .spawn(move || {
                let mut w = BufWriter::new(writer);
                // Farewell: whoever drops the last sender (a clean quit or a
                // hard client teardown) still tells the server goodbye.
                write_loop(
                    &mut w,
                    &out_rx,
                    ClientToServer::KeepAlive,
                    Some(ClientToServer::Disconnect),
                    |msg| remap.remap_to_server(msg),
                );
                flag.store(true, Ordering::SeqCst);
            })
            .expect("spawn connection writer");

        Ok(TcpClientConn {
            to_server,
            from_server: Some(from_server),
            lost,
            stream,
        })
    }

    pub(crate) fn sender(&self) -> Sender<ClientToServer> {
        self.to_server.clone()
    }

    /// The inbound message stream; taken once, by `ServerHandle::from_remote`.
    pub(crate) fn take_receiver(&mut self) -> Receiver<ServerToClient> {
        self.from_server.take().expect("receiver taken once")
    }

    /// Set once the connection is lost (reader EOF/timeout, writer error) —
    /// the remote `ServerHandle`'s crashed flag.
    pub(crate) fn lost_flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.lost)
    }
}

impl Drop for TcpClientConn {
    fn drop(&mut self) {
        // Mirror of `TcpServerConn`: unblock the reader only; the writer
        // flushes its farewell `Disconnect` before the socket fully closes.
        let _ = self.stream.shutdown(Shutdown::Read);
    }
}
