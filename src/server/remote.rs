//! Server-side LAN transport: TCP acceptor, pre-join handshake state machine, joined remote
//! connections, and session join/leave paths. Runs on the server thread's loop
//! (`server::handle::server_main`), between the message drain and the pump.
//!
//! Design: an acceptor thread owns the nonblocking listener and hands raw `TcpStream`s over a
//! bounded channel. Full channel just closes the socket. The handshake state machine itself runs
//! inside the server loop (`RemoteHub::pump`), so it can touch sessions/save/world, using
//! thread-free nonblocking sockets ([`admission`]). Pending set is capped globally and per address,
//! each pending connection gets a 10s deadline and is dropped silently past it. Out-of-sequence
//! handshake traffic just drops the connection.
//!
//! A join with a verified identity proof (`net::identity::verify_join`) that clears admission
//! reserves its slot and waits, still thread-free, while its player gets restored on the job pool
//! (`server::admissions`). A later pump picks up the finished restore, spins up its reader/writer
//! threads ([`TcpServerConn`]), and gives it a session. None of this blocks the server loop.
//!
//! Online servers (`account::AccountPolicy`) admit a Petramond account, not just a name: the join's
//! ticket gets redeemed on a per-join worker thread ([`Verifying`]) polled by the loop, and the
//! verified account's stable id decides identity (`joins::account_key`).

use std::io;
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError, TrySendError};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::account::{AccountError, AccountIdentity};
use crate::net::connection::TcpServerConn;
use crate::net::identity::{
    coerce_player_name, new_challenge, validate_player_name, verify_join, PlayerKey,
};
use crate::net::protocol::{
    ClientToServer, JoinCredential, JoinRejectReason, ModEntry, SectionCacheClaim, ServerToClient,
};
use crate::net::secure::{Ephemeral, Exchange, Role};
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

const ACCEPT_POLL: Duration = Duration::from_millis(25);

const ACCEPT_BACKLOG: usize = 64;

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
                            Err(TrySendError::Full(_)) => {}
                            Err(TrySendError::Disconnected(_)) => return,
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
            let _ = join.join();
        }
    }
}

struct RemoteClient {
    id: PlayerId,
    conn: TcpServerConn,
}

enum PendingVerdict {
    Keep,
    Drop,
    Join(JoinRequest),
}

struct Admitting {
    ticket: AdmissionTicket,
    conn: PendingConn,
}

struct Verifying {
    conn: PendingConn,
    verdict: Receiver<Result<AccountIdentity, AccountError>>,
    deadline: Instant,
    view_distance: u8,
    cached_sections: Vec<SectionCacheClaim>,
}

struct JoinRequest {
    binding: crate::net::secure::Binding,
    ticket_server_id: String,
    credential: JoinCredential,
    key: PlayerKey,
    proof: Vec<u8>,
    view_distance: u8,
    cached_sections: Vec<SectionCacheClaim>,
}

#[derive(Default)]
pub struct RemoteHub {
    listener: Option<LanListener>,
    pending: Vec<PendingConn>,
    verifying: Vec<Verifying>,
    admitting: Vec<Admitting>,
    clients: Vec<RemoteClient>,
}

impl RemoteHub {
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

    pub fn shutdown(&mut self) {
        for c in &self.clients {
            c.conn.send(ServerToClient::ServerClosing);
        }
        self.clients.clear();
        self.pending.clear();
        self.verifying.clear();
        self.admitting.clear();
        self.listener = None;
    }

    pub fn pump(
        &mut self,
        server: &mut ServerGame,
        inbound: &mut Vec<(PlayerId, ClientToServer)>,
        local_tx: &Sender<ServerToClient>,
    ) {
        self.accept_new();
        self.drive_pending(server);
        self.drive_verifying(server);
        self.finish_admissions(server, local_tx);
        self.drain_clients(server, inbound, local_tx);
    }

