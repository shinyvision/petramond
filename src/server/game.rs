//! The server-side simulation: the authoritative world, the connected player
//! sessions, and the fixed-tick stage ladder (multiplayer Phase C2a).
//!
//! [`ServerGame`] owns everything the deterministic 20 TPS tick mutates. Since
//! multiplayer Phase D it runs on its OWN thread (see [`super::handle`]),
//! self-clocked, talking to the client purely over message channels — it must
//! stay `Send` (asserted below). Presentation (camera, particles, lid/swing
//! animation) stays on the client side in `src/game/`.

use std::collections::HashMap;

use crate::crafting::Recipes;
use crate::events::{Attach, EventBus, PostEvent, PostEventKind, SimCtx, Stage, TickSystems};
use crate::game::tick::{TickEvents, WorldEvents, TICK_DT};
use crate::mathh::{IVec3, Vec3};
use crate::mob::LootTables;
use crate::modding::ModHost;
use crate::net::protocol::{
    BlockDelta, ClientToServer, ItemSlotWire, ItemStateRow, MobStateRow, ModSpatialSoundMsg,
    OpenScreen, PlayerAction, PlayerActionKind, PlayerStateRow, PlayerUpdate, SelfEvents,
    SelfState, SelfTransform, ServerToClient, TickUpdate, WorldEventMsg,
};
use crate::player;
use crate::server::player::{ConnectedPlayer, PlayerId};
use crate::world::{StreamEvent, World};

/// Most fixed ticks run in a single frame before the leftover is dropped. Caps
/// catch-up after a stall so the sim never spirals trying to replay lost time.
pub(crate) const MAX_TICKS_PER_FRAME: u32 = 4;

/// One pump's ordered server→client messages PER RECIPIENT — terrain payloads
/// first (column before its sections), then at most one `Tick(TickUpdate)`.
/// Each recipient applies its list in order and consumes NOTHING else (since
/// C2c-iii the tick's events ride the `TickUpdate` itself).
pub(crate) struct PumpOutput {
    /// The LOCAL session's (index 0) messages, for the in-process pipe.
    pub(crate) msgs: Vec<ServerToClient>,
    /// Each REMOTE session's messages, tagged by `PlayerId` — the server
    /// thread routes them to the matching TCP connection (Phase E).
    pub(crate) remote: Vec<(PlayerId, Vec<ServerToClient>)>,
}

/// Minimum number of game ticks between two attack swings, so a player mashing the
/// attack button can't land hits every tick (which would, e.g., instakill an owl).
/// Counted in ticks now that attacks resolve on the fixed tick — 6 ticks ≈ 0.3 s.
pub(crate) const ATTACK_COOLDOWN_TICKS: u32 = 6;

/// The per-tick replication parts every recipient shares: built once per tick
/// window by [`ServerGame::shared_tick_rows`], cloned into each recipient's
/// [`TickUpdate`].
pub(crate) struct SharedTickRows {
    tick: u64,
    clock: u64,
    mobs: Vec<MobStateRow>,
    items: Vec<ItemStateRow>,
    players: Vec<PlayerStateRow>,
    player_actions: Vec<(PlayerId, PlayerActionKind)>,
    open_chests: Vec<IVec3>,
    /// The full shader-param map when anything changed since the last window
    /// (`None` = unchanged) — see [`crate::net::protocol::TickUpdate::env`].
    pub(crate) env: Option<Vec<(String, [f32; 4])>>,
}

/// The simulation half of the former `Game`: authoritative world + sessions +
/// the tick machinery. Field-visible to the client crate-side (`pub(crate)`)
/// because the client currently owns it in-process; the replica flip narrows
/// this to the wire.
pub(crate) struct ServerGame {
    pub(crate) world: World,
    /// The connected players' simulation sessions. On a LISTEN server (the
    /// in-game host) the LOCAL session is index 0 and always exists; on a
    /// HEADLESS server every session is remote and the list may be EMPTY —
    /// fixed ticks are skipped while it is (the world freezes between
    /// players), which is what keeps
    /// every `sessions[0]` mod-ABI site sound: they all run inside the tick.
    pub(crate) sessions: Vec<ConnectedPlayer>,
    /// Whether `sessions[0]` is THIS process's local player (listen server).
    /// False on a headless server ([`crate::game::session::
    /// build_headless_session`]): no local pipe recipient, every session
    /// windowed by the streaming ack loop, and the leave path may empty the
    /// list.
    pub(crate) has_local_session: bool,
    /// Loaded crafting recipes (from `assets/recipes.json`). Used both by the open
    /// `ContainerMenu`'s craft preview (borrowed in per call) and by the furnace
    /// *smelting* tick (`World::game_tick`), which is why they live here rather
    /// than on the menu — the menu would otherwise need a self-referential
    /// borrow during the tick.
    pub(crate) recipes: Recipes,
    /// Mob loot tables (from `assets/loot_tables.json`), rolled when a mob dies to
    /// spawn its dropped items. Loaded once at world load, like [`recipes`](Self::recipes).
    pub(crate) loot: LootTables,
    /// The modding event bus (Phase 1): pre events dispatch at their decision sites,
    /// post events queue and drain at tick-stage boundaries. The engine registers no
    /// handlers yet — the seams exist for mods. See WIKI/modding.md.
    pub(crate) bus: EventBus,
    /// Systems attached between the fixed-tick stages (Phase 1 seam).
    pub(crate) systems: TickSystems,
    /// The WASM mod instances (Phase 2b). Their registered closures (held by
    /// `bus`/`systems`) share ownership; the host keeps the canonical handles
    /// for GUI click dispatch (Phase 5) and diagnostics.
    pub(crate) mods: ModHost,
    pub(crate) spawn_counter: u32,
    /// Next deterministic session handle for mod-owned spatial sounds. The app
    /// owns playback; this counter only gives mods stable identities for stop calls.
    pub(crate) next_mod_sound_handle: u64,
    /// Wall-clock seconds banked toward the next fixed simulation tick.
    pub(crate) tick_accumulator: f32,
    /// Singleplayer pause (`ClientToServer::Pause`): while set, `pump` skips
    /// the fixed ticks ONLY — message drain, streaming, and autosave keep
    /// running — and banks no tick debt (the accumulator is pinned so resume
    /// never fast-forwards). Honored only while [`lan_ever_opened`] is false
    /// (the sole connection is the local one).
    ///
    /// [`lan_ever_opened`]: Self::lan_ever_opened
    pub(crate) paused: bool,
    /// Set (permanently, for the session) when "Open to LAN" first succeeds:
    /// the server force-unpauses and `Pause` messages are ignored from then
    /// on — remote players may exist (or reappear) at any time (Phase E).
    pub(crate) lan_ever_opened: bool,
    /// World-anchored wire events produced OUTSIDE a tick window (a leaving
    /// session's menu close, e.g. its chest 1→0 transition), shipped with the
    /// next executed tick's batch so no observer misses them.
    pub(crate) pending_wire_events: Vec<WorldEventMsg>,
    /// Chat lines accepted since the last pump. Drained to currently connected
    /// sessions only (per [`crate::server::chat::ChatTargets`]); this is
    /// intentionally not history.
    pub(crate) pending_chat: Vec<crate::server::chat::PendingChat>,
    pub(crate) next_chat_seq: u64,
    /// Wall-clock seconds since the last background autosave.
    pub(crate) autosave_t: f32,
    /// How many players currently have each chest's screen open, keyed by
    /// world position. Server-side state (drives what EVERY client's lid
    /// shows); entries are removed at zero. Updated by the menu open/close
    /// funnels; 0↔1 transitions emit `ChestOpened`/`ChestClosed` world events.
    pub(crate) chest_viewers: HashMap<IVec3, u8>,
    /// The `WorldEnvironment` shader-param map the last `TickUpdate.env`
    /// shipped (value-compared per tick window; the map is tiny). `None` =
    /// nothing shipped yet, so the first window always carries the full set.
    /// Replication bookkeeping, not sim state.
    pub(crate) last_shipped_env: Option<std::sync::Arc<crate::world::environment::ShaderParamMap>>,
}

