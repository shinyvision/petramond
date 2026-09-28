use std::io::{self, BufReader, BufWriter, Write};
use std::net::{Shutdown, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, SyncSender, TrySendError};
use std::sync::Arc;
use std::time::Duration;

use super::framing::{read_opened, write_sealed, MAX_FRAME};
use super::protocol::{ClientToServer, ServerToClient};
use super::rate::TokenBucket;
use super::remap::IdRemap;
use super::secure::{Channel, FrameSealer};

const KEEPALIVE_AFTER: Duration = Duration::from_secs(2);

const READ_TIMEOUT: Duration = Duration::from_secs(10);

const WRITE_TIMEOUT: Duration = Duration::from_secs(10);

pub const SERVER_QUEUE_MSGS: usize = 4096;

pub const CLIENT_INBOX_MSGS: usize = 4096;

#[derive(Copy, Clone, Debug)]
pub struct InboundLimits {
    pub max_frame: usize,
    pub msgs_per_second: f64,
    pub msg_burst: f64,
    pub bytes_per_second: f64,
    pub byte_burst: f64,
}

impl Default for InboundLimits {
    fn default() -> Self {
        Self {
            max_frame: 64 * 1024,
            msgs_per_second: 2400.0,
            msg_burst: 4800.0,
            bytes_per_second: 512.0 * 1024.0,
            byte_burst: 2.0 * 1024.0 * 1024.0,
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Overrun {
    MessageRate,
    ByteRate,
    Queue,
}

struct InboundMeter {
    msgs: TokenBucket,
    bytes: TokenBucket,
}

impl InboundMeter {
    fn new(limits: &InboundLimits, now: std::time::Instant) -> Self {
        Self {
            msgs: TokenBucket::new(limits.msg_burst, limits.msgs_per_second, now),
            bytes: TokenBucket::new(limits.byte_burst, limits.bytes_per_second, now),
        }
    }

    fn admit(&mut self, bytes: usize, now: std::time::Instant) -> Result<(), Overrun> {
        if !self.msgs.try_take(1.0, now) {
            return Err(Overrun::MessageRate);
        }
        if !self.bytes.try_take(bytes as f64, now) {
            return Err(Overrun::ByteRate);
        }
        Ok(())
    }
}

fn configure(stream: &TcpStream) -> io::Result<()> {
    stream.set_nodelay(true)?;
    stream.set_read_timeout(Some(READ_TIMEOUT))?;
    stream.set_write_timeout(Some(WRITE_TIMEOUT))?;
    Ok(())
}

fn write_loop<T: serde::Serialize>(
    w: &mut impl Write,
    sealer: &mut FrameSealer,
    rx: &Receiver<T>,
    keepalive: T,
    farewell: Option<T>,
    map: impl Fn(&mut T),
) {
    loop {
        match rx.recv_timeout(KEEPALIVE_AFTER) {
            Ok(mut msg) => {
                map(&mut msg);
                if write_sealed(w, &msg, sealer).is_err() {
                    return;
                }
                while let Ok(mut msg) = rx.try_recv() {
                    map(&mut msg);
                    if write_sealed(w, &msg, sealer).is_err() {
                        return;
                    }
                }
                if w.flush().is_err() {
                    return;
                }
            }
            Err(RecvTimeoutError::Timeout) => {
                if write_sealed(w, &keepalive, sealer).is_err() || w.flush().is_err() {
                    return;
                }
            }
            Err(RecvTimeoutError::Disconnected) => {
                if let Some(msg) = farewell {
                    let _ = write_sealed(w, &msg, sealer);
                }
                let _ = w.flush();
                return;
            }
        }
    }
}

fn spawn_writer(stream: &TcpStream, body: impl FnOnce() + Send + 'static) -> io::Result<()> {
    std::thread::Builder::new()
        .name("petramond-conn-write".to_string())
        .spawn(body)
        .map(drop)
        .inspect_err(|_| {
            let _ = stream.shutdown(Shutdown::Both);
        })
}

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
            ServerToClient::SectionData(_) | ServerToClient::SectionCached { .. } => &self.sections,
            ServerToClient::ColumnData(_) => &self.columns,
            ServerToClient::LightData(_) => &self.light,
            ServerToClient::SectionUnload { .. } | ServerToClient::ColumnUnload { .. } => {
                &self.unloads
            }
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

pub struct TcpServerConn {
    tx: SyncSender<ServerToClient>,
    rx: Receiver<ClientToServer>,
    dead: Arc<AtomicBool>,
    queued: Arc<AtomicUsize>,
    stats: SendStats,
    stream: TcpStream,
    peer: String,
}

impl TcpServerConn {
    /// `channel` is the server side of the keys agreed during the handshake; every frame from
    /// here on is sealed and opened with it.
    pub fn spawn(stream: TcpStream, channel: Channel) -> io::Result<TcpServerConn> {
        Self::spawn_with_limits(stream, channel, InboundLimits::default())
    }

    pub fn spawn_with_limits(
        stream: TcpStream,
        channel: Channel,
        limits: InboundLimits,
    ) -> io::Result<TcpServerConn> {
        let Channel {
            mut sealer,
            mut opener,
        } = channel;
        configure(&stream)?;
        let peer = stream
            .peer_addr()
            .map(|a| a.to_string())
            .unwrap_or_else(|_| "?".to_string());
        let dead = Arc::new(AtomicBool::new(false));
        let queued = Arc::new(AtomicUsize::new(0));
        let (tx, out_rx) = mpsc::sync_channel::<ServerToClient>(SERVER_QUEUE_MSGS);
        let (in_tx, rx) = mpsc::sync_channel::<ClientToServer>(CLIENT_INBOX_MSGS);

        let reader = stream.try_clone()?;
        let flag = Arc::clone(&dead);
        let reader_peer = peer.clone();
        std::thread::Builder::new()
            .name("petramond-conn-read".to_string())
            .spawn(move || {
                let shutdown = reader.try_clone();
                let mut r = BufReader::new(reader);
                let mut meter = InboundMeter::new(&limits, std::time::Instant::now());
                while let Ok((msg, bytes)) =
                    read_opened::<ClientToServer, _>(&mut r, limits.max_frame, &mut opener)
                {
                    let verdict = meter
                        .admit(bytes, std::time::Instant::now())
                        .and_then(|()| match in_tx.try_send(msg) {
                            Ok(()) => Ok(()),
                            Err(TrySendError::Full(_)) => Err(Overrun::Queue),
                            Err(TrySendError::Disconnected(_)) => Ok(()),
                        });
                    if let Err(overrun) = verdict {
                        log::warn!("client {reader_peer} exceeded its inbound limit ({overrun:?}); disconnecting");
                        if let Ok(stream) = &shutdown {
                            let _ = stream.shutdown(Shutdown::Both);
                        }
                        break;
                    }
                }
                flag.store(true, Ordering::SeqCst);
            })?;

        let writer = stream.try_clone()?;
        let flag = Arc::clone(&dead);
        let depth = Arc::clone(&queued);
        spawn_writer(&stream, move || {
            let mut w = BufWriter::new(writer);
            write_loop(
                &mut w,
                &mut sealer,
                &out_rx,
                ServerToClient::KeepAlive,
                None,
                |_| {
                    depth.fetch_sub(1, Ordering::Relaxed);
                },
            );
            flag.store(true, Ordering::SeqCst);
        })?;

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

    pub fn send(&self, msg: ServerToClient) -> bool {
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

    pub fn queue_headroom(&self) -> usize {
        SERVER_QUEUE_MSGS.saturating_sub(self.queued.load(Ordering::Relaxed))
    }

    pub fn try_recv(&self) -> Option<ClientToServer> {
        self.rx.try_recv().ok()
    }

    #[inline]
    pub fn is_dead(&self) -> bool {
        self.dead.load(Ordering::SeqCst)
    }

    pub fn peer(&self) -> &str {
        &self.peer
    }
}

impl Drop for TcpServerConn {
    fn drop(&mut self) {
        let _ = self.stream.shutdown(Shutdown::Read);
    }
}

pub struct TcpClientConn {
    to_server: Sender<ClientToServer>,
    from_server: Option<Receiver<ServerToClient>>,
    lost: Arc<AtomicBool>,
    stream: TcpStream,
}

impl TcpClientConn {
    pub fn spawn(stream: TcpStream, remap: IdRemap, channel: Channel) -> io::Result<TcpClientConn> {
        configure(&stream)?;
        let Channel {
            mut sealer,
            mut opener,
        } = channel;
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
                while let Ok((mut msg, _)) =
                    read_opened::<ServerToClient, _>(&mut r, MAX_FRAME, &mut opener)
                {
                    map.remap_to_client(&mut msg);
                    if in_tx.send(msg).is_err() {
                        break;
                    }
                }
                flag.store(true, Ordering::SeqCst);
            })?;

        let writer = stream.try_clone()?;
        let flag = Arc::clone(&lost);
        spawn_writer(&stream, move || {
            let mut w = BufWriter::new(writer);
            write_loop(
                &mut w,
                &mut sealer,
                &out_rx,
                ClientToServer::KeepAlive,
                Some(ClientToServer::Disconnect),
                |msg| remap.remap_to_server(msg),
            );
            flag.store(true, Ordering::SeqCst);
        })?;

        Ok(TcpClientConn {
            to_server,
            from_server: Some(from_server),
            lost,
            stream,
        })
    }

    pub fn sender(&self) -> Sender<ClientToServer> {
        self.to_server.clone()
    }

    pub fn take_receiver(&mut self) -> Receiver<ServerToClient> {
        self.from_server.take().expect("receiver taken once")
    }

    pub fn lost_flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.lost)
    }
}

impl Drop for TcpClientConn {
    fn drop(&mut self) {
        let _ = self.stream.shutdown(Shutdown::Read);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::secure::channels_for_test;
    use std::net::TcpListener;
    use std::time::Instant;

    #[test]
    fn the_inbound_meter_names_the_budget_a_flood_exhausted() {
        let t0 = Instant::now();
        let limits = InboundLimits {
            max_frame: 1024,
            msgs_per_second: 1.0,
            msg_burst: 2.0,
            bytes_per_second: 1.0,
            byte_burst: 100.0,
        };
        let mut meter = InboundMeter::new(&limits, t0);
        assert_eq!(meter.admit(10, t0), Ok(()));
        assert_eq!(meter.admit(10, t0), Ok(()));
        assert_eq!(meter.admit(10, t0), Err(Overrun::MessageRate));

        let mut meter = InboundMeter::new(&limits, t0);
        assert_eq!(meter.admit(101, t0), Err(Overrun::ByteRate));
    }

    #[test]
    fn a_flooding_client_is_disconnected() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let addr = listener.local_addr().expect("addr");
        let mut client = TcpStream::connect(addr).expect("connect");
        let (accepted, _) = listener.accept().expect("accept");
        let limits = InboundLimits {
            msgs_per_second: 1.0,
            msg_burst: 8.0,
            ..InboundLimits::default()
        };
        let (mut keys, server_keys) = channels_for_test();
        let conn =
            TcpServerConn::spawn_with_limits(accepted, server_keys, limits).expect("conn threads");
        for _ in 0..64 {
            if write_sealed(&mut client, &ClientToServer::KeepAlive, &mut keys.sealer).is_err() {
                break;
            }
        }
        let _ = client.flush();
        let deadline = Instant::now() + Duration::from_secs(10);
        while !conn.is_dead() {
            assert!(Instant::now() < deadline, "the flood was never cut off");
            std::thread::yield_now();
        }
        let mut delivered = 0;
        while conn.try_recv().is_some() {
            delivered += 1;
        }
        assert!(
            delivered < 16,
            "only about the burst got through ({delivered})"
        );
    }

    #[test]
    fn queue_headroom_recovers_after_the_writer_drains() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let addr = listener.local_addr().expect("addr");
        let _client = TcpStream::connect(addr).expect("connect");
        let (accepted, _) = listener.accept().expect("accept");
        let conn = TcpServerConn::spawn(accepted, channels_for_test().1).expect("conn threads");

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

    #[test]
    fn an_unsealed_frame_on_a_joined_connection_drops_it() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let addr = listener.local_addr().expect("addr");
        let mut client = TcpStream::connect(addr).expect("connect");
        let (accepted, _) = listener.accept().expect("accept");
        let conn = TcpServerConn::spawn(accepted, channels_for_test().1).expect("conn threads");
        crate::net::framing::write_msg(&mut client, &ClientToServer::Pause(true)).expect("send");
        let deadline = Instant::now() + Duration::from_secs(10);
        while !conn.is_dead() {
            assert!(Instant::now() < deadline, "the injected frame was accepted");
            std::thread::yield_now();
        }
        assert_eq!(conn.try_recv(), None, "nothing unauthenticated got through");
    }
}
