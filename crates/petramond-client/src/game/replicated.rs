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
//! The server replicates only the entities in this client's interest, as
//! per-kind [`EntityLane`](petramond::net::protocol::EntityLane)s: an entity lives in its store from its spawn to
//! its despawn — which is how leaving view differs from absence in one batch
//! — and dying is presented from its rows (`dead`, ragdoll) while it is
//! still tracked.
//! Light is deliberately absent from the rows: the client samples it at the
//! entity position from its REPLICA world.

use std::sync::Arc;

use glam::{Quat, Vec3};
use petramond_render::{AnimId, AnimInterner, AnimNames, GaitClip};

use petramond_world::gui_state::ContainerView;

use petramond::net::protocol::{
    ItemLane, ItemSlotWire, ItemStateRow, MenuSyncMsg, MenuTargetWire, MobLane, MobStateRow,
    PlayerActionKind, PlayerLane, PlayerMount, SelfState, SpatialSoundMsg, TickSection, TickUpdate,
    WorldEventMsg,
};

use crate::game::presentation::BodyEmitters;
use petramond::player::PlayerId;
use petramond::player::{Player, PlayerMode};
use petramond_math::math::{IVec3, Tilt};
use petramond_world::gui_state::GuiStateMap;
use petramond_world::inventory::{Hand, Inventory};
use petramond_world::item::{ItemStack, ItemType};

use super::tick::WorldEvent;
use super::Game;

mod entity_replica;
mod entity_store;
pub use entity_replica::{Committed, EntityReplica};
pub(crate) use entity_store::{Adopt, AssignFrom, EntityStore, Replica};

/// One tick window's entity lanes and player actions.
pub struct EntityWindow {
    // Shared straight off the batch: each lane selects rows out of tables
    // the server builds once per tick window, and the FIFO holds up to four
    // windows, so staging them is refcount bumps and index lists rather than
    // deep copies of every tracked entity.
    pub mobs: MobLane,
    pub items: ItemLane,
    pub players: PlayerLane,
    pub actions: Arc<[(PlayerId, PlayerActionKind)]>,
}

/// One `TickUpdate`'s entity rows, STAGED until render time crosses into
/// their segment (see [`ReplicaClock`](super::tick::ReplicaClock)): the
/// committed prev→curr pair under the render never shifts mid-segment, which
/// is what keeps interpolated motion — and a rider's camera glued to it —
/// free of arrival-jitter rubber-banding.
pub struct StagedRows {
    window: EntityWindow,
    /// Newer windows an overflow folded into this entry, oldest first. The
    /// fold keeps each window's shared lanes instead of merging their rows,
    /// so collapsing a backlog copies no row: the commit replays the windows
    /// in order, which composes exactly like the lanes do.
    folded: Vec<EntityWindow>,
    /// An overflow folded older pending windows into this one. Its boundary
    /// commit must seed prev == curr rather than lerp over the dropped gap.
    resync: bool,
}

impl StagedRows {
    pub fn new(window: EntityWindow) -> Self {
        Self {
            window,
            folded: Vec::new(),
            resync: false,
        }
    }

    /// Every window this entry commits, oldest first.
    pub fn windows(&self) -> impl Iterator<Item = &EntityWindow> {
        std::iter::once(&self.window).chain(&self.folded)
    }
}

/// The lane a FULL-snapshot sender would ship for `rows` against a store
/// holding `held`: every held id missing from the snapshot despawns, every
/// row is an update. How tests drive the stores with plain row lists.
#[cfg(test)]
pub(super) fn snapshot_lane<R: petramond::net::protocol::EntityRow>(
    held: impl Iterator<Item = R::Id>,
    rows: &[R],
) -> petramond::net::protocol::EntityLane<R, R::Id> {
    petramond::net::protocol::EntityLane {
        despawned: held
            .filter(|id| !rows.iter().any(|row| row.entity_id() == *id))
            .collect(),
        spawned: Default::default(),
        updated: rows.to_vec().into(),
    }
}

/// Normal scheduling jitter needs only a few pending ticks. If a stalled
/// client exceeds this depth, staging collapses deterministically into one
/// resync entry committed at the next boundary, instead of either queueing
/// one segment per window or mutating the committed interpolation pair
/// mid-segment.
pub const MAX_STAGED_ROW_BATCHES: usize = 4;