/// The whole sim moves to the server thread at spawn ([`super::handle`]);
/// keep the bound loud so a non-`Send` field is caught at ITS introduction,
/// not at the thread boundary.
const _: () = {
    const fn assert_send<T: Send>() {}
    assert_send::<ServerGame>();
};

impl ServerGame {
    /// Test-only: connect a second (remote-shaped) session and return its index.
    #[cfg(test)]
    pub(crate) fn add_session_for_test(&mut self, player: crate::player::Player) -> usize {
        let id = crate::server::player::PlayerId(self.sessions.len() as u8);
        self.sessions
            .push(ConnectedPlayer::new(id, format!("Player{}", id.0), player));
        self.sessions.len() - 1
    }

    /// Persist everything: flush modified chunks to the save thread, then write
    /// `level.dat` (seed + world tick + mod world KV), one `players/<name>.dat`
    /// per connected session, and the save's mod-set record (`mods.json`). A
    /// no-op without an attached save.
    pub(crate) fn save_all(&mut self) {
        self.world.flush_modified_chunks();
        if let Some(save) = self.world.save() {
            save.save_level(crate::save::level::encode(
                self.world.seed,
                self.world.current_tick(),
                self.world.mod_kv(),
            ));
            for session in &self.sessions {
                save.save_player(&session.name, crate::save::player::encode(&session.player));
            }
            save.save_mods_json(crate::modding::modset::encode_active(
                self.world.disabled_mods(),
            ));
        }
    }

    pub(crate) fn maybe_autosave(&mut self, dt: f32) {
        const AUTOSAVE_SECS: f32 = 30.0;
        if self.world.save().is_none() {
            return;
        }
        self.autosave_t += dt;
        if self.autosave_t >= AUTOSAVE_SECS {
            self.autosave_t = 0.0;
            self.save_all();
        }
    }

    /// [`pump_tagged`](Self::pump_tagged) for a local-only server — the
    /// synchronous test harness's pipe service; production always goes
    /// through the thread loop's `pump_tagged`.
    #[cfg(test)]
    pub(crate) fn pump(&mut self, dt: f32, inbox: &mut Vec<ClientToServer>) -> PumpOutput {
        let local = self.sessions[0].id;
        let mut tagged: Vec<(PlayerId, ClientToServer)> =
            inbox.drain(..).map(|msg| (local, msg)).collect();
        self.pump_tagged(dt, &mut tagged, &[])
    }

    /// Consume this frame's client→server messages (tagged by the sending
    /// session's `PlayerId` — resolved to indices here, so a session that
    /// left during the drain simply drops its residue), run the fixed ticks,
    /// then stream the world (gen/load around every session) and build every
    /// recipient's outbound messages — the whole per-frame server step.
    ///
    /// Per-recipient ordering contract: terrain payloads first (each column
    /// before its sections), then at most one `Tick(TickUpdate)` when a fixed
    /// tick executed. Each client applies its list in order, so a delta for a
    /// section shipped this same pump lands after its install. In-process the
    /// payloads are `Arc` refcount bumps.
    ///
    /// `headroom` is each remote connection's free outbound-queue slots
    /// (`RemoteHub::send_headroom`), keyed by `PlayerId`; the streamer paces
    /// terrain against it. A session with no entry (the local pipe, tests) is
    /// unbounded.
    pub(crate) fn pump_tagged(
        &mut self,
        dt: f32,
        inbox: &mut Vec<(PlayerId, ClientToServer)>,
        headroom: &[(PlayerId, usize)],
    ) -> PumpOutput {
        for (id, msg) in inbox.drain(..) {
            if let Some(s) = self.sessions.iter().position(|x| x.id == id) {
                self.apply_message(s, msg);
            }
        }
        // A tick that moves a session's player (bed tuck, wake, respawn, a mod
        // Teleport) is a teleport, never a fall: player physics runs only
        // client-side, so no tick-side position write can be movement. Snapshot
        // and re-anchor the fall tracker across the ticks.
        for sess in &mut self.sessions {
            sess.pos_before_ticks = sess.player.pos;
        }
        // An EMPTY session list (a headless server between players) freezes
        // the sim exactly like pause: the world clock stops, nothing spawns,
        // and — load-bearing — no tick-side `sessions[0]` mod-ABI site can
        // run against a missing session. Streaming/autosave keep pumping.
        let (mut events, ticks_ran) = if self.paused || self.sessions.is_empty() {
            // Skip the fixed ticks ONLY, and bank no tick debt — the
            // accumulator stays pinned so resume doesn't fast-forward.
            self.tick_accumulator = 0.0;
            (
                TickEvents::with_next_spatial_sound_handle(self.next_mod_sound_handle),
                0,
            )
        } else {
            self.run_fixed_ticks(dt)
        };
        for sess in &mut self.sessions {
            // Observers skip interpolating across a tick-side TELEPORT (bed,
            // respawn, mod teleport, hard knockback). Ordinary F2 physics
            // motion must still lerp — only large discontinuities snap.
            let delta = (sess.player.pos - sess.pos_before_ticks).length();
            sess.tick_teleported = delta > 2.0;
            if sess.tick_teleported {
                sess.fall.reset(sess.player.pos.y);
                sess.pending_fall = 0.0;
            }
        }
        let mut per_session: Vec<Vec<ServerToClient>> =
            self.sessions.iter().map(|_| Vec::new()).collect();
        if !self.pending_chat.is_empty() {
            for pending in self.pending_chat.drain(..) {
                for (s, out) in per_session.iter_mut().enumerate() {
                    if pending.targets.includes(self.sessions[s].id) {
                        out.push(ServerToClient::ChatLine(pending.line.clone()));
                    }
                }
            }
        }
        let queue_room: Vec<usize> = self
            .sessions
            .iter()
            .map(|sess| {
                headroom
                    .iter()
                    .find(|(id, _)| *id == sess.id)
                    .map_or(usize::MAX, |&(_, room)| room)
            })
            .collect();
        // AFTER the ticks, so this pump's payloads carry the tick's edits and
        // freshly-final sections ship the same frame they land.
        self.pump_streaming(dt, &mut per_session, &queue_room);
        if ticks_ran > 0 {
            // Shared batch parts built ONCE per tick window: the drained
            // world-event queue (plus any leave-path events banked between
            // ticks), the coalesced delta log, and the entity/chest rows.
            let mut world_events = std::mem::take(&mut self.pending_wire_events);
            world_events.extend(wire_world_events(&mut events.world));
            let deltas = self.world.take_block_deltas();
            let shared = self.shared_tick_rows(&events);
            for (s, out) in per_session.iter_mut().enumerate() {
                out.push(ServerToClient::Tick(Box::new(self.build_tick_update(
                    s,
                    &events,
                    &world_events,
                    &deltas,
                    &shared,
                ))));
            }
        }
        // Autosave is server-owned (wall-clock dt fed by the thread's loop);
        // it keeps running while paused.
        self.maybe_autosave(dt);

        let mut per_session = per_session.into_iter();
        let msgs = if self.has_local_session {
            per_session.next().expect("the local session is index 0")
        } else {
            Vec::new()
        };
        let remote = self
            .sessions
            .iter()
            .skip(usize::from(self.has_local_session))
            .map(|sess| sess.id)
            .zip(per_session)
            .collect();
        PumpOutput { msgs, remote }
    }

