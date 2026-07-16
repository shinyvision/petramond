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

use crate::game::tick::TickEvents;
use crate::net::connection::TcpServerConn;
use crate::net::protocol::{
    ClientToServer, ItemSlotWire, JoinData, ModEntry, SelfRestore, ServerToClient,
};
use crate::net::PROTOCOL_VERSION;
use crate::server::player::{ConnectedPlayer, PlayerId};

use super::game::{wire_world_events, ServerGame};

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

impl ServerGame {
    /// Admit `requested` as a new remote session and return the `JoinAccept`
    /// payload plus the session's FINAL name. A join is never refused for its
    /// name: if `requested` is taken (case-insensitive, vs connected
    /// sessions), the lowest free numeric suffix is appended ("Rachel" →
    /// "Rachel2" → "Rachel3") and the suffixed name IS the session's name —
    /// it keys `players/<name>.dat`, so a deduped guest saves AND restores as
    /// the suffixed name (a returning "Rachel" only sees "Rachel2"'s state
    /// while the original "Rachel" is still connected to claim the base
    /// name). Restore from `players/<name>.dat` when the world has a save,
    /// else a fresh surface spawn — exactly the local session's restore path.
    pub(crate) fn admit_remote_player(
        &mut self,
        requested: &str,
        view_distance: i32,
        cached_sections: &[crate::net::protocol::SectionCacheClaim],
    ) -> (Box<JoinData>, String) {
        let name = self.dedupe_player_name(requested);
        let id = self.next_free_player_id();
        let player = self
            .world
            .save()
            .and_then(|save| save.load_player(&name))
            .and_then(|bytes| crate::save::player::decode(&bytes))
            .map(|data| data.restore())
            .unwrap_or_else(|| crate::game::session::spawn_player(self.world.seed));
        let data = Box::new(JoinData {
            player_id: id,
            seed: self.world.seed,
            clock: super::daynight::current_clock(&self.world),
            tables: crate::net::remap::local_name_tables(),
            self_restore: self_restore_from(&player),
            crafting_recipes: self.recipes.crafting().to_data(),
            players: self
                .sessions
                .iter()
                .map(|s| (s.id, s.name.clone()))
                .collect(),
        });
        let mut session = ConnectedPlayer::new(id, name.clone(), player, view_distance);
        session.terrain.seed_client_cache(cached_sections);
        self.sessions.push(session);
        (data, name)
    }

    /// The requested name, or — when a connected session already uses it
    /// (case-insensitive) — the requested name with the lowest free numeric
    /// suffix appended (`{name}2`, `{name}3`, …).
    fn dedupe_player_name(&self, requested: &str) -> String {
        let taken = |candidate: &str| {
            self.sessions
                .iter()
                .any(|s| s.name.eq_ignore_ascii_case(candidate))
        };
        if !taken(requested) {
            return requested.to_string();
        }
        (2u32..)
            .map(|n| format!("{requested}{n}"))
            .find(|candidate| !taken(candidate))
            .expect("fewer than u32::MAX sessions")
    }

    /// The smallest `PlayerId` no connected session uses (freed ids recycle).
    fn next_free_player_id(&self) -> PlayerId {
        (0u16..=u8::MAX as u16)
            .map(|i| PlayerId(i as u8))
            .find(|id| !self.sessions.iter().any(|s| s.id == *id))
            .expect("fewer than 256 sessions")
    }

