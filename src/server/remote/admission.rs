//! Pre-join admission: bounded, thread-free handling of connections that have
//! not authenticated yet.
//!
//! An unauthenticated socket costs NO threads. Each one is a nonblocking
//! [`TcpStream`] with a small inbox/outbox, polled from the server loop;
//! frames are decoded incrementally ([`decode_frame`]) under a
//! [`PRE_JOIN_MAX_FRAME`] cap, so a peer can never make the server buffer more
//! than that. Only a join that passed its identity proof and the admission
//! check is promoted to a [`TcpServerConn`] with reader/writer threads
//! ([`PendingConn::into_conn`]).
//!
//! The pending set itself is bounded: at most [`MAX_PENDING`] connections in
//! total and [`MAX_PENDING_PER_IP`] per peer address, each living at most
//! [`PRE_JOIN_DEADLINE`]. Over-limit sockets are closed on arrival.

use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::net::{IpAddr, SocketAddr, TcpStream};
use std::time::{Duration, Instant};

use crate::net::connection::TcpServerConn;
use crate::net::framing::{decode_frame, encode_frame};
use crate::net::identity::JoinChallenge;
use crate::net::protocol::{ClientToServer, ServerToClient};

/// A connection that hasn't completed Hello→Mods→Join within this window is
/// dropped silently.
pub(super) const PRE_JOIN_DEADLINE: Duration = Duration::from_secs(10);

/// Most connections allowed inside the handshake at once.
pub(super) const MAX_PENDING: usize = 32;

/// Most handshaking connections from one peer address at once.
pub(super) const MAX_PENDING_PER_IP: usize = 4;

/// Largest pre-join frame body. The biggest legitimate one is `Join`, whose
/// section-cache manifest is bounded by `SECTION_CACHE_CAP` claims (a few
/// bytes each).
pub(super) const PRE_JOIN_MAX_FRAME: usize = 256 * 1024;

/// Where a pending connection is in the handshake.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum Stage {
    /// Waiting for `Hello`.
    Fresh,
    /// `Hello` exchanged; `ModQuery`/`Join` are acceptable, and `Join` must
    /// prove the identity against this challenge.
    Helloed { challenge: JoinChallenge },
}

/// A connection still inside the join handshake.
pub(super) struct PendingConn {
    stream: TcpStream,
    peer: SocketAddr,
    inbox: Vec<u8>,
    outbox: Vec<u8>,
    pub(super) stage: Stage,
    deadline: Instant,
}

impl PendingConn {
    /// Take an accepted socket into the handshake (nonblocking from here).
    pub(super) fn new(stream: TcpStream, peer: SocketAddr) -> io::Result<PendingConn> {
        stream.set_nonblocking(true)?;
        stream.set_nodelay(true)?;
        Ok(PendingConn {
            stream,
            peer,
            inbox: Vec::new(),
            outbox: Vec::new(),
            stage: Stage::Fresh,
            deadline: Instant::now() + PRE_JOIN_DEADLINE,
        })
    }

    pub(super) fn peer(&self) -> SocketAddr {
        self.peer
    }

    pub(super) fn expired(&self, now: Instant) -> bool {
        now >= self.deadline
    }

    /// Queue one reply and push as much of the outbox as the socket takes.
    pub(super) fn send(&mut self, msg: &ServerToClient) -> io::Result<()> {
        self.outbox.extend_from_slice(&encode_frame(msg)?);
        self.flush()
    }

    fn flush(&mut self) -> io::Result<()> {
        while !self.outbox.is_empty() {
            match self.stream.write(&self.outbox) {
                Ok(0) => return Err(io::ErrorKind::WriteZero.into()),
                Ok(n) => {
                    self.outbox.drain(..n);
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => return Ok(()),
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }

    /// The next complete client frame, reading whatever the socket has.
    /// `Ok(None)` = nothing complete yet. Any error (EOF, reset, an oversize
    /// or undecodable frame) is terminal for the connection.
    pub(super) fn poll(&mut self) -> io::Result<Option<ClientToServer>> {
        self.flush()?;
        if let Some(msg) = self.take_frame()? {
            return Ok(Some(msg));
        }
        // Never buffer past one maximal frame: `take_frame` rejects an
        // oversize header as soon as it is complete, so this bounds a peer
        // that keeps sending without ever finishing a frame.
        let limit = PRE_JOIN_MAX_FRAME + 16;
        let mut chunk = [0u8; 4096];
        while self.inbox.len() < limit {
            match self.stream.read(&mut chunk) {
                Ok(0) => return Err(io::ErrorKind::UnexpectedEof.into()),
                Ok(n) => {
                    self.inbox.extend_from_slice(&chunk[..n]);
                    if let Some(msg) = self.take_frame()? {
                        return Ok(Some(msg));
                    }
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => return Ok(None),
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => return Err(e),
            }
        }
        Ok(None)
    }

    fn take_frame(&mut self) -> io::Result<Option<ClientToServer>> {
        match decode_frame(&self.inbox, PRE_JOIN_MAX_FRAME)? {
            Some((msg, used)) => {
                self.inbox.drain(..used);
                Ok(Some(msg))
            }
            None => Ok(None),
        }
    }

    /// Promote an admitted connection to its reader/writer threads. The
    /// client waits for each handshake reply before sending on, so by its
    /// `Join` every reply has been taken off the socket and nothing else has
    /// arrived: a still-unsent reply (a peer that stopped reading) or
    /// buffered surplus input is a protocol violation, never something the
    /// server thread blocks on.
    pub(super) fn into_conn(mut self) -> io::Result<TcpServerConn> {
        self.flush()?;
        if !self.outbox.is_empty() || !self.inbox.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "handshake traffic out of step at join",
            ));
        }
        self.stream.set_nonblocking(false)?;
        TcpServerConn::spawn(self.stream)
    }
}

/// The admission limits' view of every connection still joining: those in
/// the handshake and those waiting for their restore.
pub(super) fn admits<'a>(
    joining: impl Iterator<Item = &'a PendingConn>,
    ip: IpAddr,
) -> Result<(), &'static str> {
    let (mut total, mut from_ip) = (0, 0);
    for conn in joining {
        total += 1;
        from_ip += usize::from(conn.peer.ip() == ip);
    }
    if total >= MAX_PENDING {
        return Err("too many connections are joining");
    }
    if from_ip >= MAX_PENDING_PER_IP {
        return Err("too many joining connections from one address");
    }
    Ok(())
}

/// Per-address tally, for logging a flood once per pump instead of per
/// refused socket.
#[derive(Default)]
pub(super) struct Refusals(HashMap<IpAddr, (usize, &'static str)>);

impl Refusals {
    pub(super) fn note(&mut self, ip: IpAddr, why: &'static str) {
        self.0.entry(ip).or_insert((0, why)).0 += 1;
    }

    pub(super) fn log(self) {
        for (ip, (count, why)) in self.0 {
            log::warn!("refused {count} LAN connection(s) from {ip}: {why}");
        }
    }
}
