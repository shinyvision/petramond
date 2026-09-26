//! Server-side LAN transport: the TCP acceptor, the
//! pre-join handshake state machine, joined remote connections, and the
//! session join/leave paths. Driven by the server thread's loop
//! (`server::handle::server_main`) between its message drain and the pump.
//!
//! Design: an ACCEPTOR thread owns the (nonblocking) listener and hands raw
//! `TcpStream`s over a BOUNDED channel (a full channel closes the socket on
//! the spot). The handshake state machine runs IN the server loop
//! (`RemoteHub::pump`) where it can reach the sessions/save/world, over
//! thread-free nonblocking sockets ([`admission`]) — the pending set is
//! capped globally and per address, and each pending connection has a 10 s
//! deadline (dropped silently). Out-of-sequence handshake traffic drops the
//! connection. A join whose identity proof verifies
//! (`net::identity::verify_join`) and that passes the admission check
//! reserves its slot and waits, still thread-free, while its player is
//! restored on the job pool (`server::admissions`); at a later pump the
//! finished restore gets its reader/writer threads ([`TcpServerConn`]) and
//! its session. Nothing in a join blocks the server loop.

use std::io;
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, TrySendError};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::net::connection::TcpServerConn;
use crate::net::identity::{new_challenge, validate_player_name, verify_join, PlayerKey};
use crate::net::protocol::{
    ClientToServer, JoinRejectReason, ModEntry, SectionCacheClaim, ServerToClient,
};
use crate::net::PROTOCOL_VERSION;
use crate::player::PlayerId;

use super::admissions::AdmissionTicket;
use super::game::ServerGame;
use admission::{PendingConn, Refusals, Stage};

mod admission;
mod joins;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_admission;

/// The acceptor thread's poll interval: the listener is NONBLOCKING and the
/// thread sleeps this long between accept attempts, so dropping the listener
/// only needs a stop flag — no self-connect trick, no mid-accept fd race.
const ACCEPT_POLL: Duration = Duration::from_millis(25);

/// Accepted sockets waiting for the server loop. Beyond this the acceptor
/// closes new sockets itself, so a flood never piles up file descriptors
/// behind a slow server tick.
const ACCEPT_BACKLOG: usize = 64;

/// The bound LAN listener + its acceptor thread.
struct LanListener {
    port: u16,
    handoff: Receiver<(TcpStream, SocketAddr)>,
    stop: Arc<AtomicBool>,
    join: Option<std::thread::JoinHandle<()>>,
}

impl LanListener {
    fn bind(port: u16) -> io::Result<LanListener> {
        let listener = TcpListener::bind(("0.0.0.0", port))?;
        let port = listener.local_addr()?.port();
        listener.set_nonblocking(true)?;
        let stop = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&stop);
        let (tx, handoff) = mpsc::sync_channel(ACCEPT_BACKLOG);
        let join = std::thread::Builder::new()
            .name("petramond-accept".to_string())
            .spawn(move || {
                while !flag.load(Ordering::SeqCst) {
                    match listener.accept() {
                        Ok(accepted) => match tx.try_send(accepted) {
                            Ok(()) => {}
                            // Dropping the socket closes it.
                            Err(TrySendError::Full(_)) => {}
                            Err(TrySendError::Disconnected(_)) => return, // hub gone
                        },
                        Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                            std::thread::sleep(ACCEPT_POLL);
                        }
                        Err(e) => {
                            log::warn!("LAN accept error: {e}");
                            std::thread::sleep(ACCEPT_POLL);
                        }
                    }
                }
            })?;
        Ok(LanListener {
            port,
            handoff,
            stop,
            join: Some(join),
        })
    }
}

impl Drop for LanListener {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(join) = self.join.take() {
            let _ = join.join(); // exits within one ACCEPT_POLL
        }
    }
}

/// A joined remote client: its session is `sessions[i].id == id` (looked up
/// per drain — session INDICES shift on `swap_remove`, `PlayerId`s never do).
struct RemoteClient {
    id: PlayerId,
    conn: TcpServerConn,
}

