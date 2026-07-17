//! Server-side LAN transport: the TCP acceptor, the
//! pre-join handshake state machine, joined remote connections, and the
//! session join/leave paths. Driven by the server thread's loop
//! (`server::handle::server_main`) between its message drain and the pump.
//!
//! Design: an ACCEPTOR thread owns the (nonblocking) listener and hands raw
//! `TcpStream`s over a channel; every handed-off socket immediately gets its
//! reader/writer threads ([`TcpServerConn`]), but the handshake state machine
//! itself runs IN the server loop (`RemoteHub::pump`) where it can reach the
//! sessions/save/world. Pre-join connections have a 10 s deadline (dropped
//! silently); out-of-sequence handshake traffic drops the connection.

use std::io;
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::net::connection::TcpServerConn;
use crate::net::protocol::{ClientToServer, ModEntry, ServerToClient};
use crate::net::PROTOCOL_VERSION;
use crate::server::player::PlayerId;

use super::game::ServerGame;

mod joins;
#[cfg(test)]
mod tests;

/// A connection that hasn't completed Hello→Mods→Join within this window is
/// dropped silently.
const PRE_JOIN_DEADLINE: Duration = Duration::from_secs(10);

/// The acceptor thread's poll interval: the listener is NONBLOCKING and the
/// thread sleeps this long between accept attempts, so dropping the listener
/// only needs a stop flag — no self-connect trick, no mid-accept fd race.
const ACCEPT_POLL: Duration = Duration::from_millis(25);

/// The bound LAN listener + its acceptor thread.
struct LanListener {
    port: u16,
    handoff: Receiver<TcpStream>,
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
        let (tx, handoff) = mpsc::channel();
        let join = std::thread::Builder::new()
            .name("petramond-accept".to_string())
            .spawn(move || {
                while !flag.load(Ordering::SeqCst) {
                    match listener.accept() {
                        Ok((stream, _)) => {
                            if tx.send(stream).is_err() {
                                return; // hub gone
                            }
                        }
                        Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                            std::thread::sleep(ACCEPT_POLL);
                        }
                        Err(e) => {
                            log::warn!("LAN accept error: {e}");
                            std::thread::sleep(ACCEPT_POLL);
                        }
                    }
                }
            })
            .expect("spawn LAN acceptor thread");
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

/// A connection still inside the join handshake.
struct PendingConn {
    conn: TcpServerConn,
    /// `Hello` exchanged: `ModQuery`/`Join` are acceptable now.
    helloed: bool,
    deadline: Instant,
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
    Joined { id: PlayerId, name: String },
}

/// Everything the server thread owns about remote transport.
#[derive(Default)]
pub(crate) struct RemoteHub {
    listener: Option<LanListener>,
    pending: Vec<PendingConn>,
    clients: Vec<RemoteClient>,
}

