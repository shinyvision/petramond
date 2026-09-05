//! Client-side REPLICATED entity/self stores.
//!
//! The client renders mobs, dropped items, and its own HUD state from these
//! stores, fed by the per-tick [`TickUpdate`] batches the server emits — the
//! sim itself is unreachable (it lives on its own thread). Locally the
//! batches are plain values over channels; over TCP the identical messages
//! arrive remapped, so nothing here changes.
//!
//! Each store keeps the PREVIOUS and CURRENT batch row per stable id — the
//! interpolation-ready pair `collect_mobs`/`collect_item_entities` blend at
//! `tick_alpha`, exactly as the renderer used to blend `Instance::prev_*`.
//! Light is deliberately absent from the rows: the client samples it at the
//! entity position from its REPLICA world.

use std::collections::BTreeMap;
use std::sync::Arc;

use glam::{Quat, Vec3};

use petramond_world::gui_state::ContainerView;

use petramond::net::protocol::{
    ItemSlotWire, ItemStateRow, MenuSyncMsg, MenuTargetWire, MobStateRow, PlayerActionKind,
    PlayerMount, PlayerStateRow, SelfState, SpatialSoundMsg, TickUpdate, WorldEventMsg,
};
use petramond::player::PlayerId;
use petramond::player::{Player, PlayerMode};
use petramond_math::math::{IVec3, Tilt};
use petramond_world::gui_state::GuiStateMap;
use petramond_world::inventory::{Hand, Inventory};
use petramond_world::item::{ItemStack, ItemType};

use super::tick::WorldEvent;
use super::Game;

/// One `TickUpdate`'s entity rows, STAGED until render time crosses into
/// their segment (see [`ReplicaClock`](super::tick::ReplicaClock)): the
/// committed prev→curr pair under the render never shifts mid-segment, which
/// is what keeps interpolated motion — and a rider's camera glued to it —
/// free of arrival-jitter rubber-banding.
pub struct StagedRows {
    pub mobs: Vec<MobStateRow>,
    pub items: Vec<ItemStateRow>,
    pub players: Vec<PlayerStateRow>,
    pub actions: Vec<(PlayerId, PlayerActionKind)>,
    /// An overflow collapsed older pending snapshots into this newest one.
    /// Its first boundary commit must seed prev == curr rather than lerp over
    /// the dropped gap.
    resync: bool,
}

/// Normal scheduling jitter needs only a few pending ticks. If a stalled
/// client exceeds this depth, staging collapses deterministically to the
/// newest snapshot instead of either growing without bound or mutating the
/// committed interpolation pair mid-segment.
pub const MAX_STAGED_ROW_BATCHES: usize = 4;

/// How fast a named mob animation blends in/out (weight per second): ~0.17 s
/// to full — an oar picks up and settles instead of snapping between poses.
const ANIM_BLEND_PER_SEC: f32 = 6.0;

/// One replicated mob: the previous and current batch rows, keyed by the
/// mob's stable id in [`ReplicatedMobs`].
pub struct ReplicatedMob {
    pub prev: MobStateRow,
    pub curr: MobStateRow,
    /// CLIENT-side blend state over the replicated named animations
    /// (`curr.anims` names are the target set): `(name, weight, phase)` —
    /// the weight eases per frame toward 1 for active names and 0 for
    /// dropped ones (layers fade instead of snapping), and `phase` holds the
    /// last replicated phase so a fading-OUT layer keeps its pose.
    /// Presentation state only, advanced by
    /// [`ReplicatedMobs::advance_anim_blends`].
    pub anim_blend: Vec<(String, f32, f32)>,
}

impl ReplicatedMob {
    /// The feet pose this replicated row presents at `alpha`. Picking,
    /// collision, seats, and rendering all speak this same prev→curr blend;
    /// keeping the shortest-arc yaw rule here prevents interaction geometry
    /// from drifting onto the future tick while the model is still between.
    pub fn interpolated_pose(&self, alpha: f32) -> (Vec3, f32) {
        (
            self.prev.pos.lerp(self.curr.pos, alpha),
            petramond_math::math::lerp_angle(self.prev.yaw, self.curr.yaw, alpha),
        )
    }

    /// The body tilt this row presents at `alpha`, blended like the pose.
    pub fn interpolated_tilt(&self, alpha: f32) -> Tilt {
        self.prev.tilt.lerp(self.curr.tilt, alpha)
    }
}

/// A rider's frame on its mount this frame: where the seat is, the BODY yaw
/// the rider sits square to (PLAYER convention: `0` faces `+Z`), and the tilt
/// the seated body leans with. Local slaving and remote presentation share
/// this one lookup, so row pairing, yaw convention, seat projection and lean
/// cannot drift apart between the two.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct MountPose {
    pub seat: Vec3,
    pub body_yaw: f32,
    pub tilt: Tilt,
}

/// The client's replicated mob set. `BTreeMap` so presentation iterates in a
/// deterministic (id) order.
#[derive(Default)]
pub struct ReplicatedMobs {
    rows: BTreeMap<u64, ReplicatedMob>,
}