    /// The local session's id (always index 0 on a listen server); `None` on
    /// a headless server, whose sessions are all remote.
    pub(crate) fn local_session_id(&self) -> Option<PlayerId> {
        self.has_local_session.then(|| self.sessions[0].id)
    }

    /// The per-tick batch parts every recipient shares, built once per window
    /// and cheaply cloned per recipient (small rows; `SectionBytes` never
    /// rides here). `&mut` for the env-diff bookkeeping only.
    pub(crate) fn shared_tick_rows(&mut self, events: &TickEvents) -> SharedTickRows {
        let mobs = self
            .world
            .mobs()
            .instances()
            .iter()
            .map(|m| MobStateRow {
                id: m.id(),
                kind_id: m.kind.0,
                pos: m.pos,
                yaw: m.yaw,
                anim_time: m.anim_time,
                moving: m.moving,
                idle_anim: m.idle_anim,
                head_yaw: m.head_yaw,
                head_pitch: m.head_pitch,
                hurt_timer: m.hurt_timer(),
                dead: m.is_dead(),
                shorn: m.is_shorn(),
                // Present only while the death ragdoll plays (bounded): the
                // pose as of this tick (alpha 1.0); the client interpolates
                // consecutive batches.
                ragdoll: m.ragdoll_pose(1.0).map(|pose| {
                    pose.into_iter()
                        .map(|(p, q)| (p.to_array(), q.to_array()))
                        .collect()
                }),
            })
            .collect();
        let items = self
            .world
            .item_entities()
            .iter()
            .map(|it| ItemStateRow {
                id: it.id,
                item_id: it.stack.item.0,
                count: it.stack.count,
                pos: it.pos,
                spin: it.spin,
            })
            .collect();
        // Player rows: EVERY session, to every recipient (the client skips
        // its own id). `hurt_recent` ships the damage EDGE — sessions track
        // no hurt timer; each client runs its own flash envelope.
        let players = self
            .sessions
            .iter()
            .enumerate()
            .map(|(s, sess)| {
                let alive = sess.player.health() > 0;
                PlayerStateRow {
                    id: sess.id,
                    pos: sess.player.pos,
                    vel: sess.player.vel,
                    yaw: sess.player.yaw,
                    pitch: sess.player.pitch,
                    on_ground: sess.player.on_ground,
                    sneaking: sess.intent_sneak,
                    sleeping: sess.sleep.is_some(),
                    sleep_yaw: self.sleep_head_yaw(s),
                    alive,
                    visible: alive && !sess.player.is_spectator(),
                    held_item: sess.selected_item().map(|item| item.0),
                    // The same overlay state `SelfState::mining` ships for the
                    // player's own hand: target cell + crack stage. Observers
                    // derive the arm-swing flag AND the remote crack overlay.
                    mining: sess.mining.overlay(),
                    eating: sess.eating.is_some(),
                    hurt_recent: events.player_at(s).player_damaged,
                    snap: sess.tick_teleported,
                }
            })
            .collect();
        let mut player_actions = Vec::new();
        for (s, sess) in self.sessions.iter().enumerate() {
            let p = events.player_at(s);
            let mut push = |kind| player_actions.push((sess.id, kind));
            if p.swung_hand {
                push(PlayerActionKind::Swung);
            }
            if p.broke_block.is_some() {
                push(PlayerActionKind::Broke);
            }
            if p.placed_block.is_some() {
                push(PlayerActionKind::Placed);
            }
            if p.threw_item {
                push(PlayerActionKind::ThrewItem);
            }
            if p.used_item {
                push(PlayerActionKind::UsedItem);
            }
            if p.interacted {
                push(PlayerActionKind::Interacted);
            }
            if p.ate_finished {
                push(PlayerActionKind::AteFinished);
            }
            if p.player_died {
                push(PlayerActionKind::Died);
            }
            if p.respawned {
                push(PlayerActionKind::Respawned);
            }
        }
        // Full open-chest state per batch (chest_viewers keys; tiny), sorted
        // so the wire batch is deterministic.
        let mut open_chests: Vec<IVec3> = self.chest_viewers.keys().copied().collect();
        open_chests.sort_unstable_by_key(|p| (p.x, p.y, p.z));
        // Environment shader params: value-compare against the last-shipped
        // copy and ship the changed FULL set (the map is ~a dozen entries;
        // day/night rewrites its params most ticks, so this rides most
        // windows). `None` = unchanged, the client keeps what it has.
        let params = self.world.environment().shader_params().clone();
        let env = if self
            .last_shipped_env
            .as_ref()
            .is_some_and(|last| **last == *params)
        {
            None
        } else {
            let rows = params.iter().map(|(k, v)| (k.clone(), *v)).collect();
            self.last_shipped_env = Some(params);
            Some(rows)
        };
        SharedTickRows {
            tick: self.world.current_tick(),
            clock: super::daynight::current_clock(&self.world),
            mobs,
            items,
            players,
            player_actions,
            open_chests,
            env,
        }
    }

    /// Build one recipient's replication batch: the shared rows (all mobs and
    /// dropped items as of the latest tick — interest scoping lands with
    /// per-player streaming), the window's coalesced block deltas restricted
    /// to the recipient's sent sections, the window's world events + session
    /// `s`'s one-shots, its menu sync (when changed), and its own state.
    pub(crate) fn build_tick_update(
        &mut self,
        s: usize,
        events: &TickEvents,
        world_events: &[WorldEventMsg],
        deltas: &[BlockDelta],
        shared: &SharedTickRows,
    ) -> TickUpdate {
        // Per-recipient delta filter: only sections this client holds.
        let mut block_deltas: Vec<BlockDelta> = deltas
            .iter()
            .filter(|d| self.sessions[s].terrain.covers(d.pos))
            .copied()
            .collect();
        // Corrective cell sync: the CURRENT state of cells a use click
        // disagreed about (no-op click, denied place) — how a client whose
        // replica lied (ghost block, stale cell) reconciles. A shared delta
        // for the same cell already carries the truth.
        for pos in std::mem::take(&mut self.sessions[s].pending_corrective_cells) {
            if !self.sessions[s].terrain.covers(pos) || block_deltas.iter().any(|d| d.pos == pos) {
                continue;
            }
            if let Some(d) = self.world.block_delta_at(pos) {
                block_deltas.push(d);
            }
        }
        let action_outcomes = std::mem::take(&mut self.sessions[s].pending_action_outcomes);
        // Echo rule: the initiator already presented their own place/break
        // locally — strip matching world events from THEIR batch only.
        // Observers still receive the shared list unchanged.
        let presented_places: rustc_hash::FxHashSet<_> =
            std::mem::take(&mut self.sessions[s].presented_places)
                .into_iter()
                .collect();
        let presented_breaks: rustc_hash::FxHashSet<_> =
            std::mem::take(&mut self.sessions[s].presented_breaks)
                .into_iter()
                .collect();
        let events_for_recipient: Vec<WorldEventMsg> =
            if presented_places.is_empty() && presented_breaks.is_empty() {
                world_events.to_vec()
            } else {
                world_events
                    .iter()
                    .filter(|ev| match ev {
                        WorldEventMsg::BlockPlaced { pos, .. } => !presented_places.contains(pos),
                        WorldEventMsg::BlockBroken { pos, .. } => !presented_breaks.contains(pos),
                        _ => true,
                    })
                    .cloned()
                    .collect()
            };
        TickUpdate {
            tick: shared.tick,
            clock: shared.clock,
            block_deltas,
            mobs: shared.mobs.clone(),
            items: shared.items.clone(),
            players: shared.players.clone(),
            player_actions: shared.player_actions.clone(),
            self_state: Some(self.build_self_state(s)),
            open_chests: shared.open_chests.clone(),
            env: shared.env.clone(),
            events: events_for_recipient,
            self_events: self.build_self_events(s, events),
            action_outcomes,
            menu_sync: self.build_menu_sync(s),
        }
    }