enum PendingVerdict {
    Keep,
    Drop,
    Join(JoinRequest),
}

/// A verified join waiting for its player restore (`server::admissions`).
/// Its socket stays thread-free and unpolled: the client waits for the
/// `JoinAccept`.
struct Admitting {
    ticket: AdmissionTicket,
    conn: PendingConn,
}

/// A `Join` frame, with the challenge it must prove itself against.
struct JoinRequest {
    challenge: crate::net::identity::JoinChallenge,
    player_name: String,
    key: PlayerKey,
    proof: Vec<u8>,
    view_distance: u8,
    cached_sections: Vec<SectionCacheClaim>,
}

/// Everything the server thread owns about remote transport.
#[derive(Default)]
pub struct RemoteHub {
    listener: Option<LanListener>,
    pending: Vec<PendingConn>,
    admitting: Vec<Admitting>,
    clients: Vec<RemoteClient>,
}

impl RemoteHub {
    /// Bind the LAN listener ("Open to LAN"): port 0 = ephemeral. Idempotent —
    /// already open just reports the bound port.
    pub fn open_to_lan(&mut self, port: u16) -> io::Result<u16> {
        if let Some(l) = &self.listener {
            return Ok(l.port);
        }
        let listener = LanListener::bind(port)?;
        let port = listener.port;
        log::info!("open to LAN on port {port}");
        self.listener = Some(listener);
        Ok(port)
    }

    /// Server shutdown: farewell every joined client with `ServerClosing`,
    /// then drop all transport state (each writer drains + flushes the
    /// farewell before its socket closes).
    pub fn shutdown(&mut self) {
        for c in &self.clients {
            c.conn.send(ServerToClient::ServerClosing);
        }
        self.clients.clear();
        self.pending.clear();
        self.admitting.clear();
        self.listener = None;
    }

    /// One server-loop step: accept handed-off sockets, drive the pre-join
    /// handshakes, promote the joins whose restore finished, then process
    /// leaves and drain the joined connections' messages into `inbound`
    /// (tagged by `PlayerId`; the pump resolves the tags against the
    /// post-leave session list).
    pub fn pump(
        &mut self,
        server: &mut ServerGame,
        inbound: &mut Vec<(PlayerId, ClientToServer)>,
        local_tx: &Sender<ServerToClient>,
    ) {
        self.accept_new();
        self.drive_pending(server);
        self.finish_admissions(server, local_tx);
        self.drain_clients(server, inbound, local_tx);
    }

    /// Every joined client's outbound-queue headroom, snapshot before the
    /// pump so the terrain streamer can pace each session's plan to what its
    /// connection is actually draining (`ServerGame::pump_streaming`).
    pub fn send_headroom(&self) -> Vec<(PlayerId, usize)> {
        self.clients
            .iter()
            .map(|c| (c.id, c.conn.queue_headroom()))
            .collect()
    }

    /// Route each remote recipient's pump output through its connection
    /// queue. A refused send (dead reader/writer, or a slow client's FULL
    /// queue) marks the connection dead; the next pump runs its leave path.
    pub fn route(&mut self, remote: Vec<(PlayerId, Vec<ServerToClient>)>) {
        for (id, msgs) in remote {
            let Some(c) = self.clients.iter().find(|c| c.id == id) else {
                continue;
            };
            for msg in msgs {
                if !c.conn.send(msg) {
                    break;
                }
            }
        }
    }

    /// Disconnect the sessions the pump evicted after a fault (their leave
    /// path already ran server-side): tell each why, drop its connection,
    /// and announce the leave to everyone else.
    pub fn kick(
        &mut self,
        kicked: Vec<(PlayerId, Option<String>)>,
        local_tx: &Sender<ServerToClient>,
    ) {
        for (id, name) in kicked {
            let Some(at) = self.clients.iter().position(|c| c.id == id) else {
                continue;
            };
            let client = self.clients.remove(at);
            client.conn.send(ServerToClient::Disconnect {
                reason: "Kicked: the server hit an error handling your session".to_string(),
            });
            log::info!(
                "player '{}' (id {}) kicked after a fault",
                name.as_deref().unwrap_or("?"),
                id.0
            );
            self.broadcast(ServerToClient::PlayerLeft { id }, local_tx);
        }
    }