/// How fast a named mob animation blends in/out (weight per second): ~0.17 s
/// to full — an oar picks up and settles instead of snapping between poses.
const ANIM_BLEND_PER_SEC: f32 = 6.0;

/// One replicated mob: the previous and current batch rows, keyed by the
/// mob's stable id in [`ReplicatedMobs`].
pub struct ReplicatedMob {
    pub prev: MobStateRow,
    pub curr: MobStateRow,
    /// `prev.anims` / `curr.anims` with each name interned into the store's
    /// session [`AnimInterner`], in row order — interned when a row's set
    /// changes, so no frame clones or compares a name.
    prev_anims: Vec<(AnimId, f32)>,
    curr_anims: Vec<(AnimId, f32)>,
    /// CLIENT-side blend state over the replicated named animations
    /// (`curr_anims` are the target set): `(anim, weight, phase)` — the
    /// weight eases per frame toward 1 for active layers and 0 for dropped
    /// ones (layers fade instead of snapping), and `phase` holds the last
    /// replicated phase so a fading-OUT layer keeps its pose. Presentation
    /// state only, advanced by [`ReplicatedMobs::advance_anim_blends`].
    pub anim_blend: Vec<(AnimId, f32, f32)>,
    /// The same blend over the BASE gait the row's locomotion selects (walk,
    /// an idle, or none = rest): `(clip, weight, phase)`. A body eases into
    /// and out of its gait like any layer, however briefly it steps.
    pub gait_blend: Vec<(GaitClip, f32, f32)>,
    /// `curr.draw` resolved for the renderer, rebuilt only when the row's
    /// set changes.
    pub draw: Option<petramond::world::draw::BlockDraw>,
    /// `curr`'s body emitters and the tint and self-light they compose,
    /// re-derived only when its attached bundles or conditions change.
    emitters: BodyEmitters,
}

/// A row's draw set resolved for the renderer (`None` = draws nothing).
fn resolve_draw(row: &MobStateRow) -> Option<petramond::world::draw::BlockDraw> {
    (!row.draw.prims.is_empty()).then(|| {
        std::sync::Arc::new(petramond::world::draw::BlockDrawSet::new(
            row.draw.prims.clone(),
        ))
    })
}

/// Whether two rows wear the same draw set. In process the server's shared
/// prims arrive by refcount, so an unchanged set is the SAME allocation and
/// this is a pointer compare; only a set decoded afresh off the wire falls
/// back to comparing its (handful of) prims, which never allocates.
fn same_draw(a: &petramond::world::draw::DrawPrims, b: &petramond::world::draw::DrawPrims) -> bool {
    Arc::ptr_eq(&a.0, &b.0) || (a.is_empty() && b.is_empty()) || a == b
}

/// `anims` with each name interned, into `out`. A name the previous row had
/// at the same index reuses that row's id, so a steady layer set never
/// touches the interner; any other name interns.
fn intern_anims(
    anims: &[(String, f32)],
    prev_names: &[(String, f32)],
    prev_ids: &[(AnimId, f32)],
    interner: &mut AnimInterner,
    out: &mut Vec<(AnimId, f32)>,
) {
    out.clear();
    out.extend(anims.iter().enumerate().map(|(i, (name, phase))| {
        let id = match (prev_names.get(i), prev_ids.get(i)) {
            (Some((prev, _)), Some(&(id, _))) if prev == name => id,
            _ => interner.intern(name),
        };
        (id, *phase)
    }));
}

/// The gait a replicated row is in, or `None` at rest.
pub fn gait_of(row: &MobStateRow) -> Option<GaitClip> {
    if row.moving {
        Some(GaitClip::Walk)
    } else {
        row.idle_anim.map(GaitClip::Idle)
    }
}

impl Replica<MobStateRow> for ReplicatedMob {
    type Ctx = AnimInterner;

    /// A freshly tracked mob: prev == curr, and its animations start at FULL
    /// weight (a mob streamed in mid-row must not fade in from rest).
    fn spawn(row: &MobStateRow, interner: &mut AnimInterner) -> Self {
        let mut mob = ReplicatedMob {
            prev: row.clone(),
            curr: row.clone(),
            prev_anims: Vec::new(),
            curr_anims: Vec::new(),
            anim_blend: Vec::new(),
            gait_blend: Vec::new(),
            draw: resolve_draw(row),
            emitters: BodyEmitters::default(),
        };
        intern_anims(&row.anims, &[], &[], interner, &mut mob.curr_anims);
        mob.prev_anims.clone_from(&mob.curr_anims);
        mob.emitters.refresh(&row.emitters, &row.conditions);
        mob.settle_blends();
        mob
    }