impl RemoteHub {
    /// Bind the LAN listener ("Open to LAN"): port 0 = ephemeral. Idempotent —
    /// already open just reports the bound port.
    pub(crate) fn open_to_lan(&mut self, port: u16) -> io::Result<u16> {
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
    pub(crate) fn shutdown(&mut self) {
        for c in &self.clients {
            c.conn.send(ServerToClient::ServerClosing);
        }
        self.clients.clear();
        self.pending.clear();
        self.listener = None;
    }

    /// One server-loop step: accept handed-off sockets, drive the pre-join
    /// handshakes, then process leaves and drain the joined connections'
    /// messages into `inbound` (tagged by `PlayerId`; the pump resolves the
    /// tags against the post-leave session list).
    pub(crate) fn pump(
        &mut self,
        server: &mut ServerGame,
        inbound: &mut Vec<(PlayerId, ClientToServer)>,
        local_tx: &Sender<ServerToClient>,
    ) {
        self.accept_new();
        self.drive_pending(server, local_tx);
        self.drain_clients(server, inbound, local_tx);
    }

    /// Every joined client's outbound-queue headroom, snapshot before the
    /// pump so the terrain streamer can pace each session's plan to what its
    /// connection is actually draining (`ServerGame::pump_streaming`).
    pub(crate) fn send_headroom(&self) -> Vec<(PlayerId, usize)> {
        self.clients
            .iter()
            .map(|c| (c.id, c.conn.queue_headroom()))
            .collect()
    }

    /// Route each remote recipient's pump output through its connection
    /// queue. A refused send (dead reader/writer, or a slow client's FULL
    /// queue) marks the connection dead; the next pump runs its leave path.
    pub(crate) fn route(&mut self, remote: Vec<(PlayerId, Vec<ServerToClient>)>) {
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

    fn accept_new(&mut self) {
        let Some(listener) = &self.listener else {
            return;
        };
        while let Ok(stream) = listener.handoff.try_recv() {
            match TcpServerConn::spawn(stream) {
                Ok(conn) => {
                    log::info!("LAN connection from {}", conn.peer());
                    self.pending.push(PendingConn {
                        conn,
                        helloed: false,
                        deadline: Instant::now() + PRE_JOIN_DEADLINE,
                    });
                }
                Err(e) => log::warn!("LAN connection setup failed: {e}"),
            }
        }
    }

    fn drive_pending(&mut self, server: &mut ServerGame, local_tx: &Sender<ServerToClient>) {
        let mut i = 0;
        while i < self.pending.len() {
            match step_pending(&mut self.pending[i], server) {
                PendingVerdict::Keep => i += 1,
                PendingVerdict::Drop => {
                    // Dropping the conn flushes any farewell frame
                    // (HelloReject/JoinReject) through its writer.
                    self.pending.remove(i);
                }
                PendingVerdict::Joined { id, name } => {
                    let pending = self.pending.remove(i);
                    log::info!("player '{name}' joined as id {}", id.0);
                    server.enqueue_join_chat(&name);
                    self.broadcast(ServerToClient::PlayerJoined { id, name }, local_tx);
                    self.clients.push(RemoteClient {
                        id,
                        conn: pending.conn,
                    });
                }
            }
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
                server.enqueue_leave_chat(name);
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
fn step_pending(pending: &mut PendingConn, server: &mut ServerGame) -> PendingVerdict {
    if pending.conn.is_dead() || Instant::now() >= pending.deadline {
        return PendingVerdict::Drop; // silent: it never joined
    }
    while let Some(msg) = pending.conn.try_recv() {
        match msg {
            ClientToServer::Hello { protocol } if !pending.helloed => {
                if protocol != PROTOCOL_VERSION {
                    pending.conn.send(ServerToClient::HelloReject {
                        server_protocol: PROTOCOL_VERSION,
                    });
                    return PendingVerdict::Drop;
                }
                pending.helloed = true;
                pending.conn.send(ServerToClient::HelloAck {
                    protocol: PROTOCOL_VERSION,
                });
            }
            ClientToServer::ModQuery if pending.helloed => {
                let mods = crate::modding::modset::active(server.world.disabled_mods())
                    .into_iter()
                    .map(|m| ModEntry {
                        id: m.id,
                        version: m.version,
                    })
                    .collect();
                pending.conn.send(ServerToClient::ModList { mods });
            }
            ClientToServer::Join {
                player_name,
                view_distance,
                cached_sections,
            } if pending.helloed => {
                // Never rejected: a taken name is auto-deduped with a numeric
                // suffix (the returned name is the session's — it keys the
                // broadcast and the per-name save file).
                let (data, name) = server.admit_remote_player(
                    &player_name,
                    view_distance as i32,
                    &cached_sections,
                );
                let id = data.player_id;
                pending.conn.send(ServerToClient::JoinAccept(data));
                return PendingVerdict::Joined { id, name };
            }
            ClientToServer::KeepAlive => {}
            // Out-of-sequence handshake traffic (a pre-Hello Join/ModQuery, a
            // repeated Hello, gameplay before joining) drops the connection.
            _ => return PendingVerdict::Drop,
        }
    }
    PendingVerdict::Keep
}