    fn accept_new(&mut self) {
        let Some(listener) = &self.listener else {
            return;
        };
        let mut refused = Refusals::default();
        while let Ok((stream, peer)) = listener.handoff.try_recv() {
            let joining = self
                .pending
                .iter()
                .chain(self.admitting.iter().map(|a| &a.conn));
            if let Err(why) = admission::admits(joining, peer.ip()) {
                refused.note(peer.ip(), why);
                continue; // dropping the socket closes it
            }
            match PendingConn::new(stream, peer) {
                Ok(pending) => {
                    log::info!("LAN connection from {peer}");
                    self.pending.push(pending);
                }
                Err(e) => log::warn!("LAN connection setup failed for {peer}: {e}"),
            }
        }
        refused.log();
    }

    fn drive_pending(&mut self, server: &mut ServerGame) {
        let mut i = 0;
        while i < self.pending.len() {
            match step_pending(&mut self.pending[i], server) {
                PendingVerdict::Keep => i += 1,
                // Dropping the socket closes it; any farewell frame
                // (HelloReject) was already written.
                PendingVerdict::Drop => drop(self.pending.swap_remove(i)),
                PendingVerdict::Join(request) => {
                    let pending = self.pending.swap_remove(i);
                    if let Some(admitting) = begin_admission(pending, request, server) {
                        self.admitting.push(admitting);
                    }
                }
            }
        }
    }

    /// Promote every join whose player restore finished: spawn its
    /// connection's I/O threads, then make it a session and announce it. A
    /// connection that cannot be promoted releases its reservation instead.
    fn finish_admissions(&mut self, server: &mut ServerGame, local_tx: &Sender<ServerToClient>) {
        for admitted in server.take_admitted() {
            let Some(at) = self
                .admitting
                .iter()
                .position(|a| a.ticket == admitted.ticket())
            else {
                // Its connection is gone (a shutdown cleared it).
                server.abandon_admission(admitted);
                continue;
            };
            let Admitting { conn, .. } = self.admitting.swap_remove(at);
            let peer = conn.peer();
            let conn = match conn.into_conn() {
                Ok(conn) => conn,
                Err(e) => {
                    log::warn!("LAN connection setup failed for {peer}: {e}");
                    server.abandon_admission(admitted);
                    continue;
                }
            };
            let Some((data, name)) = server.finish_admission(admitted) else {
                // The restore failed (logged where it happened); dropping the
                // connection closes it.
                continue;
            };
            let id = data.player_id;
            conn.send(ServerToClient::JoinAccept(data));
            log::info!("player '{name}' joined as id {}", id.0);
            server.chat.joined(&name);
            self.broadcast(ServerToClient::PlayerJoined { id, name }, local_tx);
            self.clients.push(RemoteClient { id, conn });
        }
    }

    /// Drain joined connections, splitting leavers (dead transport or a
    /// `Disconnect` message) from gameplay traffic, then run each leaver's
    /// leave path and announce it. Messages a leaver sent this frame leave
    /// with it.
    fn drain_clients(
        &mut self,
        server: &mut ServerGame,
        inbound: &mut Vec<(PlayerId, ClientToServer)>,
        local_tx: &Sender<ServerToClient>,
    ) {
        let mut leavers: Vec<usize> = Vec::new();
        for (i, c) in self.clients.iter().enumerate() {
            let mut leaving = c.conn.is_dead();
            while let Some(msg) = c.conn.try_recv() {
                if leaving {
                    continue; // drain and drop the residue
                }
                if matches!(msg, ClientToServer::Disconnect) {
                    leaving = true;
                } else {
                    inbound.push((c.id, msg));
                }
            }
            if leaving {
                leavers.push(i);
            }
        }
        for i in leavers.into_iter().rev() {
            let client = self.clients.remove(i);
            let name = server.remove_remote_session(client.id);
            log::info!(
                "player '{}' (id {}) left",
                name.as_deref().unwrap_or("?"),
                client.id.0
            );
            if let Some(name) = &name {
                server.chat.left(name);
            }
            // Their queued-but-unrouted messages in `inbound` die at the
            // pump's id→index resolution (the session is gone).
            self.broadcast(ServerToClient::PlayerLeft { id: client.id }, local_tx);
        }
    }