    /// Session `s`'s per-tick one-shots: the lossy `PlayerTickEvents` slice
    /// plus the session's screen-request outbox (taken here — the request
    /// fields are internal since C2c-iii; the client only sees `OpenScreen`).
    fn build_self_events(&mut self, s: usize, events: &TickEvents) -> SelfEvents {
        let p = events.player_at(s);
        let sess = &mut self.sessions[s];
        // Take every request so nothing lingers; the tick can only set one of
        // them (one consumed click per tick), so first-Some is the open.
        let inventory = std::mem::take(&mut sess.request_open_inventory);
        let table = std::mem::take(&mut sess.request_open_table);
        let furnace = sess.request_open_furnace.take();
        let chest = sess.request_open_chest.take();
        let workbench = std::mem::take(&mut sess.request_open_workbench);
        let mod_gui = sess.request_open_mod_gui.take();
        let sleep = std::mem::take(&mut sess.request_open_sleep);
        let open_screen = if inventory {
            Some(OpenScreen::Inventory)
        } else if table {
            Some(OpenScreen::CraftingTable)
        } else if let Some(pos) = furnace {
            Some(OpenScreen::Furnace(pos))
        } else if let Some(pos) = chest {
            Some(OpenScreen::Chest(pos))
        } else if workbench {
            Some(OpenScreen::Workbench)
        } else if let Some((kind, pos)) = mod_gui {
            crate::gui::kind_key(kind).map(|kind_key| OpenScreen::ModGui {
                kind_key: kind_key.to_string(),
                pos,
            })
        } else if sleep {
            Some(OpenScreen::Sleep)
        } else {
            None
        };
        // The hand one-shots (broke/placed/swung/threw/used/interacted) are
        // deliberately absent: the recipient initiated them and animated at
        // click time — see WIKI/client-prediction.md's echo rule. Observers
        // get them via the shared `player_actions` rows.
        SelfEvents {
            picked_up_item: p.picked_up_item,
            bed_interacted: p.bed_interacted,
            player_damaged: p.player_damaged,
            player_died: p.player_died,
            sleep_ended: p.sleep_ended,
            respawned: p.respawned,
            open_screen,
            close_mod_gui: std::mem::take(&mut sess.request_close_mod_gui),
            toggled_door: p.toggled_door,
        }
    }

    /// Session `s`'s own replicated state. The full inventory rides only when
    /// its revision moved since the last state this session was sent (always
    /// on the first update after join).
    pub(crate) fn build_self_state(&mut self, s: usize) -> SelfState {
        let sleeping = self.sleep_progress01(s).map(|p| (p * 255.0).round() as u8);
        let sleep_bed = self.sleep_bed_base(s);
        let sess = &mut self.sessions[s];
        let player = &sess.player;
        // Transform correction: ships only on REAL divergence from the last
        // CLIENT-REPORTED transform — a rejected claim, a tick-side
        // teleport/knockback, or the very first update (nothing reported yet;
        // the echo is a no-op there, the client seeded from the same player).
        // The server free-runs its integration past a slow client's last
        // claim, so plain inequality is just time-phase drift — correcting it
        // rubber-bands the client. The deadbands scale with the claim gap.
        let current = SelfTransform {
            pos: player.pos,
            vel: player.vel,
            yaw: player.yaw,
            pitch: player.pitch,
            on_ground: player.on_ground,
        };
        let diverged = match &sess.last_reported_transform {
            None => true,
            Some(r) => {
                let spectator = player.is_spectator();
                let gap = sess.ticks_since_claim;
                (player.pos - r.pos).length()
                    > crate::server::movement::claim_drift_ring(spectator, gap)
                    || (player.vel - r.vel).length()
                        > crate::server::movement::vel_correction_eps(gap)
                    // Yaw/pitch never extrapolate (ticks don't turn the
                    // head), so any difference is a genuine server-side set.
                    || player.yaw != r.yaw
                    || player.pitch != r.pitch
            }
        };
        let transform = diverged.then_some(current);
        let revision = player.inventory.revision();
        let inventory = (sess.last_sent_inventory_revision != Some(revision)).then(|| {
            player
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
                .collect()
        });
        sess.last_sent_inventory_revision = Some(revision);
        SelfState {
            health: player.health(),
            mode: match player.mode() {
                crate::player::PlayerMode::Survival => 0,
                crate::player::PlayerMode::Spectator => 1,
            },
            effects: player
                .effects()
                .iter()
                .map(|e| (e.effect.0, e.remaining))
                .collect(),
            inventory_revision: revision,
            inventory,
            eating: sess
                .eating_progress()
                .map(|p| (p.clamp(0.0, 1.0) * 255.0).round() as u8),
            sleeping,
            sleep_bed,
            transform,
        }
    }

    /// Apply one message from session `s`, latching intents/edges the fixed
    /// tick consumes. Message order within a frame is preserved.
    pub(crate) fn apply_message(&mut self, s: usize, msg: ClientToServer) {
        match msg {
            ClientToServer::PlayerUpdate(u) => self.apply_player_update(s, &u),
            ClientToServer::Action(action) => self.apply_action(s, action),
            ClientToServer::MenuClick {
                slot,
                button,
                shift,
                gather,
                request_id,
            } => {
                self.sessions[s].pending_menu_clicks.push((
                    slot.to_menu_slot(),
                    crate::net::protocol::button_from_wire(button),
                    shift,
                    gather,
                    request_id,
                ));
            }
            ClientToServer::ChatSend { text } => {
                let name = self.sessions[s].name.clone();
                if let Some(line) =
                    crate::server::chat::player_line(self.alloc_chat_seq(), &name, &text)
                {
                    self.enqueue_chat(line, crate::server::chat::ChatTargets::All);
                }
            }
            // Pause is honorable only while the sole connection has always
            // been the local one. Once the server has been open to LAN the
            // gate is permanent — see the `lan_ever_opened` field.
            ClientToServer::Pause(paused) => {
                if !self.lan_ever_opened {
                    self.paused = paused;
                }
            }
            ClientToServer::StreamBatchAck {
                messages_per_second,
            } => self.sessions[s]
                .terrain
                .apply_batch_ack(messages_per_second),
            ClientToServer::KeepAlive => {}
            // Handshake/lifecycle messages are consumed by the transport
            // (the hub's pre-join state machine and its leave path); one
            // reaching a joined session is protocol misuse.
            ClientToServer::Hello { .. }
            | ClientToServer::ModQuery
            | ClientToServer::Join { .. }
            | ClientToServer::Disconnect => {
                log::warn!("ignoring handshake/lifecycle message on a joined session");
            }
        }
    }