impl ReplicatedMobs {
    /// Apply one batch: a known id shifts curr→prev and adopts the new row, a
    /// fresh id starts with prev == curr (no interpolation from nowhere), and
    /// an id absent from the batch is dropped (killed/despawned server-side).
    pub fn apply(&mut self, batch: Vec<MobStateRow>) {
        let mut old = std::mem::take(&mut self.rows);
        for row in batch {
            // A fresh id starts its animations at FULL weight (a mob streamed
            // in mid-row must not fade in from rest); a known id keeps its
            // blend state and eases toward the new target set.
            let (prev, anim_blend) = match old.remove(&row.id) {
                Some(entry) => (entry.curr, entry.anim_blend),
                None => (
                    row.clone(),
                    row.anims
                        .iter()
                        .map(|(n, phase)| (n.clone(), 1.0, *phase))
                        .collect(),
                ),
            };
            self.rows.insert(
                row.id,
                ReplicatedMob {
                    prev,
                    curr: row,
                    anim_blend,
                },
            );
        }
    }

    /// Replace a discontinuous backlog with one fresh interpolation seed.
    fn resync(&mut self, batch: Vec<MobStateRow>) {
        self.rows.clear();
        self.apply(batch);
    }

    /// Ease every entry's animation blend weights toward its replicated
    /// target set (in → 1, out → 0, dropped at 0), refreshing each active
    /// layer's held phase from the row (a fading-out layer keeps its last
    /// pose). Runs once per frame.
    pub fn advance_anim_blends(&mut self, dt: f32) {
        let step = ANIM_BLEND_PER_SEC * dt;
        for entry in self.rows.values_mut() {
            for (name, phase) in &entry.curr.anims {
                if !entry.anim_blend.iter().any(|(n, _, _)| n == name) {
                    entry.anim_blend.push((name.clone(), 0.0, *phase));
                }
            }
            let target = &entry.curr.anims;
            for (name, weight, phase) in entry.anim_blend.iter_mut() {
                match target.iter().find(|(n, _)| n == name) {
                    Some((_, row_phase)) => {
                        *weight = (*weight + step).min(1.0);
                        *phase = *row_phase;
                    }
                    None => *weight -= step,
                }
            }
            entry.anim_blend.retain(|(_, w, _)| *w > 0.0);
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = &ReplicatedMob> {
        self.rows.values()
    }

    /// The replicated mob with stable id `id`, if present this batch — how
    /// rider glue finds its mount.
    pub fn get(&self, id: u64) -> Option<&ReplicatedMob> {
        self.rows.get(&id)
    }

    /// The rider's frame on `mount` at `alpha`, or `None` while a mob mount's
    /// rows are not available yet — the caller keeps its current transform
    /// and waits for the rows to agree. A pose anchor is static world state:
    /// the wire pos IS the seat, its yaw is already player convention, and it
    /// is level.
    pub fn mount_pose(&self, mount: PlayerMount, alpha: f32) -> Option<MountPose> {
        match mount {
            PlayerMount::Mob { id, seat } => {
                let entry = self.get(id)?;
                let def = petramond::mob::def(petramond::mob::Mob(entry.curr.kind_id));
                let offset = *def.seats.get(seat as usize)?;
                let (pos, yaw) = entry.interpolated_pose(alpha);
                let tilt = entry.interpolated_tilt(alpha);
                Some(MountPose {
                    seat: petramond::mob::riding::seat_world_pos(pos, yaw, tilt, offset),
                    // Mob yaw is mount convention (`0` faces `-Z`), π from
                    // player body yaw.
                    body_yaw: petramond_math::math::wrap_angle(yaw + std::f32::consts::PI),
                    tilt,
                })
            }
            PlayerMount::Anchor { pos, yaw, .. } => Some(MountPose {
                seat: pos,
                body_yaw: yaw,
                tilt: Tilt::LEVEL,
            }),
        }
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.rows.len()
    }

    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// One replicated dropped item (prev/current batch rows).
pub struct ReplicatedItem {
    pub prev: ItemStateRow,
    pub curr: ItemStateRow,
}

/// The client's replicated dropped-item set — same contract as
/// [`ReplicatedMobs`].
#[derive(Default)]
pub struct ReplicatedItems {
    rows: BTreeMap<u64, ReplicatedItem>,
}

impl ReplicatedItems {
    pub fn apply(&mut self, batch: Vec<ItemStateRow>) {
        let mut old = std::mem::take(&mut self.rows);
        for row in batch {
            let prev = match old.remove(&row.id) {
                Some(entry) => entry.curr,
                None => row.clone(),
            };
            self.rows.insert(row.id, ReplicatedItem { prev, curr: row });
        }
    }

    /// Replace a discontinuous backlog with one fresh interpolation seed.
    fn resync(&mut self, batch: Vec<ItemStateRow>) {
        self.rows.clear();
        self.apply(batch);
    }

    pub fn iter(&self) -> impl Iterator<Item = &ReplicatedItem> {
        self.rows.values()
    }
}

/// The client-side mirror of the local player's [`SelfState`]: everything the
/// HUD, hand, and overlays read. Seeded from the session at join (the wire
/// path seeds it from `SelfRestore`), then overwritten by every batch.
pub struct SelfView {
    /// Health in half-heart points.
    pub health: i32,
    pub mode: PlayerMode,
    /// Active effects (id, remaining ticks) in application order. Wire effect
    /// ids arrive already remapped to local ids, so they are stored directly.
    pub effects: Vec<(petramond_world::effect::Effect, u32)>,
    /// A real `Inventory` value reconstructed from the wire slots — the menu
    /// renders slots + cursor from it. Contents refresh only when the server
    /// shipped them (revision moved); the active slot refreshes every batch.
    pub inventory: Inventory,
    /// Server-side content revision. Reconstructing `Inventory` from a wire
    /// snapshot resets its own local counter, so cache users retain this one.
    pub inventory_revision: u64,
    /// The in-progress mining target + crack stage (0..=9).
    pub mining: Option<(IVec3, u8)>,
    /// The in-progress eat's progress in `[0, 1)`.
    pub eating: Option<f32>,
    /// The in-progress eat consumes from the OFF hand — the left hand carries
    /// the food. Meaningless while `eating` is `None`.
    pub eating_off_hand: bool,
    /// The in-progress sleep's fade progress in `[0, 1]`.
    pub sleeping: Option<f32>,
    /// The in-progress sleep's bed base (foot) cell.
    pub sleep_bed: Option<IVec3>,
    /// The body-level land-speed scale the movement code reads every step
    /// (adopted onto the predicted player beside the effect list).
    pub move_scale: f32,
    /// The actions mods denied on this body
    /// adopted onto the predicted player with the speed scale — the local
    /// mining timer and the attack click read it, so the button goes dead here
    /// at the same moment it does on the authority.
    pub denied_actions: petramond::player::DeniedActions,
    /// Per-hand held poses — the AUTHORITATIVE answer
    /// for this player's hands, which a client mod predicting the same rule
    /// overrides locally (see `ClientModRuntime::local_held_poses`).
    pub held_pose_main: Option<mod_api::HeldPose>,
    pub held_pose_off: Option<mod_api::HeldPose>,
    /// What each hand displays in place of its stack (`[main, off]`) — the
    /// authoritative answer, overridden locally by a client mod dressing the
    /// same hand (see `ClientModRuntime::local_held_displays`).
    pub held_display: [Option<ItemType>; 2],
    /// claimed rig-bone offsets — the authoritative
    /// answer, overridden locally by a client mod predicting the same rule.
    pub bone_poses: Vec<petramond::player::BonePose>,
    /// Which of each hand's engine motions carry a live claim (`[main,
    /// off]`) — the authoritative answer, overridden locally by a client mod
    /// making the same claim (see `ClientModRuntime::local_motion_claims`).
    pub motion_claims: [petramond::player::HandMotions; 2],
}

impl SelfView {
    /// Seed from the freshly-restored session player at world open — the
    /// in-process stand-in for the join handshake's `SelfRestore`, so the HUD
    /// is right on the very first frame (before any tick has run).
    pub fn seed_from(player: &Player) -> Self {
        Self {
            health: player.health(),
            mode: player.mode(),
            effects: player
                .effects()
                .iter()
                .map(|e| (e.effect, e.remaining))
                .collect(),
            inventory: player.inventory.clone(),
            inventory_revision: player.inventory.revision(),
            mining: None,
            eating: None,
            eating_off_hand: false,
            sleeping: None,
            sleep_bed: None,
            move_scale: player
                .claims
                .replicated_attribute(mod_api::PlayerAttribute::MoveSpeed),
            denied_actions: player.claims.replicated_denied_actions(),
            held_pose_main: player.claims.held_pose(Hand::Main),
            held_pose_off: player.claims.held_pose(Hand::Off),
            held_display: [
                player.claims.held_display(Hand::Main),
                player.claims.held_display(Hand::Off),
            ],
            bone_poses: player.claims.bone_poses().collect(),
            motion_claims: [
                player.claims.hand_motions(Hand::Main),
                player.claims.hand_motions(Hand::Off),
            ],
        }
    }

    /// Adopt one batch's self state. `adopt_inventory` is false when the
    /// batch's inventory snapshot is stale against a pending prediction (see
    /// `apply_tick_update`): contents and revision then keep the predicted
    /// view — the pending request's own outcome batch carries the truth.
    pub fn apply(&mut self, state: &SelfState, adopt_inventory: bool) {
        self.health = state.health;
        self.mode = match state.mode {
            1 => PlayerMode::Spectator,
            _ => PlayerMode::Survival,
        };
        self.effects = state
            .effects
            .iter()
            .map(|&(id, remaining)| (petramond_world::effect::Effect(id), remaining))
            .collect();
        // The active hotbar INDEX is client-owned (it rides `PlayerUpdate`):
        // a full-body ship keeps the CURRENT local selection, never a server
        // echo that would yank a fast scroll back. `mining` is likewise
        // untouched — the own crack overlay is the local timer's.
        if adopt_inventory {
            if let Some(slots) = &state.inventory {
                let active = self.inventory.active_slot();
                self.inventory = inventory_from_wire(slots, active);
            }
            self.inventory_revision = state.inventory_revision;
        }
        self.eating = state.eating.map(|p| p as f32 / 255.0);
        self.eating_off_hand = state.eating_off_hand;
        self.sleeping = state.sleeping.map(|p| p as f32 / 255.0);
        self.sleep_bed = state.sleep_bed;
        self.move_scale = state.move_scale;
        self.denied_actions = state.denied_actions;
        self.held_pose_main = state.held_pose_main;
        self.held_pose_off = state.held_pose_off;
        self.held_display = state.held_display.map(|id| id.map(ItemType));
        self.bone_poses.clone_from(&state.bone_poses);
        self.motion_claims = state.motion_claims;
    }
}
/// The client's MENU-session mirror, fed by [`MenuSyncMsg`]s (sent on-change
/// only) and temporarily mutated by rollback-backed P1 menu predictions — the
/// exclusive source `Game::menu_read_model` renders from. Wire ids arrive
/// already remapped to local ids.
#[derive(Clone, Debug, Default)]
pub struct MenuView {
    /// The real output produced by the last accepted CRAFT request.
    pub craft_output: Option<ItemStack>,
    /// The open mod GUI's container slots.
    pub container: Option<ContainerView>,
    /// The open mod GUI's kind — resolves the document's slot semantics
    /// (take-only outputs) for click prediction against `container`.
    pub container_kind: Option<petramond_world::gui_state::GuiKind>,
    /// The open mod GUI's state map. Only replaced when a sync carries one
    /// (the server ships it on `Arc` change only).
    pub gui_state: Option<Arc<GuiStateMap>>,
}

fn stack_from_wire(slot: &Option<ItemSlotWire>) -> Option<ItemStack> {
    slot.as_ref().map(ItemSlotWire::to_stack)
}

impl MenuView {
    /// Adopt one on-change sync: the target view is replaced whole; the mod
    /// GUI state map is kept unless the sync carries a fresh one.
    pub fn apply(&mut self, msg: MenuSyncMsg) {
        self.craft_output = None;
        self.container = None;
        self.container_kind = None;
        match msg.target {
            MenuTargetWire::None => {
                self.gui_state = None;
            }
            MenuTargetWire::Crafting { output } => {
                self.gui_state = None;
                self.craft_output = stack_from_wire(&output);
            }
            MenuTargetWire::Container {
                kind_key,
                slots,
                gui_state,
                ..
            } => {
                self.container_kind = petramond_world::gui_state::resolve_kind(&kind_key);
                self.container = slots.map(|slots| ContainerView {
                    slots: slots.iter().map(stack_from_wire).collect(),
                });
                if let Some(entries) = gui_state {
                    self.gui_state = Some(Arc::new(
                        entries
                            .into_iter()
                            .map(|(k, v)| (k, v.into_value()))
                            .collect(),
                    ));
                } else if self.gui_state.is_none() {
                    // First sight of this session without a map yet: render
                    // from the shared empty map until a change ships one.
                    self.gui_state = Some(petramond_world::gui_state::empty_gui_state());
                }
            }
        }
    }

    /// Adopt ONLY the sync's mod GUI state map, keeping the slot views as
    /// they are. Used when the sync's slot state is stale against a pending
    /// menu prediction: gauges keep flowing (predictions never touch them,
    /// and a skipped map would be lost until its next change), while slot
    /// truth arrives with the pending request's own forced outcome batch.
    pub fn adopt_gui_state(&mut self, msg: MenuSyncMsg) {
        if let MenuTargetWire::Container {
            gui_state: Some(entries),
            ..
        } = msg.target
        {
            self.gui_state = Some(Arc::new(
                entries
                    .into_iter()
                    .map(|(k, v)| (k, v.into_value()))
                    .collect(),
            ));
        }
    }
}

/// Rebuild a real [`Inventory`] from the wire layout (36 slots, then the
/// cursor, then the off-hand LAST — the `SelfRestore`/`SelfState` layout).
/// Short/absent tails read empty. Also rebuilds the remote join's player.
pub fn inventory_from_wire(
    slots: &[Option<petramond::net::protocol::ItemSlotWire>],
    active: u8,
) -> Inventory {
    let mut grid: [Option<ItemStack>; petramond_world::inventory::TOTAL_SLOTS] =
        [None; petramond_world::inventory::TOTAL_SLOTS];
    for (dst, src) in grid.iter_mut().zip(slots.iter()) {
        *dst = src.as_ref().map(ItemSlotWire::to_stack);
    }
    let cursor = slots
        .get(petramond_world::inventory::TOTAL_SLOTS)
        .and_then(|s| s.as_ref())
        .map(ItemSlotWire::to_stack);
    let off_hand = slots
        .get(petramond_world::inventory::TOTAL_SLOTS + 1)
        .and_then(|s| s.as_ref())
        .map(ItemSlotWire::to_stack);
    Inventory::from_parts(grid, cursor, off_hand, active)
}

/// Interpolate a replicated ragdoll pose between two batches: positions lerp,
/// orientations slerp per bone. A fresh/mismatched previous pose (the ragdoll
/// just started, or a bone-count change) snaps to the current one.
pub fn lerp_ragdoll(
    prev: Option<&Vec<([f32; 3], [f32; 4])>>,
    curr: &[([f32; 3], [f32; 4])],
    alpha: f32,
) -> Vec<(Vec3, Quat)> {
    let to_pose = |&(p, q): &([f32; 3], [f32; 4])| (Vec3::from(p), Quat::from_array(q));
    match prev {
        Some(prev) if prev.len() == curr.len() => prev
            .iter()
            .zip(curr.iter())
            .map(|(a, b)| {
                let (pa, qa) = to_pose(a);
                let (pb, qb) = to_pose(b);
                (pa.lerp(pb, alpha), qa.slerp(qb, alpha))
            })
            .collect(),
        _ => curr.iter().map(to_pose).collect(),
    }
}

impl Game {
    /// Apply one pump's ordered server→client messages: terrain payloads into
    /// the REPLICA world, then the tick batch. A remote client applies the
    /// identical messages off the wire (remapped at its transport boundary).
    pub fn apply_server_messages(
        &mut self,
        msgs: &mut Vec<petramond::net::protocol::ServerToClient>,
    ) {
        use petramond::net::protocol::ServerToClient;
        debug_assert!(self.remote_section_installs.is_empty());
        for msg in msgs.drain(..) {
            match msg {
                ServerToClient::ColumnData(column) => self.replica.install_remote_column(column),
                ServerToClient::SectionData(section) => {
                    // A full payload supersedes any parked copy: the server
                    // only re-streams a claimed section when its content
                    // moved (or after a SectionCacheMiss dropped the belief).
                    self.section_cache.discard(section.pos);
                    if let Some(pos) = self.replica.install_remote_section_deferred(*section) {
                        self.remote_section_installs.push(pos);
                    }
                }
                ServerToClient::LightData(light) => self.replica.install_remote_light(light),
                ServerToClient::SectionUnload { pos, cache_hash } => {
                    let evicted = self.replica.uninstall_remote_section(pos);
                    if let (Some(section), Some(hash)) = (evicted, cache_hash) {
                        self.park_evicted_section(pos, section, hash);
                    }
                }
                ServerToClient::ColumnUnload { pos, cache_hashes } => {
                    for (sp, section) in self.replica.uninstall_remote_column(pos) {
                        if let Some(&(_, hash)) = cache_hashes.iter().find(|(cy, _)| *cy == sp.cy) {
                            self.park_evicted_section(sp, section, hash);
                        }
                    }
                }
                ServerToClient::SectionCached { pos, hash } => {
                    match self.section_cache.promote(pos, hash) {
                        Some(section) => {
                            let pos = self.replica.install_cached_section(pos, section);
                            self.remote_section_installs.push(pos);
                        }
                        // Like the batch ack, a miss reports through the
                        // handle right away (never the frame outbox): until
                        // the server re-streams the full payload this pos is
                        // a hole in the world.
                        None => {
                            if self
                                .handle
                                .send(petramond::net::protocol::ClientToServer::SectionCacheMiss {
                                    pos,
                                })
                                .is_err()
                            {
                                self.note_connection_lost();
                            }
                        }
                    }
                }
                ServerToClient::Tick(update) => self.apply_tick_update(update),
                // Roster changes (broadcast to every connection, local
                // included). The remote-player STORE keys off the per-tick
                // rows; the roster carries names (and survives even if a row
                // beats its PlayerJoined — the store refreshes names per
                // batch).
                ServerToClient::PlayerJoined { id, name } => {
                    self.player_roster.insert(id, name);
                }
                ServerToClient::PlayerLeft { id } => {
                    self.player_roster.remove(&id);
                }
                ServerToClient::ChatLine(line) => {
                    self.pending_chat_lines.push(line);
                }
                // The server is the only writer of the unlocked set; the
                // client mirrors it so its browser lists exactly what the
                // server would accept a CRAFT for.
                ServerToClient::RecipesUnlocked { recipes } => {
                    for recipe in recipes {
                        self.player.progression.unlock(&recipe);
                    }
                }
                // Streaming flow control: Start opens the timing window, End
                // closes it into a measured apply rate and an immediate ack
                // (both markers apply in THIS same drain loop, so the elapsed
                // time is the real cost of installing the batch's messages).
                ServerToClient::StreamBatchStart => {
                    self.stream_batch_started = Some(std::time::Instant::now());
                }
                ServerToClient::StreamBatchEnd { count } => self.ack_stream_batch(count),
                ServerToClient::KeepAlive => {}
                ServerToClient::ServerClosing => {
                    self.note_connection_lost_because("the server closed");
                }
                ServerToClient::Disconnect { reason } => {
                    self.note_connection_lost_because(&format!("disconnected: {reason}"));
                }
                // Handshake messages never reach a joined session.
                other => {
                    debug_assert!(false, "unexpected post-join message: {other:?}");
                }
            }
        }
        self.replica
            .finish_remote_install_batch(&self.remote_section_installs);
        self.remote_section_installs.clear();
    }

    /// Park one server-vouched evicted section in the section cache — unless
    /// a pending predicted edit touches it. The vouched hash covers the
    /// server's content at unload issue, which the ordered stream makes equal
    /// to the replica's copy at unload APPLY only when nothing local mutated
    /// it; an unconfirmed prediction breaks that, and a wrongly parked copy
    /// would re-promote as silent desync. Dropping instead costs one
    /// SectionCacheMiss round-trip if the section ever comes back.
    fn park_evicted_section(
        &mut self,
        pos: petramond_world::chunk::SectionPos,
        section: std::sync::Arc<petramond_world::section::Section>,
        hash: u64,
    ) {
        let predicted = self
            .prediction
            .predicted_cells()
            .chain(self.predicted_presentation_cells.iter().copied())
            .any(|c| petramond_world::chunk::SectionPos::from_world(c.x, c.y, c.z) == Some(pos));
        if !predicted {
            self.section_cache.park(pos, section, hash);
        }
    }

    /// Close the open batch window into a rate sample and ack it RIGHT AWAY
    /// through the handle (not the frame outbox: acks must flow even on
    /// frames that never reach `tick_send`, or the server's window starves).
    /// The EMA smooths per-batch noise; the server clamps whatever we report.
    fn ack_stream_batch(&mut self, count: u32) {
        let Some(started) = self.stream_batch_started.take() else {
            return; // End without Start: tolerate, nothing to measure
        };
        let elapsed = started.elapsed().as_secs_f32().max(1e-4);
        let sampled = count as f32 / elapsed;
        let rate = match self.stream_rate_ema {
            Some(ema) => ema * 0.75 + sampled * 0.25,
            None => sampled,
        };
        self.stream_rate_ema = Some(rate);
        if self
            .handle
            .send(petramond::net::protocol::ClientToServer::StreamBatchAck {
                messages_per_second: rate,
            })
            .is_err()
        {
            self.note_connection_lost();
        }
    }

    /// Adopt entity rows into the committed stores. Ordinary callers shift
    /// curr→prev; an overflow resync seeds prev == curr for every entity so a
    /// dropped backlog cannot become one segment of extreme-speed motion.
    /// The own row's mount adopts HERE — the local body slaves to the same
    /// committed pair every observer renders.
    fn apply_committed_rows(&mut self, staged: StagedRows) {
        let StagedRows {
            mobs,
            items,
            mut players,
            actions,
            resync,
        } = staged;
        let was_mounted = self.self_mount.is_some();
        if let Some(own) = players.iter().find(|row| row.id == self.self_id) {
            self.self_mount = own.mount;
        }
        if resync {
            self.replicated_mobs.resync(mobs);
            self.replicated_items.resync(items);
            for row in &mut players {
                row.snap = true;
            }
        } else {
            self.replicated_mobs.apply(mobs);
            self.replicated_items.apply(items);
        }
        self.remote_players.apply(&players, &actions, self.self_id);
        if was_mounted && self.self_mount.is_none() {
            self.predict_dismount_placement();
        }
    }

    /// Queue one post-bootstrap row snapshot. Overflow is a declared resync:
    /// retain only the newest state, but prepend every dropped batch's player
    /// actions in arrival order so one-shot animation triggers are not lost.
    fn stage_rows(&mut self, mut staged: StagedRows) {
        if self.staged_rows.len() >= MAX_STAGED_ROW_BATCHES {
            let action_count = self
                .staged_rows
                .iter()
                .map(|rows| rows.actions.len())
                .sum::<usize>()
                + staged.actions.len();
            let mut actions = Vec::with_capacity(action_count);
            for mut rows in self.staged_rows.drain(..) {
                actions.append(&mut rows.actions);
            }
            actions.append(&mut staged.actions);
            staged.actions = actions;
            staged.resync = true;
        }
        self.staged_rows.push_back(staged);
        debug_assert!(self.staged_rows.len() <= MAX_STAGED_ROW_BATCHES);
    }

    /// Mount→None edge: predict the SAME side-of-the-hull landing spot the
    /// server's riding pass chose, from the replica + interpolated rows. The
    /// local body was slaved INSIDE the hull's solid box; left there it keeps
    /// claiming that position, claim adoption keeps accepting it, and the
    /// body swims out through the boat. Prediction and authority run the one
    /// shared `dismount_spot`, so they converge without a visible correction.
    fn predict_dismount_placement(&mut self) {
        if self.player.is_spectator() {
            return;
        }
        let obstacles = self.solid_entity_obstacles();
        let spot = petramond::mob::riding::dismount_spot(
            self.player.pos,
            self.player.yaw,
            |feet| petramond::mob::riding::player_body_free(&self.replica, feet, &obstacles),
            |feet| {
                let c = petramond_math::math::voxel_at(feet);
                !self.replica.water_cell_at(c.x, c.y, c.z)
                    && !self.replica.water_cell_at(c.x, c.y - 1, c.z)
            },
        );
        if let Some(feet) = spot {
            self.player.teleport(feet);
        }
    }

    /// Turn the interpolation window when render time crossed the current
    /// segment: commit queued rows FIFO and consume exactly one crossed segment
    /// per batch; starved queues hold at the segment end. Outside the first
    /// bootstrap, this is the ONLY path that shifts committed prev/curr rows.
    /// Runs each frame right after the batches drained (`tick_receive`), before
    /// presentation samples `tick_alpha`.
    pub fn advance_interp_window(&mut self) {
        while self.replica_clock.overdue() {
            let Some(staged) = self.staged_rows.pop_front() else {
                break;
            };
            self.apply_committed_rows(staged);
            self.replica_clock.consume_segment();
        }
        if self.staged_rows.is_empty() {
            self.replica_clock.hold();
        }
    }

    /// Test-only: advance render time one full tick and turn the window —
    /// how row-assertion tests step the staged interpolation deterministically.
    #[cfg(test)]
    pub fn commit_replication_window_for_test(&mut self) {
        self.replica_clock
            .advance(crate::game::tick::TICK_DT * 1.001);
        self.advance_interp_window();
    }

    /// Adopt one replication batch: block deltas and client read models apply
    /// immediately, entity rows enter the interpolation FIFO, and this
    /// window's events translate to LOCAL types and buffer for `GameEvents`.
    pub fn apply_tick_update(&mut self, update: Box<TickUpdate>) {
        let update = *update;
        self.replicated_tick = update.tick;
        // The batch's written cells, collected before the deltas are consumed
        // (the rollback and place-ghost checks below both key on them).
        let delta_cells: rustc_hash::FxHashSet<IVec3> =
            update.block_deltas.iter().map(|d| d.pos).collect();
        for delta in update.block_deltas {
            self.replica.apply_remote_delta(delta);
        }
        // AFTER the block deltas for the same reason as the KV below: a block
        // write drops the cell's draw set on both sides, so a same-batch
        // write-block-then-draw sequence has to land in that order.
        for d in update.block_draws {
            self.replica.apply_remote_block_draw(d.pos, d.prims);
        }
        // AFTER the block deltas: a block write wipes the cell's KV on both
        // sides, so a same-batch write-block-then-KV sequence lands in order.
        for kv in update.cell_kv_deltas {
            self.replica.apply_remote_cell_kv(kv);
        }
        // Entity ROWS are STAGED, not applied: the committed prev→curr pair
        // under the render must never shift mid-segment (see `ReplicaClock`).
        // A bounded FIFO absorbs ordinary bursts; overflow collapses pending
        // state into the newest snapshot and preserves all player actions for
        // a boundary-only resync. Everything else in this update — deltas,
        // self state, events, menu — applies immediately: it is either an
        // authoritative correction or one-shot presentation, not interpolated
        // motion.
        let staged = StagedRows {
            mobs: update.mobs,
            items: update.items,
            players: update.players,
            actions: update.player_actions,
            resync: false,
        };
        if !self.replica_clock.started() {
            // Bootstrap: the first batch renders directly (prev == curr —
            // there is nothing to interpolate from), without pretending a
            // render-time segment was crossed, and starts the timeline.
            debug_assert!(self.staged_rows.is_empty());
            self.apply_committed_rows(staged);
            self.replica_clock.start();
        } else {
            self.stage_rows(staged);
        }
        // A batch's inventory / menu snapshot reflects the server state as of
        // the requests it ANSWERS. A snapshot-bearing prediction of the same
        // store that stays pending past this batch postdates that snapshot —
        // adopting it would visibly regress the newer prediction for one RTT
        // (the rapid-click flicker). Skip adoption then: every predicted menu
        // request forces the authoritative pair into its own outcome batch,
        // so the store reconciles the moment the pending queue drains.
        let stale_inventory = self
            .prediction
            .awaits_inventory_authority(&update.action_outcomes);
        let stale_menu = self
            .prediction
            .awaits_menu_authority(&update.action_outcomes);
        let adopted_inventory = !stale_inventory
            && update
                .self_state
                .as_ref()
                .is_some_and(|s| s.inventory.is_some());
        let adopted_menu = !stale_menu && update.menu_sync.is_some();
        if let Some(state) = &update.self_state {
            self.self_view.apply(state, !stale_inventory);
            if self.player.mode() != self.self_view.mode {
                self.player.set_mode(self.self_view.mode);
            }
            // The predicted body runs the same movement code as the server,
            // and that code reads the effect list (a speed effect scales land
            // speed). An unsynced predicted player would walk at the wrong
            // speed for the whole duration and rubber-band every batch.
            self.player.set_effects(
                self.self_view
                    .effects
                    .iter()
                    .map(
                        |&(effect, remaining)| petramond_world::effect::ActiveEffect {
                            effect,
                            remaining,
                        },
                    )
                    .collect(),
            );
            // Same rule for the resolved mod body — the speed scale the
            // movement code reads every step, and the actions this body is
            // barred from. Both are authority, so the predicted body adopts
            // them with the effects.
            self.player
                .adopt_resolved_body(self.self_view.move_scale, self.self_view.denied_actions);
            // Tick-side transform mutations (teleports, knockback) win over
            // the local prediction — per-field against what we last sent.
            if let Some(t) = &state.transform {
                self.adopt_authoritative_transform(t);
            }
        }
        // Snapshot predicted cells BEFORE reconcile so accept/deny this batch
        // still suppress matching wire presentation events (the ledger entry
        // is about to drop).
        let suppress: rustc_hash::FxHashSet<IVec3> = self
            .prediction
            .predicted_cells()
            .chain(self.predicted_presentation_cells.iter().copied())
            .collect();
        // Authoritative inventory / block deltas win; then apply deny rollbacks
        // for any predicted mutations the server rejected. Snapshots come back
        // oldest-first, each capturing the state BEFORE its own prediction —
        // so a newer snapshot still embeds an older denied mutation. Applied
        // newest-first so the OLDEST snapshot wins.
        let (rollbacks, resolved_cells) = self.prediction.reconcile(&update.action_outcomes);
        for pos in &resolved_cells {
            self.predicted_presentation_cells.remove(pos);
        }
        for snap in rollbacks.into_iter().rev() {
            match snap {
                crate::game::prediction::PredictionSnapshot::None => {}
                crate::game::prediction::PredictionSnapshot::Inventory(inv) => {
                    // Only restore if we did not adopt a fresh authoritative
                    // body this batch (adopted SelfState inventory wins).
                    if !adopted_inventory {
                        self.self_view.inventory = inv;
                    }
                }
                crate::game::prediction::PredictionSnapshot::Menu { inventory, menu } => {
                    if !adopted_inventory {
                        self.self_view.inventory = inventory;
                    }
                    if !adopted_menu {
                        self.menu_view = menu;
                    }
                }
                crate::game::prediction::PredictionSnapshot::World { inventory, cells } => {
                    if let Some(inv) = inventory {
                        if !adopted_inventory {
                            self.self_view.inventory = inv;
                        }
                    }
                    // Silent restore: no world events. A same-batch
                    // authoritative delta at a cell wins over the rollback.
                    let mut restored = Vec::with_capacity(cells.len());
                    for (pos, prev_block_id) in cells {
                        if delta_cells.contains(&pos) {
                            continue;
                        }
                        let before = self.replica.chunk_block(pos.x, pos.y, pos.z);
                        let _ = self.replica.set_block_world(
                            pos.x,
                            pos.y,
                            pos.z,
                            petramond_world::block::Block::from_id(prev_block_id),
                        );
                        restored.push((pos, before));
                        if self.place_ghost.is_some_and(|(p, _)| p == pos) {
                            self.place_ghost = None;
                        }
                    }
                    // A rollback is a local edit too: its restored geometry
                    // and light publish under the same prediction fence.
                    self.replica.reconcile_predicted_edit(&restored);
                }
            }
        }
        if let Some((pos, _)) = self.place_ghost {
            if delta_cells.contains(&pos) {
                self.place_ghost = None;
            }
        }
        // Shader-param environment (day/night sky, mod visuals): applied into
        // the REPLICA world's `WorldEnvironment` — the map the renderer reads
        // (`Game::environment` snapshots `replica.environment()` per frame).
        // `None` = unchanged since the last batch.
        if let Some(env) = update.env {
            for (key, value) in env {
                self.replica.set_shader_param(key, value);
            }
        }
        self.open_chests = update.open_chests.into_iter().collect();
        if let Some(sync) = update.menu_sync {
            if stale_menu {
                self.menu_view.adopt_gui_state(sync);
            } else {
                self.menu_view.apply(sync);
            }
        }
        for msg in update.events {
            self.buffer_world_event(msg, &suppress);
        }
        self.pending_events
            .self_events
            .merge_from(update.self_events);
    }

    /// Translate one wire world event to local types into the frame buffer.
    /// Ids arrived remapped (identity in-process), so constructors are direct.
    ///
    /// Own predicted place/break presentation is NEVER replayed: `suppress`
    /// holds every cell this client already presented (or still has pending).
    /// Observers' / natural breaks still present. Server-side strip is the
    /// primary filter; this is the belt for races.
    fn buffer_world_event(&mut self, msg: WorldEventMsg, suppress: &rustc_hash::FxHashSet<IVec3>) {
        use crate::game::tick::{MobSoundEvent, SoundEvent, SpatialSoundCommand};
        let ev = &mut self.pending_events;
        match msg {
            WorldEventMsg::BlockBroken {
                pos,
                block_id,
                normal,
                tint,
            } => {
                if suppress.contains(&pos) {
                    return;
                }
                ev.world.push(WorldEvent::BlockBroken {
                    pos,
                    block: petramond_world::block::Block::from_id(block_id),
                    normal,
                    tint,
                });
            }
            WorldEventMsg::BlockPlaced { pos, block_id } => {
                if suppress.contains(&pos) {
                    return;
                }
                ev.world.push(WorldEvent::BlockPlaced {
                    pos,
                    block: petramond_world::block::Block::from_id(block_id),
                });
            }
            WorldEventMsg::DoorToggled { lower, open } => {
                ev.world.push(WorldEvent::DoorToggled { lower, open })
            }
            WorldEventMsg::ChestOpened { pos } => ev.world.push(WorldEvent::ChestOpened { pos }),
            WorldEventMsg::ChestClosed { pos } => ev.world.push(WorldEvent::ChestClosed { pos }),
            WorldEventMsg::ItemPickedUp { pos, by } => ev.world.push(WorldEvent::ItemPickedUp {
                pos,
                by_self: by == self.self_id,
            }),
            WorldEventMsg::MobSound {
                mob_id,
                kind_id,
                category,
                pos,
            } => ev.mob_sounds.push(MobSoundEvent {
                mob_id,
                kind: petramond::mob::Mob(kind_id),
                category: petramond::mob::MobSoundCategory::from_u8(category),
                pos,
            }),
            WorldEventMsg::Sound { sound_id, pos } => ev.sounds.push(SoundEvent {
                sound: petramond_world::sound_registry::Sound(sound_id),
                pos,
            }),
            WorldEventMsg::EmitterBurst {
                emitter_id,
                pos,
                intensity,
            } => ev.world.push(WorldEvent::EmitterBurst {
                emitter: emitter_id,
                pos,
                intensity,
            }),
            WorldEventMsg::SpatialSound(cmd) => ev.spatial_sounds.push(match cmd {
                SpatialSoundMsg::PlayAt {
                    handle,
                    sound_id,
                    pos,
                    volume,
                    pitch,
                } => SpatialSoundCommand::PlayAt {
                    handle,
                    sound: petramond_world::sound_registry::Sound(sound_id),
                    pos,
                    volume,
                    pitch,
                },
                SpatialSoundMsg::PlayOnMob {
                    handle,
                    sound_id,
                    mob_id,
                    volume,
                    pitch,
                    last_pos,
                } => SpatialSoundCommand::PlayOnMob {
                    handle,
                    sound: petramond_world::sound_registry::Sound(sound_id),
                    mob_id,
                    volume,
                    pitch,
                    last_pos,
                },
                SpatialSoundMsg::Set {
                    handle,
                    volume,
                    pitch,
                } => SpatialSoundCommand::Set {
                    handle,
                    volume,
                    pitch,
                },
                SpatialSoundMsg::Stop { handle } => SpatialSoundCommand::Stop { handle },
            }),
        }
    }
}