    /// Adopt the next row: curr→prev in place (the retired prev row takes the
    /// new one's contents), keeping the blend state (it eases toward the new
    /// target set) and the resolved draw set while unchanged.
    fn advance(&mut self, row: &MobStateRow, interner: &mut AnimInterner) {
        if !same_draw(&self.curr.draw.prims, &row.draw.prims) {
            self.draw = resolve_draw(row);
        }
        // `prev_anims` takes the outgoing ids, which still pair with
        // `curr.anims`' names until the rows swap below.
        std::mem::swap(&mut self.prev_anims, &mut self.curr_anims);
        intern_anims(
            &row.anims,
            &self.curr.anims,
            &self.prev_anims,
            interner,
            &mut self.curr_anims,
        );
        std::mem::swap(&mut self.prev, &mut self.curr);
        self.curr.assign_from(row);
        self.emitters.refresh(&row.emitters, &row.conditions);
    }

    /// A fresh seed in place: prev == curr == `row`, animations at full
    /// weight, like a spawn but reusing this entry's buffers.
    fn reseed(&mut self, row: &MobStateRow, interner: &mut AnimInterner) {
        self.advance(row, interner);
        self.hold();
        self.settle_blends();
    }

    /// An unchanged window: the pair collapses onto the current row.
    fn hold(&mut self) {
        self.prev.assign_from(&self.curr);
        self.prev_anims.clone_from(&self.curr_anims);
    }
}

impl ReplicatedMob {
    /// Every current layer and the current gait at full weight.
    fn settle_blends(&mut self) {
        self.anim_blend.clear();
        self.anim_blend
            .extend(self.curr_anims.iter().map(|&(id, phase)| (id, 1.0, phase)));
        self.gait_blend.clear();
        self.gait_blend
            .extend(gait_of(&self.curr).map(|clip| (clip, 1.0, self.curr.anim_time)));
    }

    /// `prev.anims` with interned names, in row order.
    pub fn prev_anims(&self) -> &[(AnimId, f32)] {
        &self.prev_anims
    }

    /// `curr.anims` with interned names, in row order.
    pub fn curr_anims(&self) -> &[(AnimId, f32)] {
        &self.curr_anims
    }

    /// `curr`'s body emitters, as derived when the row last changed them.
    pub fn emitters(&self) -> &BodyEmitters {
        &self.emitters
    }

    /// The feet pose this replicated row presents at `alpha`. Picking,
    /// collision, seats, and rendering all speak this same prev→curr blend;
    /// keeping the shortest-arc yaw rule here prevents interaction geometry
    /// from drifting onto the future tick while the model is still between.
    pub fn interpolated_pose(&self, alpha: f32) -> (petramond_math::world_pos::WorldPos, f32) {
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
    pub seat: petramond_math::world_pos::WorldPos,
    pub body_yaw: f32,
    pub tilt: Tilt,
}

/// The client's replicated mob set, and the session's animation-name
/// interner its rows' layer names intern into.
#[derive(Default)]
pub struct ReplicatedMobs {
    rows: EntityStore<u64, ReplicatedMob>,
    anim_names: AnimInterner,
}

impl ReplicatedMobs {
    /// Apply one lane (see [`EntityStore::apply`]): a spawn starts with
    /// prev == curr (no interpolation from nowhere), an update shifts
    /// curr→prev and adopts the new row in place, and a despawn — out of
    /// view, or gone from the world — drops the id.
    pub fn apply(&mut self, lane: &MobLane) {
        self.rows
            .apply(lane, Adopt::Advance, &mut self.anim_names, |_| true);
    }

    /// [`apply`](Self::apply) a full row snapshot (see [`snapshot_lane`]).
    #[cfg(test)]
    pub fn apply_snapshot(&mut self, rows: &[MobStateRow]) {
        let lane = snapshot_lane(self.rows.keys(), rows);
        self.apply(&lane);
    }