    /// Queue one accepted chat line for the next pump. Console `say`, player
    /// chat, and join/leave always use [`ChatTargets::All`](crate::server::chat::ChatTargets::All);
    /// mods may target a player-id list.
    pub(crate) fn enqueue_chat(
        &mut self,
        line: crate::net::protocol::ChatLine,
        targets: crate::server::chat::ChatTargets,
    ) {
        self.pending_chat
            .push(crate::server::chat::PendingChat { line, targets });
    }

    pub(crate) fn enqueue_server_chat(&mut self, text: &str) {
        if let Some(line) = crate::server::chat::server_line(self.alloc_chat_seq(), text) {
            self.enqueue_chat(line, crate::server::chat::ChatTargets::All);
        }
    }

    /// Mod-/engine-authored helper text (markup allowed; no `[Server]` prefix).
    pub(crate) fn enqueue_authored_chat(
        &mut self,
        text: &str,
        targets: crate::server::chat::ChatTargets,
    ) {
        if let Some(line) = crate::server::chat::authored_line(self.alloc_chat_seq(), text) {
            self.enqueue_chat(line, targets);
        }
    }

    pub(crate) fn enqueue_join_chat(&mut self, name: &str) {
        let seq = self.alloc_chat_seq();
        self.enqueue_chat(
            crate::server::chat::joined_line(seq, name),
            crate::server::chat::ChatTargets::All,
        );
    }

    pub(crate) fn enqueue_leave_chat(&mut self, name: &str) {
        let seq = self.alloc_chat_seq();
        self.enqueue_chat(
            crate::server::chat::left_line(seq, name),
            crate::server::chat::ChatTargets::All,
        );
    }

    fn alloc_chat_seq(&mut self) -> u64 {
        let seq = self.next_chat_seq;
        self.next_chat_seq = self.next_chat_seq.wrapping_add(1);
        seq
    }

    /// Latch a `PlayerUpdate`: movement intent (F2), validated transform (F1),
    /// hotbar + held rotation, held intents (menu focus forces them off and
    /// drops queued edges, exactly as the old `capture_intent` did), the
    /// reach-validated look target, and the fall tracker.
    fn apply_player_update(&mut self, s: usize, u: &PlayerUpdate) {
        if !(u.pos.is_finite()
            && u.vel.is_finite()
            && u.yaw.is_finite()
            && u.pitch.is_finite()
            && u.wishdir.is_finite())
        {
            log::warn!("dropping PlayerUpdate with non-finite transform/intent");
            return;
        }

        let sess = &mut self.sessions[s];
        sess.move_wishdir = u.wishdir;
        sess.move_jump = u.jump;
        sess.move_sprint = u.sprint;
        sess.claim_pos = u.pos;
        sess.claim_vel = u.vel;
        sess.claim_on_ground = u.on_ground;
        sess.claim_fresh = true;
        // What the client last claimed — after the ticks, a session transform
        // that drifted from this means the server rejected the claim or a
        // tick-side teleport/knockback moved the player.
        sess.last_reported_transform = Some(SelfTransform {
            pos: u.pos,
            vel: u.vel,
            yaw: u.yaw,
            pitch: u.pitch,
            on_ground: u.on_ground,
        });
        sess.player.yaw = u.yaw;
        sess.player.pitch = u.pitch;
        sess.player.inventory.set_active(u.hotbar_slot);
        let selected = sess.selected_item();
        sess.held_rotation.apply_wire(u.held_rotation, selected);

        sess.intent_gameplay = u.gameplay;
        sess.intent_sneak = u.sneak;
        if u.gameplay {
            sess.intent_break_held = u.break_held;
            sess.intent_use_held = u.use_held;
        } else {
            // Menu focus drops queued action edges so clicks cannot fire behind screens.
            sess.intent_break_held = false;
            sess.intent_use_held = false;
            sess.pending_attack = false;
            sess.pending_attack_mob = None;
            sess.pending_attack_player = None;
            sess.pending_place = false;
            sess.pending_use_mob = None;
            sess.pending_place_target = None;
            // The dropped click still owes its outcome: deny, so the client
            // rolls its place ghost back instead of leaking the ledger entry.
            if let Some(id) = sess.pending_place_request_id.take() {
                sess.pending_action_outcomes
                    .push(crate::net::protocol::ActionOutcome {
                        id,
                        accepted: false,
                        reason: Some(crate::net::protocol::ActionDenyReason::Denied),
                    });
            }
        }

        // Reach validation against the CLAIMED eye (where the client says it
        // is aiming from this frame). Measured to the CLOSEST point of the cell.
        let eye = sess.claim_pos + Vec3::new(0.0, crate::player::EYE, 0.0);
        sess.look = u
            .target
            .filter(|t| player::block_within_reach(eye, t.block));
    }