    pub fn send_headroom(&self) -> Vec<(PlayerId, usize)> {
        self.clients
            .iter()
            .map(|c| (c.id, c.conn.queue_headroom()))
            .collect()
    }

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
                .chain(self.verifying.iter().map(|v| &v.conn))
                .chain(self.admitting.iter().map(|a| &a.conn));
            if let Err(why) = admission::admits(joining, peer.ip()) {
                refused.note(peer.ip(), why);
                continue;
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
                PendingVerdict::Drop => drop(self.pending.swap_remove(i)),
                PendingVerdict::Join(request) => {
                    let pending = self.pending.swap_remove(i);
                    match begin_join(pending, request, server) {
                        Some(Joining::Verifying(verifying)) => self.verifying.push(verifying),
                        Some(Joining::Admitting(admitting)) => self.admitting.push(admitting),
                        None => {}
                    }
                }
            }
        }
    }

    fn drive_verifying(&mut self, server: &mut ServerGame) {
        let now = Instant::now();
        let mut i = 0;
        while i < self.verifying.len() {
            let v = &self.verifying[i];
            if now >= v.deadline {
                drop(self.verifying.swap_remove(i));
                continue;
            }
            let verdict = match v.verdict.try_recv() {
                Ok(verdict) => verdict,
                Err(TryRecvError::Empty) => {
                    i += 1;
                    continue;
                }
                Err(TryRecvError::Disconnected) => Err(AccountError::Unreachable(
                    "The server could not check your Petramond account".to_string(),
                )),
            };
            let verifying = self.verifying.swap_remove(i);
            if let Some(admitting) = admit_account(verifying, verdict, server) {
                self.admitting.push(admitting);
            }
        }
    }

    fn finish_admissions(&mut self, server: &mut ServerGame, local_tx: &Sender<ServerToClient>) {
        for admitted in server.take_admitted() {
            let Some(at) = self
                .admitting
                .iter()
                .position(|a| a.ticket == admitted.ticket())
            else {
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
                    continue;
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
            self.broadcast(ServerToClient::PlayerLeft { id: client.id }, local_tx);
        }
    }

    fn broadcast(&self, msg: ServerToClient, local_tx: &Sender<ServerToClient>) {
        for c in &self.clients {
            c.conn.send(msg.clone());
        }
        let _ = local_tx.send(msg);
    }
}

fn step_pending(pending: &mut PendingConn, server: &ServerGame) -> PendingVerdict {
    if pending.expired(Instant::now()) {
        return PendingVerdict::Drop;
    }
    loop {
        let msg = match pending.poll() {
            Ok(Some(msg)) => msg,
            Ok(None) => return PendingVerdict::Keep,
            Err(_) => return PendingVerdict::Drop,
        };
        let sent = match (msg, &pending.stage) {
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
                let Ok(ephemeral) = Ephemeral::generate() else {
                    log::error!("no OS randomness for a key share; refusing connection");
                    return PendingVerdict::Drop;
                };
                let key_share = ephemeral.share();
                pending.stage = Stage::Helloed {
                    challenge,
                    ephemeral,
                };
                pending.send(&ServerToClient::HelloAck {
                    protocol: PROTOCOL_VERSION,
                    challenge,
                    requires_account: server.account_policy.requires_account(),
                    server_id: server.server_id.clone(),
                    key_share,
                })
            }
            (ClientToServer::KeyExchange { key_share }, Stage::Helloed { .. }) => {
                let Stage::Helloed {
                    challenge,
                    ephemeral,
                } = std::mem::replace(&mut pending.stage, Stage::Fresh)
                else {
                    unreachable!("matched Helloed above");
                };
                let server_share = ephemeral.share();
                let exchange = Exchange {
                    server_id: &server.server_id,
                    challenge: &challenge,
                    client_share: &key_share,
                    server_share: &server_share,
                };
                let Ok(session) = ephemeral.agree(Role::Server, &exchange) else {
                    return PendingVerdict::Drop;
                };
                pending.key(session.channel(Role::Server));
                pending.stage = Stage::Keyed {
                    binding: *session.binding(),
                    ticket_server_id: session.ticket_server_id(),
                };
                Ok(())
            }
            (ClientToServer::ModQuery, Stage::Keyed { .. }) => {
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
                    credential,
                    key,
                    proof,
                    view_distance,
                    cached_sections,
                },
                Stage::Keyed {
                    binding,
                    ticket_server_id,
                },
            ) => {
                return PendingVerdict::Join(JoinRequest {
                    binding: *binding,
                    ticket_server_id: ticket_server_id.clone(),
                    credential,
                    key,
                    proof,
                    view_distance,
                    cached_sections,
                });
            }
            (ClientToServer::KeepAlive, _) => Ok(()),
            _ => return PendingVerdict::Drop,
        };
        if sent.is_err() {
            return PendingVerdict::Drop;
        }
    }
}