    /// Replace a discontinuous backlog with one fresh interpolation seed:
    /// every row the lane carries starts over as a spawn would.
    fn resync(&mut self, lane: &MobLane) {
        self.rows
            .apply(lane, Adopt::Reseed, &mut self.anim_names, |_| true);
    }

    /// The session's interned animation names, which every entry's
    /// [`AnimId`]s index.
    pub fn anim_names(&self) -> &AnimNames {
        self.anim_names.names()
    }

    /// Ease every entry's animation blend weights toward its replicated
    /// target set (in → 1, out → 0, dropped at 0), refreshing each active
    /// layer's held phase from the row (a fading-out layer keeps its last
    /// pose). Runs once per frame.
    pub fn advance_anim_blends(&mut self, dt: f32) {
        let step = ANIM_BLEND_PER_SEC * dt;
        for entry in self.rows.iter_mut() {
            for &(id, phase) in &entry.curr_anims {
                if !entry.anim_blend.iter().any(|(n, _, _)| *n == id) {
                    entry.anim_blend.push((id, 0.0, phase));
                }
            }
            let target = &entry.curr_anims;
            for (id, weight, phase) in entry.anim_blend.iter_mut() {
                match target.iter().find(|(n, _)| *n == *id) {
                    Some((_, row_phase)) => {
                        *weight = (*weight + step).min(1.0);
                        *phase = *row_phase;
                    }
                    None => *weight -= step,
                }
            }
            entry.anim_blend.retain(|(_, w, _)| *w > 0.0);
            let gait = gait_of(&entry.curr);
            if let Some(clip) = gait {
                if !entry.gait_blend.iter().any(|(c, _, _)| *c == clip) {
                    entry.gait_blend.push((clip, 0.0, entry.curr.anim_time));
                }
            }
            for (clip, weight, phase) in entry.gait_blend.iter_mut() {
                if Some(*clip) == gait {
                    *weight = (*weight + step).min(1.0);
                    *phase = entry.curr.anim_time;
                } else {
                    *weight -= step;
                }
            }
            entry.gait_blend.retain(|(_, w, _)| *w > 0.0);
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = &ReplicatedMob> {
        self.rows.iter()
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
    rows: EntityStore<u64, ReplicatedItem>,
}

impl Replica<ItemStateRow> for ReplicatedItem {
    type Ctx = ();

    fn spawn(row: &ItemStateRow, _: &mut ()) -> Self {
        ReplicatedItem {
            prev: row.clone(),
            curr: row.clone(),
        }
    }

    fn advance(&mut self, row: &ItemStateRow, _: &mut ()) {
        std::mem::swap(&mut self.prev, &mut self.curr);
        self.curr.assign_from(row);
    }

    fn reseed(&mut self, row: &ItemStateRow, _: &mut ()) {
        self.curr.assign_from(row);
        self.prev.assign_from(row);
    }

    fn hold(&mut self) {
        self.prev.assign_from(&self.curr);
    }
}

impl ReplicatedItems {
    pub fn apply(&mut self, lane: &ItemLane) {
        self.rows.apply(lane, Adopt::Advance, &mut (), |_| true);
    }

    /// Replace a discontinuous backlog with one fresh interpolation seed.
    fn resync(&mut self, lane: &ItemLane) {
        self.rows.apply(lane, Adopt::Reseed, &mut (), |_| true);
    }

    pub fn iter(&self) -> impl Iterator<Item = &ReplicatedItem> {
        self.rows.iter()
    }
}

/// The client-side mirror of the local player's [`SelfState`]: everything the
/// HUD, hand, and overlays read. Seeded from the session at join (the wire
/// path seeds it from `SelfRestore`), then overwritten by every batch.
pub struct SelfView {
    /// Active body-condition stages `(condition id, stage)`.
    pub conditions: Vec<(u8, u8)>,
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
    /// The body's resolved animator claims — the authoritative answer,
    /// overridden per key by a client mod claiming it (see
    /// `ClientModRuntime::local_animator`).
    pub animator: petramond::player::AnimatorClaims,
}

impl SelfView {
    /// Seed from the freshly-restored session player at world open — the
    /// in-process stand-in for the join handshake's `SelfRestore`, so the HUD
    /// is right on the very first frame (before any tick has run).
    pub fn seed_from(player: &Player) -> Self {
        Self {
            health: player.health(),
            conditions: player
                .conditions()
                .active()
                .iter()
                .map(|c| (c.condition.0, c.stage()))
                .collect(),
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
            animator: player.claims.animator().clone(),
        }
    }

    /// Adopt one batch's self state. `adopt_inventory` is false when the
    /// batch's inventory snapshot is stale against a pending prediction (see
    /// `apply_tick_update`): contents and revision then keep the predicted
    /// view — the pending request's own outcome batch carries the truth.
    pub fn apply(&mut self, state: &SelfState, adopt_inventory: bool) {
        self.health = state.health;
        self.conditions.clone_from(&state.conditions);
        self.mode = PlayerMode::from_u8(state.mode);
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
        self.animator.clone_from(&state.animator);
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
/// just started, or a bone-count change) snaps to the current one. The bones
/// append to `out`, the frame's arena.
pub fn lerp_ragdoll(
    prev: Option<&[([f32; 3], [f32; 4])]>,
    curr: &[([f32; 3], [f32; 4])],
    alpha: f32,
    out: &mut Vec<(Vec3, Quat)>,
) {
    let to_pose = |&(p, q): &([f32; 3], [f32; 4])| (Vec3::from(p), Quat::from_array(q));
    match prev {
        Some(prev) if prev.len() == curr.len() => {
            out.extend(prev.iter().zip(curr.iter()).map(|(a, b)| {
                let (pa, qa) = to_pose(a);
                let (pb, qb) = to_pose(b);
                (pa.lerp(pb, alpha), qa.slerp(qb, alpha))
            }))
        }
        _ => out.extend(curr.iter().map(to_pose)),
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
        debug_assert!(self.replica.section_installs.is_empty());
        for msg in msgs.drain(..) {
            match msg {
                ServerToClient::ColumnData(column) => {
                    self.replica.world.install_remote_column(column)
                }
                ServerToClient::SectionData(section) => {
                    // A full payload supersedes any parked copy: the server
                    // only re-streams a claimed section when its content
                    // moved (or after a SectionCacheMiss dropped the belief).
                    self.replica.section_cache.discard(section.pos);
                    if let Some(pos) = self.replica.world.install_remote_section_deferred(*section)
                    {
                        self.replica.section_installs.push(pos);
                    }
                }
                ServerToClient::LightData(light) => self.replica.world.install_remote_light(light),
                ServerToClient::SectionUnload { pos, cache_hash } => {
                    let evicted = self.replica.world.uninstall_remote_section(pos);
                    if let (Some(section), Some(hash)) = (evicted, cache_hash) {
                        self.park_evicted_section(pos, section, hash);
                    }
                }
                ServerToClient::ColumnUnload { pos, cache_hashes } => {
                    for (sp, section) in self.replica.world.uninstall_remote_column(pos) {
                        if let Some(&(_, hash)) = cache_hashes.iter().find(|(cy, _)| *cy == sp.cy) {
                            self.park_evicted_section(sp, section, hash);
                        }
                    }
                }
                ServerToClient::SectionCached { pos, hash } => {
                    match self.replica.section_cache.promote(pos, hash) {
                        Some(section) => {
                            let pos = self.replica.world.install_cached_section(pos, section);
                            self.replica.section_installs.push(pos);
                        }
                        // Like the batch ack, a miss reports through the
                        // handle right away (never the frame outbox): until
                        // the server re-streams the full payload this pos is
                        // a hole in the world.
                        None => self.net.send_now(
                            petramond::net::protocol::ClientToServer::SectionCacheMiss { pos },
                        ),
                    }
                }
                ServerToClient::Tick(update) => self.apply_tick_update(update),
                // Roster changes (broadcast to every connection, local
                // included). The remote-player STORE keys off the per-tick
                // rows; the roster carries names (and survives even if a row
                // beats its PlayerJoined — the store refreshes names per
                // batch).
                ServerToClient::PlayerJoined { id, name } => {
                    self.replica.entities.player_joined(id, name);
                }
                ServerToClient::PlayerLeft { id } => {
                    self.replica.entities.player_left(id);
                }
                ServerToClient::ChatLine(line) => {
                    self.replica.chat_lines.push(line);
                }
                // The server is the only writer of the unlocked set; the
                // client mirrors it so its browser lists exactly what the
                // server would accept a CRAFT for.
                ServerToClient::RecipesUnlocked { recipes } => {
                    for recipe in recipes {
                        self.local.player.progression.unlock(&recipe);
                    }
                }
                ServerToClient::ModsDisabled { mods } => {
                    self.client_mods.disable_from_server(&mods);
                }
                // Streaming flow control: Start opens the timing window, End
                // closes it into a measured apply rate and an immediate ack
                // (both markers apply in THIS same drain loop, so the elapsed
                // time is the real cost of installing the batch's messages).
                ServerToClient::StreamBatchStart => self.net.stream_batch_started(),
                ServerToClient::StreamBatchEnd { count } => {
                    self.replica
                        .world
                        .finish_remote_install_batch(&self.replica.section_installs);
                    self.replica.section_installs.clear();
                    self.net.stream_batch_ended(count);
                }
                ServerToClient::KeepAlive => {}
                ServerToClient::ServerClosing => {
                    self.net.note_lost_because("the server closed");
                }
                ServerToClient::Disconnect { reason } => {
                    self.net
                        .note_lost_because(&format!("disconnected: {reason}"));
                }
                // Handshake messages never reach a joined session.
                other => {
                    debug_assert!(false, "unexpected post-join message: {other:?}");
                }
            }
        }
        self.replica
            .world
            .finish_remote_install_batch(&self.replica.section_installs);
        self.replica.section_installs.clear();
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
            .suppressed_cells()
            .any(|c| petramond_world::chunk::SectionPos::from_world(c.x, c.y, c.z) == Some(pos));
        if !predicted {
            self.replica.section_cache.park(pos, section, hash);
        }
    }

    /// Mount→None edge: predict the SAME side-of-the-hull landing spot the
    /// server's riding pass chose, from the replica + interpolated rows. The
    /// local body was slaved INSIDE the hull's solid box; left there it keeps
    /// claiming that position, claim adoption keeps accepting it, and the
    /// body swims out through the boat. Prediction and authority run the one
    /// shared `dismount_spot`, so they converge without a visible correction.
    fn predict_dismount_placement(&mut self) {
        if self.local.player.is_spectator() {
            return;
        }
        let obstacles = self.replica.solid_entity_obstacles();
        let spot = petramond::mob::riding::dismount_spot(
            self.local.player.pos,
            self.local.player.yaw,
            |feet| {
                petramond::mob::riding::player_body_free(
                    self.replica.world.data(),
                    feet,
                    &obstacles,
                )
            },
            |feet| petramond::mob::riding::dismount_footing_safe(self.replica.world.data(), feet),
        );
        if let Some(feet) = spot {
            self.local.player.teleport(feet);
        }
    }

    /// Turn the interpolation window when render time crossed the current
    /// segment: commit queued rows FIFO, one crossed segment per batch (see
    /// [`EntityReplica::commit_next_due`]); a commit that dismounted the local
    /// body lands it beside the hull. Runs each frame right after the batches
    /// drained (`tick_receive`), before presentation samples `tick_alpha`.
    pub fn advance_interp_window(&mut self) {
        while let Some(committed) = self.replica.entities.commit_next_due() {
            self.after_commit(committed);
        }
    }

    fn after_commit(&mut self, committed: Committed) {
        if committed.dismounted {
            self.predict_dismount_placement();
        }
    }

    /// Test-only: advance render time one full tick and turn the window —
    /// how row-assertion tests step the staged interpolation deterministically.
    #[cfg(test)]
    pub fn commit_replication_window_for_test(&mut self) {
        self.replica
            .entities
            .advance_clock(crate::game::tick::TICK_DT * 1.001);
        self.advance_interp_window();
    }

    /// Adopt one replication batch: block deltas and client read models apply
    /// immediately, entity rows enter the interpolation FIFO, and this
    /// window's events translate to LOCAL types and buffer for `GameEvents`.
    #[allow(clippy::boxed_local)] // The network message already owns a boxed tick payload.
    pub fn apply_tick_update(&mut self, update: Box<TickUpdate>) {
        let TickUpdate {
            tick,
            clock: _,
            sections,
        } = *update;
        self.replica.entities.set_tick(tick);
        let mut rows = EntityWindow {
            mobs: MobLane::default(),
            items: ItemLane::default(),
            players: PlayerLane::default(),
            actions: Vec::new().into(),
        };
        let mut sleep_tally = None;
        let mut delta_cells = rustc_hash::FxHashSet::<IVec3>::default();
        let mut outcomes = Vec::new();
        let mut self_state = None;
        let mut menu_sync = None;
        let mut open_chests = Vec::new();
        let mut world_events = Vec::new();
        for section in sections {
            match section {
                TickSection::Creative(replies) => self.receive_creative_replies(replies),
                TickSection::Schematics(notices) => self.receive_schematic_notices(notices),
                TickSection::BlockDeltas(deltas) => {
                    delta_cells.extend(deltas.iter().map(|d| d.pos));
                    self.note_ghost_changes(deltas.iter().map(|d| d.pos));
                    for delta in deltas {
                        self.replica.world.apply_remote_delta(delta);
                    }
                }
                TickSection::BlockDraws(draws) => {
                    for d in draws {
                        self.replica.world.apply_remote_block_draw(d.pos, d.prims);
                    }
                }
                TickSection::CellKvDeltas(deltas) => {
                    for delta in deltas {
                        self.replica.world.apply_remote_cell_kv(delta);
                    }
                }
                TickSection::Mobs(lane) => rows.mobs = lane,
                TickSection::Items(lane) => rows.items = lane,
                TickSection::Players(lane) => rows.players = lane,
                TickSection::PlayerActions(actions) => rows.actions = actions,
                TickSection::SleepTally(tally) => sleep_tally = Some(tally),
                TickSection::ActionOutcomes(list) => outcomes = list,
                TickSection::SelfState(state) => self_state = Some(state),
                TickSection::MenuSync(sync) => menu_sync = Some(sync),
                TickSection::Env(env) => {
                    for (key, value) in env {
                        self.replica.world.set_shader_param(key, value);
                    }
                }
                TickSection::OpenChests(chests) => open_chests = chests,
                TickSection::Events(events) => world_events = events,
                TickSection::SelfEvents(events) => {
                    self.replica.events.self_events.merge_from(events);
                }
            }
        }
        let tally = sleep_tally.unwrap_or_else(|| self.replica.entities.sleep_tally());
        let committed = self.replica.entities.receive(tally, StagedRows::new(rows));
        self.after_commit(committed);
        self.fx.set_open_chests(open_chests.into_iter().collect());
        self.apply_authority(&outcomes, self_state, menu_sync, &delta_cells, world_events);
    }

    /// The prediction-coupled half of a batch: adopt the authoritative self
    /// state and menu view, reconcile the prediction ledger against the
    /// batch's outcomes (rolling back what the server denied), then buffer
    /// the world events with this client's own presented cells suppressed.
    fn apply_authority(
        &mut self,
        outcomes: &[petramond::net::protocol::ActionOutcome],
        self_state: Option<SelfState>,
        menu_sync: Option<MenuSyncMsg>,
        delta_cells: &rustc_hash::FxHashSet<IVec3>,
        world_events: Vec<WorldEventMsg>,
    ) {
        // A batch's inventory / menu snapshot reflects the server state as of
        // the requests it ANSWERS. A snapshot-bearing prediction of the same
        // store that stays pending past this batch postdates that snapshot —
        // adopting it would visibly regress the newer prediction for one RTT
        // (the rapid-click flicker). Skip adoption then: every predicted menu
        // request forces the authoritative pair into its own outcome batch,
        // so the store reconciles the moment the pending queue drains.
        let stale_inventory = self.prediction.awaits_inventory_authority(outcomes);
        let stale_menu = self.prediction.awaits_menu_authority(outcomes);
        let adopted_inventory =
            !stale_inventory && self_state.as_ref().is_some_and(|s| s.inventory.is_some());
        let adopted_menu = !stale_menu && menu_sync.is_some();
        if let Some(state) = &self_state {
            self.replica.self_view.apply(state, !stale_inventory);
            if self.local.player.mode() != self.replica.self_view.mode {
                self.local.player.set_mode(self.replica.self_view.mode);
            }
            // The predicted body runs the same movement code as the server,
            // and that code reads the effect list (a speed effect scales land
            // speed). An unsynced predicted player would walk at the wrong
            // speed for the whole duration and rubber-band every batch.
            self.local.player.set_effects(
                self.replica
                    .self_view
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
            self.local.player.adopt_resolved_body(
                self.replica.self_view.move_scale,
                self.replica.self_view.denied_actions,
            );
            // Tick-side transform mutations (teleports, knockback) win over
            // the local prediction — per-field against what we last sent.
            if let Some(t) = &state.transform {
                self.adopt_authoritative_transform(t);
            }
        }
        // Snapshot predicted cells BEFORE reconcile so accept/deny this batch
        // still suppress matching wire presentation events (the ledger entry
        // is about to drop).
        let suppress: rustc_hash::FxHashSet<IVec3> = self.prediction.suppressed_cells().collect();
        // Authoritative inventory / block deltas win; then apply deny rollbacks
        // for any predicted mutations the server rejected. Snapshots come back
        // oldest-first, each capturing the state BEFORE its own prediction —
        // so a newer snapshot still embeds an older denied mutation. Applied
        // newest-first so the OLDEST snapshot wins.
        let rollbacks = self.prediction.reconcile(outcomes);
        for snap in rollbacks.into_iter().rev() {
            match snap {
                crate::game::prediction::PredictionSnapshot::None => {}
                crate::game::prediction::PredictionSnapshot::Inventory(inv) => {
                    // Only restore if we did not adopt a fresh authoritative
                    // body this batch (adopted SelfState inventory wins).
                    if !adopted_inventory {
                        self.replica.self_view.inventory = inv;
                    }
                }
                crate::game::prediction::PredictionSnapshot::Menu { inventory, menu } => {
                    if !adopted_inventory {
                        self.replica.self_view.inventory = inventory;
                    }
                    if !adopted_menu {
                        self.replica.menu_view = menu;
                    }
                }
                crate::game::prediction::PredictionSnapshot::World { inventory, cells } => {
                    if let Some(inv) = inventory {
                        if !adopted_inventory {
                            self.replica.self_view.inventory = inv;
                        }
                    }
                    // Silent restore: no world events. A same-batch
                    // authoritative delta at a cell wins over the rollback.
                    let mut restored = Vec::with_capacity(cells.len());
                    for (pos, prev_block_id) in cells {
                        if delta_cells.contains(&pos) {
                            continue;
                        }
                        let before = self.replica.world.data().chunk_block(pos.x, pos.y, pos.z);
                        let _ = self.replica.world.set_block_world(
                            pos.x,
                            pos.y,
                            pos.z,
                            petramond_world::block::Block::from_id(prev_block_id),
                        );
                        restored.push((pos, before));
                    }
                    // A rollback is a local edit too: its restored geometry
                    // and light publish under the same prediction fence.
                    self.replica.world.reconcile_predicted_edit(&restored);
                }
            }
        }
        if let Some(sync) = menu_sync {
            if stale_menu {
                self.replica.menu_view.adopt_gui_state(sync);
            } else {
                self.replica.menu_view.apply(sync);
            }
        }
        for msg in world_events {
            self.buffer_world_event(msg, &suppress);
        }
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
        let ev = &mut self.replica.events;
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
            WorldEventMsg::PanelToggled { anchor, open } => {
                ev.world.push(WorldEvent::PanelToggled { anchor, open })
            }
            WorldEventMsg::ChestOpened { pos } => ev.world.push(WorldEvent::ChestOpened { pos }),
            WorldEventMsg::ChestClosed { pos } => ev.world.push(WorldEvent::ChestClosed { pos }),
            WorldEventMsg::ItemPickedUp { pos, by } => ev.world.push(WorldEvent::ItemPickedUp {
                pos,
                by_self: by == self.replica.entities.self_id(),
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
                direction,
                texture,
            } => {
                use crate::particle::BurstLook;
                use petramond::net::protocol::BurstTextureMsg;
                let look = match texture {
                    None => BurstLook::Row,
                    Some(BurstTextureMsg::Block { block_id, tint }) => BurstLook::Block {
                        block: petramond_world::block::Block::from_id(block_id),
                        kv_tint: tint,
                    },
                    // A tile this client's packs do not know: the row's own
                    // look, not nothing.
                    Some(BurstTextureMsg::Tile { tile, slice, tint }) => {
                        petramond_world::particle_emitters::TextureSlice::named(&tile, slice)
                            .map_or(BurstLook::Row, |slice| BurstLook::Texture {
                                slice,
                                tint: tint.map(|c| f32::from(c) / 255.0),
                            })
                    }
                };
                ev.world.push(WorldEvent::EmitterBurst {
                    emitter: emitter_id,
                    pos,
                    intensity,
                    direction,
                    look,
                })
            }
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