    /// The leave path, in order: close the open menu (cursor/craft returns,
    /// chest-viewer release), flush the drop queue into the world (overflow
    /// from those returns must not vanish), prepare a safe detached snapshot,
    /// detach riding state, persist that snapshot, remove the session, and bank
    /// the close's world events for the next tick batch. Returns the leaver's
    /// name (`None` = no such session).
    ///
    /// `swap_remove` keeps a LISTEN server's local session at index 0 (only
    /// `s >= 1` is ever removed there; a headless server may remove any
    /// session, down to an empty list) and every survivor's `PlayerId` rides
    /// with its element; nothing stores session INDICES across loop
    /// iterations — the hub re-resolves ids at every drain, and the pump
    /// resolves its tagged inbox against the post-leave list.
    pub(crate) fn remove_remote_session(&mut self, id: PlayerId) -> Option<String> {
        let s = self.sessions.iter().position(|x| x.id == id)?;
        if s == 0 && self.has_local_session {
            debug_assert!(false, "the local session never leaves");
            return None;
        }
        let mut events = TickEvents::with_next_spatial_sound_handle(self.next_mod_sound_handle);
        self.close_open_menu_for(s, &mut events);
        self.tick_drops(s, &mut events);
        self.next_mod_sound_handle = events.next_spatial_sound_handle();
        self.pending_wire_events
            .extend(wire_world_events(&mut events.world));
        let obstacles = self.world.mobs().solid_obstacles();
        let snapshot = self.player_snapshot_for_save(s, &obstacles);
        self.detach_departing_session(s);
        if let Some(save) = self.world.save() {
            if let Some(snapshot) = snapshot {
                save.save_player(
                    &self.sessions[s].name,
                    crate::save::player::encode(&snapshot),
                );
            } else {
                log::debug!(
                    "deferring final player save for '{}': no stream-final detached riding position",
                    self.sessions[s].name
                );
            }
        }
        Some(self.sessions.swap_remove(s).name)
    }
}

