use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::net::{IpAddr, SocketAddr, TcpStream};
use std::time::{Duration, Instant};

use crate::net::connection::TcpServerConn;
use crate::net::framing::{decode_frame, decode_opened, encode_frame, encode_sealed};
use crate::net::identity::JoinChallenge;
use crate::net::protocol::{ClientToServer, ServerToClient};
use crate::net::secure::{Binding, Channel, Ephemeral, TAG_LEN};

pub(super) const PRE_JOIN_DEADLINE: Duration = Duration::from_secs(10);

pub(super) const MAX_PENDING: usize = 32;

pub(super) const MAX_PENDING_PER_IP: usize = 4;

pub(super) const PRE_JOIN_MAX_FRAME: usize = 256 * 1024;

pub(super) enum Stage {
    Fresh,
    /// `HelloAck` sent with our key share; waiting for the client's.
    Helloed {
        challenge: JoinChallenge,
        ephemeral: Ephemeral,
    },
    /// Keys agreed: every frame is sealed, and a join proof or ticket must match this binding.
    Keyed {
        binding: Binding,
        ticket_server_id: String,
    },
}

pub(super) struct PendingConn {
    stream: TcpStream,
    peer: SocketAddr,
    inbox: Vec<u8>,
    outbox: Vec<u8>,
    pub(super) stage: Stage,
    channel: Option<Channel>,
    deadline: Instant,
}

impl PendingConn {
    pub(super) fn new(stream: TcpStream, peer: SocketAddr) -> io::Result<PendingConn> {
        stream.set_nonblocking(true)?;
        stream.set_nodelay(true)?;
        Ok(PendingConn {
            stream,
            peer,
            inbox: Vec::new(),
            outbox: Vec::new(),
            stage: Stage::Fresh,
            channel: None,
            deadline: Instant::now() + PRE_JOIN_DEADLINE,
        })
    }

    pub(super) fn peer(&self) -> SocketAddr {
        self.peer
    }

    pub(super) fn expired(&self, now: Instant) -> bool {
        now >= self.deadline
    }

    pub(super) fn send(&mut self, msg: &ServerToClient) -> io::Result<()> {
        let frame = match &mut self.channel {
            Some(channel) => encode_sealed(msg, &mut channel.sealer)?,
            None => encode_frame(msg)?,
        };
        self.outbox.extend_from_slice(&frame);
        self.flush()
    }

    /// Switches both directions to sealed frames. The client seals everything after its key
    /// share, so any of it already in the inbox opens with this channel too.
    pub(super) fn key(&mut self, channel: Channel) {
        self.channel = Some(channel);
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

    pub(super) fn poll(&mut self) -> io::Result<Option<ClientToServer>> {
        self.flush()?;
        if let Some(msg) = self.take_frame()? {
            return Ok(Some(msg));
        }
        let limit = PRE_JOIN_MAX_FRAME + 16 + TAG_LEN;
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
        let frame = match &mut self.channel {
            Some(channel) => decode_opened(&self.inbox, PRE_JOIN_MAX_FRAME, &mut channel.opener)?,
            None => decode_frame(&self.inbox, PRE_JOIN_MAX_FRAME)?,
        };
        match frame {
            Some((msg, used)) => {
                self.inbox.drain(..used);
                Ok(Some(msg))
            }
            None => Ok(None),
        }
    }

    pub(super) fn into_conn(mut self) -> io::Result<TcpServerConn> {
        self.flush()?;
        if !self.outbox.is_empty() || !self.inbox.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "handshake traffic out of step at join",
            ));
        }
        let channel = self.channel.ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidData, "join before the key exchange")
        })?;
        self.stream.set_nonblocking(false)?;
        TcpServerConn::spawn(self.stream, channel)
    }
}

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