    fn apply_action(&mut self, s: usize, action: PlayerAction) {
        use crate::net::protocol::{ActionDenyReason, ActionOutcome};
        // EVERY latched request id must eventually receive an ActionOutcome —
        // an unanswered id leaks the client's prediction-ledger entry forever
        // (see WIKI/client-prediction.md). Single-slot latches deny the
        // superseded id; queue rejections deny immediately.
        let deny = |id| ActionOutcome {
            id,
            accepted: false,
            reason: Some(ActionDenyReason::Denied),
        };
        match action {
            PlayerAction::UseClick {
                mob,
                target,
                request_id,
            } => {
                let sess = &mut self.sessions[s];
                if let Some(old) = sess.pending_place_request_id.take() {
                    sess.pending_action_outcomes.push(deny(old));
                }
                sess.pending_place = true;
                sess.pending_use_mob = mob;
                // Reach-validate the CLICK's target against the claimed eye —
                // the same rule as the look latch. The placement stage
                // resolves against this cell, never a fresher look.
                let eye = sess.claim_pos + Vec3::new(0.0, crate::player::EYE, 0.0);
                sess.pending_place_target =
                    target.filter(|t| player::block_within_reach(eye, t.block));
                sess.pending_place_request_id = request_id;
            }
            PlayerAction::AttackClick { mob, player } => {
                let sess = &mut self.sessions[s];
                sess.pending_attack = true;
                sess.pending_attack_mob = mob;
                sess.pending_attack_player = player;
            }
            PlayerAction::Drop { all, request_id } => {
                let sess = &mut self.sessions[s];
                let slot = sess.player.inventory.active_slot();
                sess.drop_queue.queue_selected(slot, all, Some(request_id));
            }
            PlayerAction::ThrowCursorStack { request_id } => {
                let sess = &mut self.sessions[s];
                if !sess
                    .drop_queue
                    .queue_cursor_stack(&sess.player.inventory, Some(request_id))
                {
                    sess.pending_action_outcomes.push(deny(request_id));
                }
            }
            PlayerAction::ThrowCursorOne { request_id } => {
                let sess = &mut self.sessions[s];
                if !sess
                    .drop_queue
                    .queue_cursor_one(&sess.player.inventory, Some(request_id))
                {
                    sess.pending_action_outcomes.push(deny(request_id));
                }
            }
            PlayerAction::BreakFinished {
                request_id,
                pos,
                tool_item_id,
            } => {
                // A newer finish supersedes any in-flight latch OR deferred
                // TooFast wait — answer the old id so the ledger cannot leak.
                let (old_pending, old_deferred) = {
                    let sess = &mut self.sessions[s];
                    (
                        sess.pending_break_finished.take(),
                        sess.deferred_break_finished.take(),
                    )
                };
                if let Some(old) = old_pending {
                    self.sessions[s]
                        .pending_action_outcomes
                        .push(deny(old.request_id));
                }
                if let Some(old) = old_deferred {
                    self.sessions[s]
                        .pending_action_outcomes
                        .push(deny(old.request_id));
                    // Old optimistic clear may still be on the client.
                    let cells = self.world.break_footprint_cells(old.pos);
                    self.sessions[s].pending_corrective_cells.extend(cells);
                }
                self.sessions[s].pending_break_finished =
                    Some(crate::server::player::PendingBreakFinished {
                        request_id,
                        pos,
                        tool_item_id,
                    });
            }
            // A mode switch is not tick input: applied at message time, like
            // the direct call it replaces. The floating spectator must never
            // be measured as falling — re-anchor the tracker (mirrors
            // `Player::set_mode`).
            PlayerAction::ToggleMode => {
                let sess = &mut self.sessions[s];
                sess.player.toggle_mode();
                sess.fall.reset(sess.player.pos.y);
                sess.pending_fall = 0.0;
            }
            PlayerAction::Wake => self.sessions[s].wake_requested = true,
            PlayerAction::Respawn => self.sessions[s].respawn_requested = true,
            // Menu transitions run ON THE TICK (their chest-viewer transitions
            // must land in the tick's world events): latched here, consumed by
            // `game_tick_step` (close) / the Menu stage (inventory open).
            PlayerAction::OpenInventory => self.sessions[s].open_inventory_requested = true,
            PlayerAction::CloseMenu => self.sessions[s].close_menu_requested = true,
        }
    }

    pub(crate) fn push_action_outcome(
        &mut self,
        s: usize,
        id: crate::net::protocol::ClientRequestId,
        accepted: bool,
        reason: Option<crate::net::protocol::ActionDenyReason>,
    ) {
        self.sessions[s]
            .pending_action_outcomes
            .push(crate::net::protocol::ActionOutcome {
                id,
                accepted,
                reason,
            });
    }

    /// Run the fixed ticks `dt` banked. Returns the events plus how many ticks
    /// actually executed (the pump emits a replication batch only when > 0).
    pub(crate) fn run_fixed_ticks(&mut self, dt: f32) -> (TickEvents, u32) {
        // Clamp long stalls and cap catch-up so fixed ticks never spiral.
        self.tick_accumulator += dt.clamp(0.0, 1.0);
        let mut ran = 0;
        let mut events = TickEvents::with_next_spatial_sound_handle(self.next_mod_sound_handle);
        while self.tick_accumulator >= TICK_DT && ran < MAX_TICKS_PER_FRAME {
            self.game_tick_step(&mut events);
            self.tick_accumulator -= TICK_DT;
            ran += 1;
        }
        if self.tick_accumulator > TICK_DT {
            self.tick_accumulator = TICK_DT;
        }
        self.next_mod_sound_handle = events.next_spatial_sound_handle();
        (events, ran)
    }

    /// One fixed game tick: world and entity mutation only. The hardwired engine
    /// steps run in [`Stage`] order; between them the scheduler runs attached
    /// systems and the post-event queue drains (see [`end_stage`](Self::end_stage)).
    /// `pub(crate)` so tests can drive exactly one tick.
    pub(crate) fn game_tick_step(&mut self, events: &mut TickEvents) {
        // Post events queued from per-frame code since the last tick (section
        // stream installs, container screens) dispatch first, before any stage:
        // per-frame code only ever queues; handlers run on the tick. Mod
        // actions still queued from the previous tick's final drain (or from
        // mod_init) apply here first.
        self.pump_stream_events();
        self.apply_mod_actions(events);
        self.drain_post_events(events);

        // Latched menu closes (the CloseMenu message) apply before any stage,
        // where CloseMenu used to apply at message time — the close's chest
        // 1→0 transition lands in THIS tick's world events.
        for s in 0..self.sessions.len() {
            if std::mem::take(&mut self.sessions[s].close_menu_requested) {
                self.close_open_menu_for(s, events);
            }
        }

        // Keep action intent before world/entity simulation so inputs resolve
        // on the tick. Per-player stages loop the sessions in id order INSIDE
        // the stage, so the mod seams (`begin_stage`/`end_stage`) still run
        // once per stage per tick.
        for s in 0..self.sessions.len() {
            self.tick_movement(s);
        }

        self.begin_stage(Stage::Mining, events);
        for s in 0..self.sessions.len() {
            self.tick_mining(s, events);
        }
        self.end_stage(Stage::Mining, events);

        self.begin_stage(Stage::Placement, events);
        for s in 0..self.sessions.len() {
            self.tick_place(s, events);
        }
        self.end_stage(Stage::Placement, events);

        self.begin_stage(Stage::Attack, events);
        for s in 0..self.sessions.len() {
            self.tick_attack(s, events);
        }
        self.end_stage(Stage::Attack, events);

        self.begin_stage(Stage::Drops, events);
        for s in 0..self.sessions.len() {
            self.tick_drops(s, events);
        }
        self.end_stage(Stage::Drops, events);

        self.begin_stage(Stage::Menu, events);
        for s in 0..self.sessions.len() {
            self.tick_menu(s, events);
        }
        self.end_stage(Stage::Menu, events);

        self.begin_stage(Stage::PlayerDamage, events);
        for s in 0..self.sessions.len() {
            self.tick_fall_damage(s, events);
            // Status effects ride the same stage: they are pure player-state
            // steps (regen heals, durations count down) on the tick, after damage
            // so a same-tick hit lands before the heal.
            self.tick_effects(s);
            // Sleeping and respawn ride the same stage: both are pure player-state
            // transitions (teleport, health restore, time skip) on the tick.
            self.tick_bed_and_respawn(s, events);
        }
        // Sleep completion is a cross-player decision (everyone must sleep),
        // resolved once after every session advanced its own timer.
        self.resolve_sleep_completion(events);
        self.end_stage(Stage::PlayerDamage, events);

        // World::game_tick's internal order (scheduled → block updates → furnaces
        // → random ticks) is its own sealed contract; the stage wraps it whole.
        self.begin_stage(Stage::WorldScheduled, events);
        self.world.game_tick(&self.recipes);
        self.dispatch_mod_block_hooks(events);
        self.end_stage(Stage::WorldScheduled, events);

        self.begin_stage(Stage::NaturalBreaks, events);
        self.process_natural_breaks(events);
        self.end_stage(Stage::NaturalBreaks, events);

        self.begin_stage(Stage::Pickup, events);
        // Drop lifetime advances once per tick; each player then vacuums
        // eligible drops in session-id order.
        self.world.tick_item_lifetime();
        // Reservations are per-requester: release any whose owner is gone or
        // dead (their pickup pass no longer runs to re-evaluate it), so the
        // drop returns to the pool this tick instead of staying claimed.
        {
            let sessions = &self.sessions;
            self.world
                .dropped_items_mut()
                .release_requests_not_from(|id| {
                    sessions
                        .iter()
                        .any(|sess| sess.id == id && sess.player.health() > 0)
                });
        }
        for s in 0..self.sessions.len() {
            if self.item_pickup_tick(s) {
                events.player(s).picked_up_item = true;
                // Every observer hears the pickup at the collector's body.
                events
                    .world
                    .item_picked_up
                    .push((self.sessions[s].player.body_center(), self.sessions[s].id));
            }
        }
        self.end_stage(Stage::Pickup, events);

        // Player anchors for the entity stages, sampled here (after
        // PlayerDamage teleports settle — same point the old single-player
        // snapshot was taken).
        let anchors: Vec<crate::mob::PlayerAnchor> = self
            .sessions
            .iter()
            .map(|sess| crate::mob::PlayerAnchor {
                id: sess.id,
                pos: sess.player.body_center(),
                body: (!sess.player.is_spectator()).then(|| sess.player.body()),
            })
            .collect();
        // Passive natural spawning still centres on one anchor per tick, round-robin,
        // so its per-tick attempt budget stays constant. Hostile spawning builds its
        // own chunk/cap plan from every connected anchor below.
        let spawn_s = (self.world.current_tick() as usize) % self.sessions.len();

        self.begin_stage(Stage::Mobs, events);
        let mob_events = self.world.tick_mobs(TICK_DT, &anchors);
        self.apply_mob_fall_damage(mob_events.falls, events);
        // Mob→player combat resolves right after the mobs moved: each strike runs
        // through the `player_damage_pre` pipeline (i-frame mods cancel there) and
        // an applied strike knocks the player back.
        self.apply_mob_attacks(mob_events.attacks, events);
        self.end_stage(Stage::Mobs, events);

        self.begin_stage(Stage::ItemPhysics, events);
        // The magnet pulls each requested drop toward ITS requester, so the
        // anchors carry ids alongside the body centres.
        let magnet_anchors: Vec<(PlayerId, crate::mathh::Vec3)> =
            anchors.iter().map(|a| (a.id, a.pos)).collect();
        self.world.tick_item_physics(TICK_DT, &magnet_anchors);
        self.end_stage(Stage::ItemPhysics, events);

        self.begin_stage(Stage::Spawning, events);
        for (kind, pos) in self.world.spawn_mobs_tick(anchors[spawn_s].pos) {
            self.bus.emit(PostEvent::MobSpawned { kind, pos });
        }
        self.tick_mod_hostile_mob_spawns(&anchors, events);
        self.end_stage(Stage::Spawning, events);
    }