/// The joining player's `SelfRestore`, mirrored off the restored session
/// player (wire ids are raw server ids; effects travel by name).
fn self_restore_from(player: &crate::player::Player) -> SelfRestore {
    SelfRestore {
        transform: crate::net::protocol::Transform {
            pos: player.pos,
            vel: player.vel,
            yaw: player.yaw,
            pitch: player.pitch,
        },
        mode: match player.mode() {
            crate::player::PlayerMode::Survival => 0,
            crate::player::PlayerMode::Spectator => 1,
        },
        health: player.health(),
        bed_spawn: player.bed_spawn.map(|b| (b.bed, b.spot)),
        effects: player
            .effects()
            .iter()
            .map(|e| (e.effect.def().name.to_string(), e.remaining))
            .collect(),
        inventory: player
            .inventory
            .raw_slots()
            .iter()
            .copied()
            .chain(std::iter::once(player.inventory.cursor().copied()))
            .map(|slot| {
                slot.map(|st| ItemSlotWire {
                    item_id: st.item.0,
                    count: st.count,
                })
            })
            .collect(),
        active_slot: player.inventory.active_slot(),
        craft_craftable_only: player.craft_craftable_only,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block::Block;
    use crate::chunk::{section_idx, SectionPos};
    use crate::item::{ItemStack, ItemType};
    use crate::mathh::{IVec3, Vec3};
    use crate::net::connection::TcpClientConn;
    use crate::net::framing::{read_msg, write_msg};
    use crate::net::handshake::{client_handshake, installed_mod_ids};
    use crate::net::protocol::{PlayerAction, PlayerUpdate, TargetRef};
    use crate::net::remap::IdRemap;
    use crate::server::handle::ServerHandle;
    use std::collections::HashMap;
    use std::net::TcpStream;
    use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};

    fn connect(port: u16) -> TcpStream {
        let stream = TcpStream::connect(("127.0.0.1", port)).expect("connect to loopback");
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .expect("read timeout");
        stream
    }

    /// Drain `handle` until `f` yields, sleeping between polls; None =
    /// timeout. Acks every streaming batch like a live client so the
    /// server's flow-control window keeps streaming.
    fn drain_until<T>(
        handle: &mut ServerHandle,
        timeout: Duration,
        mut f: impl FnMut(ServerToClient) -> Option<T>,
    ) -> Option<T> {
        let deadline = Instant::now() + timeout;
        let mut msgs = Vec::new();
        while Instant::now() < deadline {
            handle.drain(&mut msgs);
            for msg in msgs.drain(..) {
                if matches!(msg, ServerToClient::StreamBatchEnd { .. }) {
                    let _ = handle.send(ClientToServer::StreamBatchAck {
                        messages_per_second: 1e9, // server clamps
                    });
                }
                if let Some(hit) = f(msg) {
                    return Some(hit);
                }
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        None
    }

    /// A surface cell we can build on, from the terrain the client RECEIVED:
    /// a bare ground block with two air (dry) cells above, near `(cx, cz)` =
    /// (8, 8) world-horizontal. Scanning what actually arrived keeps the test
    /// independent of the seed's exact terrain shape.
    /// A ground cell (two dry-air cells above) the player standing at `pos`
    /// can LEGITIMATELY build against: within reach of its eye — the server
    /// bounds the reach eye by the F1 drift ring, so a hover claim far from
    /// the session's own position no longer grants reach — and far enough
    /// sideways that the placed block cannot overlap the placer's body.
    fn find_place_spot_within_reach(
        sections: &HashMap<SectionPos, (Vec<u8>, Option<Vec<u8>>)>,
        pos: Vec3,
    ) -> Option<IVec3> {
        let eye = pos + Vec3::new(0.0, 1.62, 0.0);
        let ground = [Block::Grass.0, Block::Dirt.0, Block::Stone.0, Block::Sand.0];
        for (sp, (blocks, water)) in sections {
            let dry_air = |x: usize, y: usize, z: usize| {
                let i = section_idx(x, y, z);
                blocks[i] == Block::Air.0 && water.as_ref().is_none_or(|w| w[i] == 0)
            };
            for y in 0..14 {
                for z in 0..16 {
                    for x in 0..16 {
                        let cell = IVec3::new(
                            sp.cx * 16 + x as i32,
                            sp.cy * 16 + y as i32,
                            sp.cz * 16 + z as i32,
                        );
                        let dx = (cell.x as f32 + 0.5 - pos.x).abs();
                        let dz = (cell.z as f32 + 0.5 - pos.z).abs();
                        if dx.max(dz) < 1.2 {
                            continue; // the placed cell would overlap the placer
                        }
                        let lo = Vec3::new(cell.x as f32, cell.y as f32, cell.z as f32);
                        let closest = eye.clamp(lo, lo + Vec3::ONE);
                        if (closest - eye).length() > 3.5 {
                            continue; // out of legitimate reach
                        }
                        if ground.contains(&blocks[section_idx(x, y, z)])
                            && dry_air(x, y + 1, z)
                            && dry_air(x, y + 2, z)
                        {
                            return Some(cell);
                        }
                    }
                }
            }
        }
        None
    }

    /// Duplicate names are never rejected: admission appends the lowest free
    /// numeric suffix (case-insensitive vs every connected session), and the
    /// suffixed name IS the session name (it keys the per-name save file).
    #[test]
    fn duplicate_join_names_dedupe_with_the_lowest_free_numeric_suffix() {
        let (mut server, _) = crate::game::session::build_session("", 3, 2);
        // The local session's name resolves from the REAL environment
        // (client.json / $USER); pin it so an ambient "Rachel"-ish name
        // can't occupy a suffix the assertions below count on.
        server.sessions[0].name = "Host".to_string();
        let (_, first) = server.admit_remote_player("Rachel", 32, &[]);
        assert_eq!(first, "Rachel");
        let (_, second) = server.admit_remote_player("rachel", 32, &[]);
        assert_eq!(
            second, "rachel2",
            "case-insensitive dedupe, suffix appended"
        );
        let (_, third) = server.admit_remote_player("RACHEL", 32, &[]);
        assert_eq!(third, "RACHEL3", "the lowest FREE suffix (2 is taken)");
        let names: Vec<&str> = server.sessions.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"Rachel") && names.contains(&"rachel2"));
    }

    #[test]
    fn headless_disconnect_detaches_before_player_id_reuse() {
        let mut server = crate::game::session::build_headless_session("", 3, 2);
        let dismounts = Arc::new(AtomicUsize::new(0));
        let observed = Arc::clone(&dismounts);
        server.bus.on_post(
            crate::events::PostEventKind::PlayerDismounted,
            0,
            move |_, event| {
                if matches!(
                    event,
                    crate::events::PostEvent::PlayerDismounted {
                        player: PlayerId(0),
                        mob_id: 77
                    }
                ) {
                    observed.fetch_add(1, AtomicOrdering::SeqCst);
                }
            },
        );

        let (first, _) = server.admit_remote_player("First", 16, &[]);
        assert_eq!(first.player_id, PlayerId(0));
        assert!(server.world.riding_mut().mount(0, 77, 0));
        server.sessions[0].mount = server.world.riding().mount_of(0);

        assert_eq!(
            server.remove_remote_session(PlayerId(0)).as_deref(),
            Some("First")
        );
        assert!(server.sessions.is_empty());
        assert_eq!(server.world.riding().mount_of(0), None);

        let (second, _) = server.admit_remote_player("Second", 16, &[]);
        assert_eq!(second.player_id, PlayerId(0), "the freed id recycles");
        assert_eq!(server.world.riding().mount_of(0), None);
        assert_eq!(server.sessions[0].mount, None);

        server.pump_tagged(crate::game::tick::TICK_DT * 1.01, &mut Vec::new(), &[]);
        assert_eq!(dismounts.load(AtomicOrdering::SeqCst), 1);
        server.pump_tagged(crate::game::tick::TICK_DT * 1.01, &mut Vec::new(), &[]);
        assert_eq!(
            dismounts.load(AtomicOrdering::SeqCst),
            1,
            "one detach transition emits exactly once"
        );
    }

    /// The full remote-join loop over real TCP on 127.0.0.1: open to LAN on an
    /// ephemeral port, handshake + join a remote client (restored from a
    /// pre-seeded player file), stream it terrain, place a block from the
    /// remote side and see the delta come back, dedupe a duplicate name,
    /// ignore Pause while remote players exist, and broadcast joins/leaves.
    #[test]
    fn full_lan_join_place_pause_gate_and_leave() {
        let dir = std::env::temp_dir().join(format!("petramond-lan-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("players")).expect("temp players dir");
        // Pre-seed the joining player's save: standing on the seed's dry-land
        // spawn pick (a placement within legitimate reach must exist around
        // it — the reach eye is ring-bounded, so the visitor builds from
        // where it actually stands) + dirt to place (a fresh spawn would be
        // empty-handed).
        let spawn = crate::worldgen::spawn::find_spawn(7);
        let visitor_feet = Vec3::new(
            spawn.x as f32 + 0.5,
            (spawn.y + 1) as f32,
            spawn.z as f32 + 0.5,
        );
        let mut visitor = crate::player::Player::new(visitor_feet);
        visitor.inventory.add(ItemStack::new(ItemType::Dirt, 64));
        std::fs::write(
            dir.join("players/Visitor.dat"),
            crate::save::player::encode(&visitor),
        )
        .expect("player file");

        let (mut server, _) = crate::game::session::build_session("", 7, 2);
        let opened = crate::save::open_at(dir.clone()).expect("temp save opens");
        server.world.attach_save(opened.save);
        let mut host = ServerHandle::spawn(server);

        let port = host.open_to_lan(0).expect("bind an ephemeral port");
        assert_ne!(port, 0, "the reply carries the actual bound port");
        assert_eq!(
            host.open_to_lan(0).expect("idempotent"),
            port,
            "a second open reports the same port"
        );

        // A wrong protocol version is refused with HelloReject.
        {
            let mut probe = connect(port);
            write_msg(&mut probe, &ClientToServer::Hello { protocol: 9999 }).expect("send");
            match read_msg::<ServerToClient, _>(&mut probe).expect("a reply") {
                ServerToClient::HelloReject { server_protocol } => {
                    assert_eq!(server_protocol, PROTOCOL_VERSION);
                }
                other => panic!("expected HelloReject, got {other:?}"),
            }
        }

        // The real join.
        let mut stream = connect(port);
        let join = client_handshake(&mut stream, "Visitor", 16, &installed_mod_ids(), Vec::new())
            .expect("handshake succeeds")
            .join;
        assert_eq!(join.player_id, PlayerId(1));
        assert_eq!(join.seed, 7);
        assert_eq!(join.self_restore.transform.pos, visitor_feet);
        assert_eq!(
            join.self_restore.inventory[0],
            Some(crate::net::protocol::ItemSlotWire {
                item_id: ItemType::Dirt.0,
                count: 64
            }),
            "the join restore carries the saved inventory"
        );
        assert_eq!(join.players.len(), 1, "only the host was connected");
        assert_eq!(join.players[0].0, PlayerId(0));
        let remap = IdRemap::build(&join.tables);
        assert!(remap.is_identity(), "same process, same registries");
        let conn = TcpClientConn::spawn(stream, remap).expect("connection threads");
        let mut remote = ServerHandle::from_remote(conn);

        // The host is told about the join.
        let joined = drain_until(&mut host, Duration::from_secs(10), |msg| match msg {
            ServerToClient::PlayerJoined { id, name } => Some((id, name)),
            _ => None,
        })
        .expect("host hears PlayerJoined");
        assert_eq!(joined, (PlayerId(1), "Visitor".to_string()));

        // Terrain streams to the remote client WITH the server's baked light
        // (the ship gate holds a section until its light is final; the remote
        // replica never manufactures authoritative light). Meanwhile the
        // visitor's server-side
        // body free-falls from its restored y=80 to the surface — the reach
        // eye is bounded by the F1 drift ring around the server's own
        // integration, so the client reads its SETTLED position off its own
        // replicated player row and builds within reach of it, exactly as a
        // real client would.
        let mut sections: HashMap<SectionPos, (Vec<u8>, Option<Vec<u8>>)> = HashMap::new();
        let mut lit_sections = 0usize;
        let mut self_row: Option<(Vec3, Vec3)> = None;
        let (own_pos, target) = drain_until(&mut remote, Duration::from_secs(60), |msg| {
            match msg {
                ServerToClient::SectionData(p) => {
                    if p.skylight.is_some() {
                        lit_sections += 1;
                    }
                    sections.insert(p.pos, (p.blocks.0.to_vec(), p.water.map(|w| w.0.to_vec())));
                }
                ServerToClient::Tick(update) => {
                    if let Some(row) = update.players.iter().find(|r| r.id == PlayerId(1)) {
                        self_row = Some((row.transform.pos, row.transform.vel));
                    }
                }
                _ => {}
            }
            let (pos, vel) = self_row?;
            if vel.length() > 0.5 {
                return None; // still falling
            }
            find_place_spot_within_reach(&sections, pos).map(|spot| (pos, spot))
        })
        .expect("streamed terrain holds a buildable cell within reach of the settled visitor");
        assert!(lit_sections > 0, "baked light rides TCP section payloads");
        let placed_at = IVec3::new(target.x, target.y + 1, target.z);

        // Place a dirt block from the remote client: standing where the
        // server saw this body settle, aim at a nearby ground cell's top
        // face, use-click.
        let update = PlayerUpdate {
            transform: crate::net::protocol::Transform {
                pos: own_pos,
                vel: Vec3::ZERO,
                yaw: 0.0,
                pitch: 0.0,
            },
            on_ground: true,
            sneak: false,
            gameplay: true,
            break_held: false,
            use_held: false,
            target: Some(TargetRef {
                block: target,
                normal: IVec3::Y,
            }),
            hotbar_slot: 0,
            held_rotation: 0,
            wishdir: Vec3::ZERO,
            jump: false,
            sprint: false,
        };
        remote
            .send(ClientToServer::PlayerUpdate(update))
            .expect("live connection");
        remote
            .send(ClientToServer::Action(PlayerAction::UseClick {
                mob: None,
                target: Some(TargetRef {
                    block: target,
                    normal: IVec3::Y,
                }),
                request_id: None,
                predicted: false,
                jabbed: false,
            }))
            .expect("live connection");
        drain_until(&mut remote, Duration::from_secs(10), |msg| {
            let ServerToClient::Tick(update) = msg else {
                return None;
            };
            update
                .block_deltas
                .iter()
                .find(|d| d.pos == placed_at && d.block_id == Block::Dirt.0)
                .map(|_| ())
        })
        .expect("the remote client's placement comes back as a block delta");

        // Pause is ignored while the server has been opened to LAN: ticks
        // keep flowing to the remote client.
        host.send(ClientToServer::Pause(true)).expect("live pipe");
        std::thread::sleep(Duration::from_millis(200));
        let mut drained = Vec::new();
        remote.drain(&mut drained);
        let before = drained
            .iter()
            .filter_map(|m| match m {
                ServerToClient::Tick(u) => Some(u.tick),
                _ => None,
            })
            .max()
            .unwrap_or(0);
        drain_until(&mut remote, Duration::from_secs(5), |msg| match msg {
            ServerToClient::Tick(u) if u.tick > before => Some(()),
            _ => None,
        })
        .expect("ticks keep flowing: Pause is ignored once open to LAN");

        // A second client with the same name (case-insensitive) is ADMITTED
        // under the lowest free numeric suffix, never refused.
        {
            let mut dup = connect(port);
            let data = client_handshake(&mut dup, "vISITOR", 16, &installed_mod_ids(), Vec::new())
                .expect("a duplicate name joins deduped, not rejected")
                .join;
            let dup_id = data.player_id;
            let name = drain_until(&mut host, Duration::from_secs(10), |msg| match msg {
                ServerToClient::PlayerJoined { id, name } if id == dup_id => Some(name),
                _ => None,
            })
            .expect("host hears the deduped join");
            assert_eq!(
                name, "vISITOR2",
                "the requested name gains a numeric suffix"
            );
            drop(dup); // socket drop -> leave path
            drain_until(&mut host, Duration::from_secs(10), |msg| match msg {
                ServerToClient::PlayerLeft { id } if id == dup_id => Some(()),
                _ => None,
            })
            .expect("the deduped guest's leave lands before the next join");
        }

        // A third player joins, then vanishes (socket drop, no Disconnect):
        // everyone else hears PlayerJoined then PlayerLeft.
        let guest_id = {
            let mut guest = connect(port);
            let data = client_handshake(&mut guest, "Guest", 16, &installed_mod_ids(), Vec::new())
                .expect("guest joins")
                .join;
            assert_eq!(
                data.players.len(),
                2,
                "the guest sees both connected players"
            );
            data.player_id
            // `guest` drops here: the server reader hits EOF -> leave path.
        };
        for (name, handle) in [("host", &mut host), ("visitor", &mut remote)] {
            // One pass for both events: they may land in the same drain batch.
            let mut joined = false;
            let mut left = false;
            drain_until(handle, Duration::from_secs(10), |msg| {
                match msg {
                    ServerToClient::PlayerJoined { id, .. } if id == guest_id => joined = true,
                    ServerToClient::PlayerLeft { id } if id == guest_id => {
                        assert!(joined, "{name}: join broadcasts before leave");
                        left = true;
                    }
                    _ => {}
                }
                (joined && left).then_some(())
            })
            .unwrap_or_else(|| panic!("{name} hears the guest join then leave"));
        }

        // A clean remote quit (farewell Disconnect through the handle drop
        // path) runs the leave path: the host hears PlayerLeft and the
        // visitor's player file is saved with the post-placement inventory.
        remote.shutdown_and_join();
        let left = drain_until(&mut host, Duration::from_secs(10), |msg| match msg {
            ServerToClient::PlayerLeft { id } => Some(id),
            _ => None,
        });
        assert_eq!(left, Some(PlayerId(1)), "host hears the visitor leave");

        let deadline = Instant::now() + Duration::from_secs(10);
        let saved_count = loop {
            if let Some(data) = std::fs::read(dir.join("players/Visitor.dat"))
                .ok()
                .and_then(|bytes| crate::save::player::decode(&bytes))
            {
                let count = data
                    .inventory
                    .slot(0)
                    .map(|s| (s.item, s.count))
                    .unwrap_or((ItemType::Dirt, 0));
                if count == (ItemType::Dirt, 63) || Instant::now() >= deadline {
                    break count;
                }
            } else if Instant::now() >= deadline {
                break (ItemType::Dirt, 0);
            }
            std::thread::sleep(Duration::from_millis(25));
        };
        assert_eq!(
            saved_count,
            (ItemType::Dirt, 63),
            "the leave path saved the visitor with the placed block consumed"
        );

        host.shutdown_and_join();
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The HEADLESS server shape end-to-end: built with NO local session,
    /// the world freezes while empty (pause-when-empty), the first TCP join
    /// claims PlayerId(0) and streams terrain through the ack-windowed
    /// batches, and after the last leave the world freezes again — a rejoin
    /// resumes from (nearly) the frozen tick instead of wall-clock time
    /// having passed.
    #[test]
    fn headless_server_join_leave_cycle_freezes_the_world_when_empty() {
        let mut server = crate::game::session::build_headless_session("", 11, 2);
        assert!(!server.has_local_session);
        assert!(server.sessions.is_empty());
        assert!(server.lan_ever_opened, "the pause gate starts open");
        // In-process smoke first: pumping an EMPTY headless server runs no
        // ticks, produces no recipients, and panics nowhere.
        let t0 = server.world.current_tick();
        for _ in 0..5 {
            let out = server.pump_tagged(0.05, &mut Vec::new(), &[]);
            assert!(out.msgs.is_empty() && out.remote.is_empty());
        }
        assert_eq!(server.world.current_tick(), t0, "empty server: frozen");

        let mut host = ServerHandle::spawn(server);
        let port = host.open_to_lan(0).expect("bind an ephemeral port");

        // First join claims id 0 — no local session holds it on headless.
        let mut stream = connect(port);
        let join = client_handshake(&mut stream, "Head", 16, &installed_mod_ids(), Vec::new())
            .expect("join")
            .join;
        assert_eq!(join.player_id, PlayerId(0));
        let conn =
            TcpClientConn::spawn(stream, IdRemap::build(&join.tables)).expect("conn threads");
        let mut remote = ServerHandle::from_remote(conn);

        // Connected: the world runs and terrain streams (drain_until acks
        // the batches like a live client). Terrain FIRST — the whole
        // render-dist-2 window can finish streaming before the first tick
        // lands, and a drain that waited for a tick would silently discard
        // every section payload it swept past.
        drain_until(&mut remote, Duration::from_secs(60), |msg| {
            matches!(msg, ServerToClient::SectionData(_)).then_some(())
        })
        .expect("terrain streams to the headless server's first player");
        let first = drain_until(&mut remote, Duration::from_secs(10), |msg| match msg {
            ServerToClient::Tick(u) => Some(u.tick),
            _ => None,
        })
        .expect("ticks flow to the joined player");
        let last_seen = drain_until(&mut remote, Duration::from_secs(10), |msg| match msg {
            ServerToClient::Tick(u) if u.tick > first + 5 => Some(u.tick),
            _ => None,
        })
        .expect("the world advances while a player is connected");

        // Clean leave (farewell Disconnect through the handle drop path):
        // the session list empties and the world freezes. Two seconds of
        // wall time would be ~40 ticks if the sim kept running.
        remote.shutdown_and_join();
        std::thread::sleep(Duration::from_secs(2));

        let mut stream = connect(port);
        let join = client_handshake(&mut stream, "Head", 16, &installed_mod_ids(), Vec::new())
            .expect("rejoin")
            .join;
        assert_eq!(join.player_id, PlayerId(0), "the freed id recycles");
        let conn =
            TcpClientConn::spawn(stream, IdRemap::build(&join.tables)).expect("conn threads");
        let mut remote = ServerHandle::from_remote(conn);
        let resumed = drain_until(&mut remote, Duration::from_secs(10), |msg| match msg {
            ServerToClient::Tick(u) => Some(u.tick),
            _ => None,
        })
        .expect("ticks resume on rejoin");
        assert!(
            resumed < last_seen + 20,
            "the world froze while empty: tick {last_seen} -> {resumed} across \
             2+ s of empty wall time (an unfrozen sim would be 40+ ahead)"
        );

        remote.shutdown_and_join();
        host.shutdown_and_join();
    }
}