enum Joining {
    Verifying(Verifying),
    Admitting(Admitting),
}

fn begin_join(
    pending: PendingConn,
    request: JoinRequest,
    server: &mut ServerGame,
) -> Option<Joining> {
    if !verify_join(&request.binding, &request.key, &request.proof) {
        return refuse(pending, JoinRejectReason::BadProof);
    }
    match (server.account_policy.requires_account(), request.credential) {
        (false, JoinCredential::Name(name)) => {
            let verdict = validate_player_name(&name)
                .map_err(|e| JoinRejectReason::InvalidName(e.to_string()))
                .and_then(|name| {
                    server.begin_admission(
                        request.key,
                        &name,
                        request.view_distance as i32,
                        request.cached_sections,
                    )
                });
            match verdict {
                Ok(ticket) => Some(Joining::Admitting(Admitting {
                    ticket,
                    conn: pending,
                })),
                Err(reason) => refuse(pending, reason),
            }
        }
        (false, JoinCredential::Ticket(_)) => refuse(pending, JoinRejectReason::AccountNotAccepted),
        (true, JoinCredential::Name(_)) => refuse(pending, JoinRejectReason::AccountRequired),
        (true, JoinCredential::Ticket(ticket)) => {
            // The ticket was minted for this connection's binding, never the announced id: a
            // ticket a relay obtained on another connection cannot redeem here.
            let server_id = request.ticket_server_id;
            let (tx, verdict) = mpsc::channel();
            if std::thread::Builder::new()
                .name("petramond-verify-join".to_string())
                .spawn(move || {
                    let _ = tx.send(crate::account::verify_join(&ticket, &server_id));
                })
                .is_err()
            {
                return refuse(
                    pending,
                    JoinRejectReason::AccountUnavailable(
                        "The server could not check your Petramond account".to_string(),
                    ),
                );
            }
            Some(Joining::Verifying(Verifying {
                conn: pending,
                verdict,
                deadline: Instant::now() + crate::account::SERVICE_TIMEOUT + Duration::from_secs(2),
                view_distance: request.view_distance,
                cached_sections: request.cached_sections,
            }))
        }
    }
}

fn admit_account(
    verifying: Verifying,
    verdict: Result<AccountIdentity, AccountError>,
    server: &mut ServerGame,
) -> Option<Admitting> {
    let Verifying {
        conn,
        view_distance,
        cached_sections,
        ..
    } = verifying;
    let identity = match verdict {
        Ok(identity) => identity,
        Err(e) if e.clears_sign_in() || matches!(e, AccountError::Refused(_)) => {
            return refuse(
                conn,
                JoinRejectReason::AccountRejected(e.message().to_string()),
            );
        }
        Err(e) => {
            return refuse(
                conn,
                JoinRejectReason::AccountUnavailable(e.message().to_string()),
            );
        }
    };
    log::info!(
        "verified Petramond account '{}' (id {}) for {}",
        identity.username,
        identity.user_id,
        conn.peer()
    );
    let admission = server.begin_admission(
        joins::account_key(identity.user_id),
        &coerce_player_name(&identity.username),
        view_distance as i32,
        cached_sections,
    );
    match admission {
        Ok(ticket) => Some(Admitting { ticket, conn }),
        Err(JoinRejectReason::AlreadyConnected) => {
            refuse(conn, JoinRejectReason::AccountAlreadyOnline)
        }
        Err(reason) => refuse(conn, reason),
    }
}

fn refuse<T>(mut pending: PendingConn, reason: JoinRejectReason) -> Option<T> {
    log::info!("refused join from {}: {reason:?}", pending.peer());
    let _ = pending.send(&ServerToClient::JoinReject { reason });
    None
}
