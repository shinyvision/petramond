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
//!   server tick. The server's terrain streamer paces itself against
//!   [`TcpServerConn::queue_headroom`] so bulk terrain can never fill the
//!   queue on its own — a full queue means the client stopped draining, not
//!   that the writer's encode+compress lost a burst race.
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
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
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
/// keeping up and gets disconnected. Terrain is paced against the live
/// headroom (`queue_headroom`), so only non-terrain traffic can fill it.
pub(crate) const SERVER_QUEUE_MSGS: usize = 4096;

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

/// Cumulative enqueued-message counts by kind. Every producer feeding a
/// remote queue is supposed to pace itself against `queue_headroom`, so a
/// queue-full kick means some producer broke that contract — these counts
/// name it in the kick log instead of leaving a silent mystery.
#[derive(Default)]
struct SendStats {
    ticks: AtomicU64,
    sections: AtomicU64,
    columns: AtomicU64,
    light: AtomicU64,
    unloads: AtomicU64,
    other: AtomicU64,
}

impl SendStats {
    fn count(&self, msg: &ServerToClient) {
        let slot = match msg {
            ServerToClient::Tick(_) => &self.ticks,
            ServerToClient::SectionData(_) => &self.sections,
            ServerToClient::ColumnData(_) => &self.columns,
            ServerToClient::LightData(_) => &self.light,
            ServerToClient::SectionUnload(_) | ServerToClient::ColumnUnload(_) => &self.unloads,
            _ => &self.other,
        };
        slot.fetch_add(1, Ordering::Relaxed);
    }
}

impl std::fmt::Display for SendStats {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let get = |a: &AtomicU64| a.load(Ordering::Relaxed);
        write!(
            f,
            "{} ticks, {} sections, {} columns, {} light, {} unloads, {} other",
            get(&self.ticks),
            get(&self.sections),
            get(&self.columns),
            get(&self.light),
            get(&self.unloads),
            get(&self.other)
        )
    }
}

/// The server side of one remote client's connection.
pub(crate) struct TcpServerConn {
    tx: SyncSender<ServerToClient>,
    rx: Receiver<ClientToServer>,
    dead: Arc<AtomicBool>,
    /// Messages currently sitting in the outbound queue (incremented by
    /// `send`, decremented by the writer as it dequeues) — the live depth
    /// behind [`queue_headroom`](Self::queue_headroom).
    queued: Arc<AtomicUsize>,
    /// What this connection was sent since it joined, for the kick log.
    stats: SendStats,
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
        let queued = Arc::new(AtomicUsize::new(0));
        let (tx, out_rx) = mpsc::sync_channel::<ServerToClient>(SERVER_QUEUE_MSGS);
        let (in_tx, rx) = mpsc::channel::<ClientToServer>();

        let reader = stream.try_clone()?;
        let flag = Arc::clone(&dead);
        std::thread::Builder::new()
            .name("petramond-conn-read".to_string())
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
        let depth = Arc::clone(&queued);
        std::thread::Builder::new()
            .name("petramond-conn-write".to_string())
            .spawn(move || {
                let mut w = BufWriter::new(writer);
                // Light crosses TCP too: the replica never bakes its own, so
                // the seeded cubes (and follow-up `LightData`) are the client's
                // ONLY light source. Mostly-uniform bytes — the frame
                // compressor crushes them.
                //
                // The map hook runs once per DEQUEUED message (never for the
                // internally-generated keepalives/farewell), so it is the
                // exact counterpart of `send`'s increment.
                write_loop(&mut w, &out_rx, ServerToClient::KeepAlive, None, |_| {
                    depth.fetch_sub(1, Ordering::Relaxed);
                });
                flag.store(true, Ordering::SeqCst);
            })
            .expect("spawn connection writer");

        Ok(TcpServerConn {
            tx,
            rx,
            dead,
            queued,
            stats: SendStats::default(),
            stream,
            peer,
        })
    }

    /// Queue one message. `false` = the connection is dead or its queue is
    /// FULL (a slow client): the caller runs the leave path — the server tick
    /// never blocks on a socket.
    pub(crate) fn send(&self, msg: ServerToClient) -> bool {
        // Count up BEFORE the send: the writer may dequeue (and decrement)
        // the instant try_send returns, and a post-send increment would then
        // transiently wrap the depth so headroom reads as zero.
        self.queued.fetch_add(1, Ordering::Relaxed);
        self.stats.count(&msg);
        match self.tx.try_send(msg) {
            Ok(()) => true,
            Err(TrySendError::Full(_)) => {
                self.queued.fetch_sub(1, Ordering::Relaxed);
                log::warn!(
                    "client {} outran its send queue; disconnecting (sent since join: {})",
                    self.peer,
                    self.stats
                );
                self.dead.store(true, Ordering::SeqCst);
                false
            }
            Err(TrySendError::Disconnected(_)) => {
                self.queued.fetch_sub(1, Ordering::Relaxed);
                self.dead.store(true, Ordering::SeqCst);
                false
            }
        }
    }

    /// Free slots in the outbound queue right now (approximate — the writer
    /// drains concurrently, so the true value is only ever HIGHER). The
    /// terrain streamer paces each pump's plan against this so a joining
    /// client's world load throttles to what the writer (encode + zlib +
    /// socket) actually sustains instead of overflowing the queue.
    pub(crate) fn queue_headroom(&self) -> usize {
        SERVER_QUEUE_MSGS.saturating_sub(self.queued.load(Ordering::Relaxed))
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
            .name("petramond-conn-read".to_string())
            .spawn(move || {
                let mut r = BufReader::new(reader);
                while let Ok(mut msg) = read_msg::<ServerToClient, _>(&mut r) {
                    map.remap_to_client(&mut msg);
                    if in_tx.send(msg).is_err() {
                        break;
                    }
                }
                flag.store(true, Ordering::SeqCst);
            })
            .expect("spawn connection reader");

        let writer = stream.try_clone()?;
        let flag = Arc::clone(&lost);
        std::thread::Builder::new()
            .name("petramond-conn-write".to_string())
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::time::Instant;

    /// Every `send` increment must be matched by a writer-drain decrement,
    /// or headroom leaks downward until terrain pacing starves a healthy
    /// client. Queue a burst, then watch headroom return to full.
    #[test]
    fn queue_headroom_recovers_after_the_writer_drains() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let addr = listener.local_addr().expect("addr");
        let _client = TcpStream::connect(addr).expect("connect");
        let (accepted, _) = listener.accept().expect("accept");
        let conn = TcpServerConn::spawn(accepted).expect("conn threads");

        assert_eq!(
            conn.queue_headroom(),
            SERVER_QUEUE_MSGS,
            "fresh queue is empty"
        );
        for _ in 0..100 {
            assert!(
                conn.send(ServerToClient::KeepAlive),
                "live connection accepts"
            );
        }
        let deadline = Instant::now() + Duration::from_secs(10);
        while conn.queue_headroom() < SERVER_QUEUE_MSGS {
            assert!(Instant::now() < deadline, "writer never drained the burst");
            std::thread::yield_now();
        }
    }
}