    /// Forward the behavior hooks the world tick queued on mod-behavior blocks
    /// (see `block::behavior::wasm`) to their owning mods, inside the same
    /// stage window as the world tick that fired them. The queue is drained
    /// unconditionally so it never carries over between ticks.
    fn dispatch_mod_block_hooks(&mut self, events: &mut TickEvents) {
        let hooks = self.world.take_mod_block_hooks();
        if hooks.is_empty() || !self.mods.has_block_behaviors() {
            return;
        }
        let Self {
            world,
            sessions,
            mods,
            bus,
            ..
        } = self;
        let host = &mut sessions[0];
        let mut ctx = SimCtx {
            world,
            player: &mut host.player,
            gui_state: &mut host.gui_state,
            feed: events,
            queue: bus.queue_mut(),
        };
        mods.dispatch_block_hooks(&mut ctx, &hooks);
    }

    fn tick_mod_hostile_mob_spawns(
        &mut self,
        anchors: &[crate::mob::PlayerAnchor],
        events: &mut TickEvents,
    ) {
        if !self.mods.has_hostile_spawners() {
            return;
        }

        let player_positions: Vec<_> = anchors.iter().map(|a| a.pos).collect();
        let Some(plan) = crate::mob::hostile_spawn_plan(&self.world, &player_positions) else {
            return;
        };

        'attempts: for attempt in 0..crate::mob::HOSTILE_SPAWN_ATTEMPTS {
            let sites = crate::mob::hostile_attempt_sites(&self.world, &plan, attempt);
            for site in sites {
                let kind = {
                    let Self {
                        world,
                        sessions,
                        mods,
                        bus,
                        ..
                    } = self;
                    let host = &mut sessions[0];
                    let mut ctx = SimCtx {
                        world,
                        player: &mut host.player,
                        gui_state: &mut host.gui_state,
                        feed: events,
                        queue: bus.queue_mut(),
                    };
                    mods.hostile_spawn_kind(&mut ctx, &site.candidate)
                };
                let Some(kind) = kind else {
                    continue;
                };
                if !crate::mob::hostile_kind_has_room(&self.world, &plan, kind) {
                    continue;
                }
                if self.world.spawn_mob(kind, site.pos, site.yaw) {
                    self.bus.emit(PostEvent::MobSpawned {
                        kind,
                        pos: site.pos,
                    });
                }
                break 'attempts;
            }
        }
    }

    /// Run the systems attached at `at` — the mod seam. Nothing is attached in
    /// Phase 1, so this is a bounds-checked array read per stage edge.
    fn run_systems(&mut self, at: Attach, events: &mut TickEvents) {
        if self.systems.is_empty_at(at) {
            return;
        }
        let Self {
            world,
            sessions,
            systems,
            bus,
            ..
        } = self;
        let host = &mut sessions[0];
        systems.run(
            at,
            world,
            &mut host.player,
            &mut host.gui_state,
            events,
            bus.queue_mut(),
        );
    }

    /// Open a stage: run its `Before` systems, then apply any mod actions they
    /// queued (`DamagePlayer`/`DamageMob`/... — see `apply_mod_actions`) BEFORE
    /// the engine step runs, so mob indices captured by those systems cannot be
    /// shifted by the step in between.
    fn begin_stage(&mut self, stage: Stage, events: &mut TickEvents) {
        self.run_systems(Attach::Before(stage), events);
        self.apply_mod_actions(events);
    }

    /// Close a stage: run its `After` systems, apply the mod actions they (or
    /// the stage's inline pre-event handlers) queued, then drain the post
    /// queue — so post events emitted by those actions (`player_damaged`,
    /// `mob_died`) dispatch within the same tick, at the earliest defined
    /// point. Actions queued by post handlers during the drain roll to the
    /// next action point (next stage or next tick's start) — no recursion.
    fn end_stage(&mut self, stage: Stage, events: &mut TickEvents) {
        self.run_systems(Attach::After(stage), events);
        self.apply_mod_actions(events);
        self.drain_post_events(events);
    }

    fn drain_post_events(&mut self, events: &mut TickEvents) {
        let Self {
            world,
            sessions,
            bus,
            ..
        } = self;
        let host = &mut sessions[0];
        bus.drain_post(world, &mut host.player, &mut host.gui_state, events);
    }

    /// Session `s`'s menu-session view when it changed since the last sync
    /// this session was sent (`None` = unchanged). The base message compares
    /// by value (its `gui_state` field is held `None` for the compare); the
    /// state map compares by `Arc` identity — holding the shipped `Arc` in
    /// `last_sent_gui_state` forces the next tick-side write to copy-on-write
    /// onto a fresh allocation, which is what makes identity sound here.
    pub(crate) fn build_menu_sync(
        &mut self,
        s: usize,
    ) -> Option<crate::net::protocol::MenuSyncMsg> {
        use crate::game::container::ContainerTarget;
        use crate::net::protocol::{GuiValueWire, MenuTargetWire};

        let base = self.build_menu_sync_base(s);
        let target = self.sessions[s].menu.target();
        let gui_arc = matches!(target, ContainerTarget::ModGui { .. })
            .then(|| self.mod_gui_state_for_menu_sync(s, target));
        let sess = &mut self.sessions[s];
        let gui_changed = match (&gui_arc, &sess.last_sent_gui_state) {
            (None, None) => false,
            (Some(a), Some(b)) => !std::sync::Arc::ptr_eq(a, b),
            _ => true,
        };
        let base_changed = sess.last_menu_sync.as_ref() != Some(&base);
        if !base_changed && !gui_changed {
            return None;
        }
        let mut out = base.clone();
        if gui_changed {
            if let (MenuTargetWire::ModGui { gui_state, .. }, Some(map)) =
                (&mut out.target, &gui_arc)
            {
                *gui_state = Some(
                    map.iter()
                        .map(|(k, v)| (k.clone(), GuiValueWire::from_value(v)))
                        .collect(),
                );
            }
            sess.last_sent_gui_state = gui_arc;
        }
        sess.last_menu_sync = Some(base);
        Some(out)
    }

    fn mod_gui_state_for_menu_sync(
        &self,
        s: usize,
        target: crate::game::container::ContainerTarget,
    ) -> std::sync::Arc<crate::gui::GuiStateMap> {
        let own = self.sessions[s].gui_state.clone();
        if s == 0 || !own.is_empty() {
            return own;
        }
        let host = self.sessions[0].gui_state.clone();
        if host.is_empty() || !self.host_mod_gui_state_applies_to(target) {
            return own;
        }
        host
    }

    fn host_mod_gui_state_applies_to(
        &self,
        target: crate::game::container::ContainerTarget,
    ) -> bool {
        use crate::game::container::ContainerTarget;

        if self.sessions[0].menu.target() == target {
            return true;
        }
        let mut open_target = None;
        for sess in &self.sessions {
            let t @ ContainerTarget::ModGui { .. } = sess.menu.target() else {
                continue;
            };
            if open_target.is_some_and(|seen| seen != t) {
                return false;
            }
            open_target = Some(t);
        }
        open_target == Some(target)
    }

    /// Hand the section stream events buffered by the per-frame `World::poll` to
    /// the bus. The capture gate mirrors listener presence so an idle bus costs
    /// the streamer nothing.
    fn pump_stream_events(&mut self) {
        let wants = self.bus.wants(PostEventKind::SectionGenerated)
            || self.bus.wants(PostEventKind::SectionLoaded);
        self.world.set_stream_event_capture(wants);
        if !wants {
            return;
        }
        for ev in self.world.take_stream_events() {
            self.bus.emit(match ev {
                StreamEvent::Generated(pos) => PostEvent::SectionGenerated { pos },
                StreamEvent::Loaded(pos) => PostEvent::SectionLoaded { pos },
            });
        }
    }
}