    /// Send to every JOINED remote and the local pipe. A joiner is excluded
    /// naturally: it is not in `clients` while its own join broadcasts.
    fn broadcast(&self, msg: ServerToClient, local_tx: &Sender<ServerToClient>) {
        for c in &self.clients {
            c.conn.send(msg.clone());
        }
        let _ = local_tx.send(msg);
    }
}

/// Advance one pending connection's handshake with whatever frames arrived.
/// Runs in the server loop — it can reach the sessions, save, and world.
fn step_pending(pending: &mut PendingConn, server: &ServerGame) -> PendingVerdict {
    if pending.expired(Instant::now()) {
        return PendingVerdict::Drop; // silent: it never joined
    }
    loop {
        let msg = match pending.poll() {
            Ok(Some(msg)) => msg,
            Ok(None) => return PendingVerdict::Keep,
            Err(_) => return PendingVerdict::Drop,
        };
        let sent = match (msg, pending.stage) {
            (ClientToServer::Hello { protocol }, Stage::Fresh) => {
                if protocol != PROTOCOL_VERSION {
                    let _ = pending.send(&ServerToClient::HelloReject {
                        server_protocol: PROTOCOL_VERSION,
                    });
                    return PendingVerdict::Drop;
                }
                let Some(challenge) = new_challenge() else {
                    log::error!("no OS randomness for a join challenge; refusing connection");
                    return PendingVerdict::Drop;
                };
                pending.stage = Stage::Helloed { challenge };
                pending.send(&ServerToClient::HelloAck {
                    protocol: PROTOCOL_VERSION,
                    challenge,
                })
            }
            (ClientToServer::ModQuery, Stage::Helloed { .. }) => {
                let mods = crate::modding::modset::active(server.world.data().disabled_mods())
                    .into_iter()
                    .map(|m| ModEntry {
                        id: m.id,
                        version: m.version,
                    })
                    .collect();
                pending.send(&ServerToClient::ModList { mods })
            }
            (
                ClientToServer::Join {
                    player_name,
                    key,
                    proof,
                    view_distance,
                    cached_sections,
                },
                Stage::Helloed { challenge },
            ) => {
                return PendingVerdict::Join(JoinRequest {
                    challenge,
                    player_name,
                    key,
                    proof,
                    view_distance,
                    cached_sections,
                });
            }
            (ClientToServer::KeepAlive, _) => Ok(()),
            // Out-of-sequence handshake traffic (a pre-Hello Join/ModQuery, a
            // repeated Hello, gameplay before joining) drops the connection.
            _ => return PendingVerdict::Drop,
        };
        if sent.is_err() {
            return PendingVerdict::Drop;
        }
    }
}

/// Settle a `Join`'s verdict: verify the identity proof, validate the
/// display name, and reserve the admission. Every refusal is a `JoinReject`
/// over the thread-free socket; an accepted join waits for its restore.
fn begin_admission(
    mut pending: PendingConn,
    request: JoinRequest,
    server: &mut ServerGame,
) -> Option<Admitting> {
    let peer = pending.peer();
    let verdict = if !verify_join(&request.challenge, &request.key, &request.proof) {
        Err(JoinRejectReason::BadProof)
    } else {
        validate_player_name(&request.player_name)
            .map_err(|e| JoinRejectReason::InvalidName(e.to_string()))
            .and_then(|name| {
                server.begin_admission(
                    request.key,
                    &name,
                    request.view_distance as i32,
                    request.cached_sections,
                )
            })
    };
    match verdict {
        Ok(ticket) => Some(Admitting {
            ticket,
            conn: pending,
        }),
        Err(reason) => {
            log::info!("refused join from {peer}: {reason:?}");
            let _ = pending.send(&ServerToClient::JoinReject { reason });
            None
        }
    }
}