/// Drain one tick window's world-anchored event queues into the wire batch
/// every recipient shares. Order: block/door/chest/pickup events first (they
/// key presentation state seeds), then the sound queues, each in emission
/// order.
pub(crate) fn wire_world_events(world: &mut WorldEvents) -> Vec<WorldEventMsg> {
    let mut out = Vec::new();
    for ev in world.block_broken.drain(..) {
        out.push(WorldEventMsg::BlockBroken {
            pos: ev.pos,
            block_id: ev.block.0,
            normal: ev.normal,
        });
    }
    for (pos, block) in world.block_placed.drain(..) {
        out.push(WorldEventMsg::BlockPlaced {
            pos,
            block_id: block.0,
        });
    }
    for (lower, open) in world.door_changed.drain(..) {
        out.push(WorldEventMsg::DoorToggled { lower, open });
    }
    for (pos, open) in world.chest_changed.drain(..) {
        out.push(if open {
            WorldEventMsg::ChestOpened { pos }
        } else {
            WorldEventMsg::ChestClosed { pos }
        });
    }
    for (pos, by) in world.item_picked_up.drain(..) {
        out.push(WorldEventMsg::ItemPickedUp { pos, by });
    }
    for s in world.mob_sounds.drain(..) {
        out.push(WorldEventMsg::MobSound {
            mob_id: s.mob_id,
            kind_id: s.kind.0,
            category: s.category.to_u8(),
            pos: s.pos,
        });
    }
    for s in world.sounds.drain(..) {
        out.push(WorldEventMsg::ModSound {
            sound_id: s.sound.0,
            pos: s.pos,
        });
    }
    for c in world.spatial_sounds.drain(..) {
        use crate::game::ModSpatialSoundCommand as Cmd;
        out.push(WorldEventMsg::ModSpatialSound(match c {
            Cmd::PlayAt {
                handle,
                sound,
                pos,
                volume,
                pitch,
            } => ModSpatialSoundMsg::PlayAt {
                handle,
                sound_id: sound.0,
                pos,
                volume,
                pitch,
            },
            Cmd::PlayOnMob {
                handle,
                sound,
                mob_id,
                volume,
                pitch,
                last_pos,
            } => ModSpatialSoundMsg::PlayOnMob {
                handle,
                sound_id: sound.0,
                mob_id,
                volume,
                pitch,
                last_pos,
            },
            Cmd::Stop { handle } => ModSpatialSoundMsg::Stop { handle },
        }));
    }
    out
}

#[cfg(test)]
mod chat_delivery_tests {
    use crate::net::protocol::ServerToClient;
    use crate::server::chat::ChatTargets;

    fn chat_texts(msgs: &[ServerToClient]) -> Vec<String> {
        msgs.iter()
            .filter_map(|m| match m {
                ServerToClient::ChatLine(line) => Some(
                    line.spans
                        .iter()
                        .map(|s| s.text.as_str())
                        .collect::<String>(),
                ),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn targeted_chat_reaches_only_listed_sessions() {
        let (mut server, _) = crate::game::session::build_session("", 1, 2);
        let player = crate::game::session::spawn_player(server.world.seed);
        let remote_s = server.add_session_for_test(player);
        let remote_id = server.sessions[remote_s].id;

        server.enqueue_authored_chat("only-remote", ChatTargets::Players(vec![remote_id]));
        server.enqueue_authored_chat("everyone", ChatTargets::All);

        let out = server.pump(0.0, &mut Vec::new());
        let local = chat_texts(&out.msgs);
        assert!(
            !local.iter().any(|t| t.contains("only-remote")),
            "local must not receive a remote-only line"
        );
        assert!(
            local.iter().any(|t| t.contains("everyone")),
            "local must receive broadcast"
        );

        let remote_msgs = out
            .remote
            .iter()
            .find(|(id, _)| *id == remote_id)
            .map(|(_, msgs)| msgs.as_slice())
            .unwrap_or(&[]);
        let remote = chat_texts(remote_msgs);
        assert!(
            remote.iter().any(|t| t.contains("only-remote")),
            "remote must receive its targeted line"
        );
        assert!(
            remote.iter().any(|t| t.contains("everyone")),
            "remote must receive broadcast"
        );
    }
}
