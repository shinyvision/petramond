//! [`Mobs`]: the live-mob container owned by `Game`.
//!
//! Holds every active mob and drives them on the **game tick**. Spawning and
//! despawning go through here, so adding a species to the world is `mobs.spawn(kind,
//! …)` — never a new field. The render-side scene adapter reads [`Mobs::instances`].
//!
//! At construction it scans each species' model once for the metadata the AI needs
//! (currently the `idle_*` animation count), so the per-tick idle-animation behavior
//! only ever picks animations the model actually has.

use rustc_hash::{FxHashMap, FxHashSet};

use petramond_math::math::IVec3;
#[cfg(test)]
use petramond_math::math::Vec3;
use petramond_world::block::Block;
use petramond_world::chunk::ChunkPos;

use super::noise::{Noise, NoiseField};
use super::spatial::MobSnapshot;
use super::{
    append_body_supports, body_has_peer_support, def, instance, solid_boxes,
    terrain_safe_motion_prefix, BodyMotion, EntityRef, Instance, MobCollision, MobId, MobRng,
};

#[cfg(test)]
use super::body_separation;

mod control;
mod drops;
mod lifecycle;
mod lod;
mod simulation;
#[cfg(test)]
mod tests;

pub use drops::{DeathDrop, MobSpill, ShearDrop};
pub use lod::SimDistance;
mod push;
mod solids;
use push::PushScratch;
pub use simulation::{MobAttack, MobExposureDamage, MobFall, MobTickEvents, PlayerAnchor};
use solids::SolidScratch;

/// The anchor nearest `pos`. Anchors are never empty: the local session always
/// exists.
fn nearest_anchor(
    anchors: &[PlayerAnchor],
    pos: petramond_math::world_pos::WorldPos,
) -> &PlayerAnchor {
    debug_assert!(!anchors.is_empty(), "at least the local session anchors");
    let mut best = &anchors[0];
    let mut best_d = best.pos.distance_squared(pos);
    for a in &anchors[1..] {
        let d = a.pos.distance_squared(pos);
        if d < best_d {
            best = a;
            best_d = d;
        }
    }
    best
}

/// Decorrelates the spawner's RNG stream from the per-mob AI streams (which seed
/// from the spawn counter), so the two don't march in lockstep on a given world.
const SPAWN_RNG_SALT: u64 = 0x5EED_5EED_5EED_5EED;

/// The live mob set. Its public API is addressed by [`MobId`] — every method
/// resolves the handle to a storage slot itself and answers `None`/`false`
/// for a mob that is gone, so a caller can never act on a slot a removal has
/// since renumbered. Slots are private to the manager and its tick.
pub struct Mobs {
    /// The live set. Mutated ONLY through [`push_instance`](Self::push_instance),
    /// [`swap_remove_instance`](Self::swap_remove_instance) and
    /// [`retain_instances`](Self::retain_instances), which keep `slot_by_id`
    /// in step.
    list: Vec<Instance>,
    /// Stable id → current slot in `list` — the O(1) resolver behind every
    /// id-addressed method ([`slot`](Self::slot)).
    slot_by_id: FxHashMap<MobId, usize>,
    /// Monotonic counter seeding each mob's deterministic AI.
    spawn_counter: u64,
    /// Deterministic RNG driving the per-tick natural-spawn picker.
    rng: MobRng,
    /// How much of each mob's tick runs by its distance from the players
    /// (see [`lod`]). [`SimDistance::UNLIMITED`] until the server installs
    /// its setting through [`set_sim_distance`](Self::set_sim_distance).
    sim_distance: SimDistance,
    /// Per-mob schedule for this tick (index-aligned with `list`).
    step_scratch: Vec<lod::SimStep>,
    turns: Vec<simulation::Turn>,
    push: PushScratch,
    solid: SolidScratch,
    scripted_requests: Vec<crate::modding::ai::AiNodeRequest>,
    /// The tick's shared route-search budget, refilled at the start of
    /// every [`tick`](Self::tick) (see `nav::PATH_TICK_BUDGET`).
    path_budget: super::nav::PathBudget,
    /// Reused per-tick AI snapshot (one entry per live mob), spatially
    /// indexed for the tick's neighbour queries.
    ai_snapshot: MobSnapshot,
    /// Reused buffers for every mob's exposure tick.
    exposure_scratch: crate::exposure::ExposureScratch,
    /// Gameplay noises accumulated since the last mob tick (player/block noises
    /// pushed by the game's earlier stages this tick, plus mob footsteps from
    /// the previous mob tick). Swapped into [`heard`](Self::heard) at the start
    /// of [`tick`](Self::tick).
    pending_noises: Vec<Noise>,
    /// The batch every mob's AI hears THIS tick — snapshotted before any mob
    /// moves, so hearing is independent of iteration order, and bucketed by
    /// column so each listener visits only the noises around it.
    heard: NoiseField,
    /// Chunks whose one-time population roll already completed THIS SESSION
    /// (see [`populate`]) — a memo so the per-tick scan doesn't re-roll them.
    /// The cross-session "this chunk spawned its herd" fact lives on the
    /// world's persisted populated set, not here.
    populate_checked: FxHashSet<ChunkPos>,
    /// Live confined regions shared by penned mobs (see [`super::confined`]):
    /// one flood-fill serves every mob in the same pen. Block changes drop
    /// overlapping regions through [`invalidate_confined_regions`]
    /// (fed from the world's change log) before each mob tick.
    ///
    /// [`invalidate_confined_regions`]: Mobs::invalidate_confined_regions
    confined_regions: super::confined::RegionCache,
    /// This manager's place in the world's change log: the number of the
    /// next announced change `confined_regions` has not been told about.
    change_seq: u64,
    /// Carried stacks of mobs that left the world this tick by death or
    /// despawn, waiting for the game to scatter them (the manager cannot
    /// spawn item entities itself). Drained by [`take_spills`](Self::take_spills).
    spills: Vec<drops::MobSpill>,
}

impl Default for Mobs {
    fn default() -> Self {
        Self::new(0)
    }
}

impl Mobs {
    /// Advance each mob's victim-owned damage immunity once per game tick.
    pub fn tick_damage_immunity(&mut self) {
        for mob in &mut self.list {
            mob.tick_damage_immunity();
        }
    }
    /// `seed` (the world seed) makes natural spawning reproducible per world.
    pub fn new(seed: u64) -> Self {
        Mobs {
            list: Vec::new(),
            slot_by_id: FxHashMap::default(),
            spawn_counter: 0,
            rng: MobRng::new(seed ^ SPAWN_RNG_SALT),
            sim_distance: SimDistance::UNLIMITED,
            step_scratch: Vec::new(),
            turns: Vec::new(),
            push: PushScratch::default(),
            solid: SolidScratch::default(),
            scripted_requests: Vec::new(),
            path_budget: super::nav::PathBudget::default(),
            ai_snapshot: MobSnapshot::default(),
            exposure_scratch: Default::default(),
            pending_noises: Vec::new(),
            heard: NoiseField::default(),
            populate_checked: FxHashSet::default(),
            confined_regions: super::confined::RegionCache::default(),
            change_seq: 0,
            spills: Vec::new(),
        }
    }

    /// The carried stacks of every mob that died or despawned since the last
    /// drain, for the game to scatter.
    pub fn take_spills(&mut self) -> Vec<drops::MobSpill> {
        std::mem::take(&mut self.spills)
    }

    fn spill_container(&mut self, slot: usize) {
        let Some(mob) = self.list.get_mut(slot) else {
            return;
        };
        let stacks = mob.take_container_items();
        if !stacks.is_empty() {
            self.spills.push(drops::MobSpill {
                pos: mob.pos,
                stacks,
                skylight: mob.skylight,
                blocklight: mob.blocklight,
            });
        }
    }

    /// This manager's place in the world's change log (see
    /// [`invalidate_confined_regions`](Self::invalidate_confined_regions)).
    pub fn change_seq(&self) -> u64 {
        self.change_seq
    }

    /// Drop cached confined regions a block change could have altered: the
    /// nav-relevant changes logged from [`change_seq`](Self::change_seq) on,
    /// with `next` the place to read from next time. `all` means some were
    /// lost (exact positions unknown).
    pub fn invalidate_confined_regions(
        &mut self,
        next: u64,
        changed: &[petramond_math::math::IVec3],
        all: bool,
    ) {
        self.change_seq = next;
        self.confined_regions.invalidate(changed, all);
    }

    /// Record one gameplay noise for the NEXT mob AI batch (this tick's, when
    /// pushed before the mob stage). Emitters go through
    /// [`World::push_noise`](crate::world::ServerWorld::push_noise).
    pub fn push_noise(&mut self, noise: Noise) {
        self.pending_noises.push(noise);
    }

    /// Drop the accumulated noise batch unheard — the mob tick's early-out for
    /// an empty live set calls this so emitters can't grow the buffer forever
    /// while nothing exists to listen.
    pub fn discard_noises(&mut self) {
        self.pending_noises.clear();
        self.heard.clear();
    }

    /// Install the simulation-distance policy (sanitized: see
    /// [`SimDistance::sanitized`]).
    pub fn set_sim_distance(&mut self, policy: SimDistance) {
        self.sim_distance = policy.sanitized();
    }

    /// The simulation-distance policy in force.
    pub fn sim_distance(&self) -> SimDistance {
        self.sim_distance
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.list.len()
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.list.is_empty()
    }

    /// The storage slot of the mob `id`, or `None` when it is gone. Slots
    /// are valid only until the next removal and never leave the manager.
    #[inline]
    fn slot(&self, id: MobId) -> Option<usize> {
        self.slot_by_id.get(&id).copied()
    }

    /// The live mob `id` — the shared guard behind every id-addressed
    /// setter, so `list` stays private and `Game` never holds a
    /// `&mut Instance`.
    fn mob_mut(&mut self, id: MobId) -> Option<&mut Instance> {
        let slot = self.slot(id)?;
        self.list.get_mut(slot)
    }

    /// The mob `id` (dead corpses included), or `None` when it has left the
    /// live set.
    #[inline]
    pub fn get(&self, id: MobId) -> Option<&Instance> {
        self.list.get(self.slot(id)?)
    }

    /// Whether the mob `id` is still in the live set (a ragdolling corpse
    /// counts until it is removed).
    #[inline]
    pub fn contains(&self, id: MobId) -> bool {
        self.slot_by_id.contains_key(&id)
    }

    /// The mob `id` when it is alive — `None` for a gone mob and for a corpse
    /// alike, the one dead-mob policy of id-addressed gameplay actions.
    #[inline]
    pub fn live(&self, id: MobId) -> Option<&Instance> {
        self.get(id).filter(|m| !m.is_dead())
    }

    /// Where the mob `id` sits in [`instances`](Self::instances) right now —
    /// a read-only ORDERING key (the mod ABI's intra-tick join key), never an
    /// address: no method takes one back, and a removal reorders it.
    #[inline]
    pub fn position_of(&self, id: MobId) -> Option<usize> {
        self.slot(id)
    }

    /// One tick of the dig the mob `id` is driven through (see
    /// [`Instance::advance_dig`](super::Instance::advance_dig)).
    pub fn advance_dig(
        &mut self,
        id: MobId,
        now: u64,
        pos: IVec3,
        block: petramond_world::block::Block,
        tool: Option<petramond_world::item::Tool>,
    ) -> Option<super::DigStep> {
        Some(self.mob_mut(id)?.advance_dig(now, pos, block, tool))
    }

    /// Turn the mob `id` to look along a world yaw and pitch at once.
    #[cfg(test)]
    pub fn set_gaze_for_test(&mut self, id: MobId, yaw: f32, pitch: f32) {
        if let Some(mob) = self.mob_mut(id) {
            mob.yaw = yaw;
            mob.head_yaw = 0.0;
            mob.head_pitch = pitch;
        }
    }

    /// Replace the draw set the mob `id` wears.
    pub fn set_draw(&mut self, id: MobId, draw: crate::world::draw::BodyDraw) {
        if let Some(mob) = self.mob_mut(id) {
            mob.set_draw(draw);
        }
    }

    /// Claim what the mob `id` draws in its hands.
    pub fn set_held(&mut self, id: MobId, held: [Option<petramond_world::item::ItemType>; 2]) {
        if let Some(mob) = self.mob_mut(id) {
            mob.set_held(held);
        }
    }

    /// The carried slots of the mob `id`.
    pub fn container_mut(
        &mut self,
        id: MobId,
    ) -> Option<&mut petramond_world::container::Container> {
        self.mob_mut(id).map(Instance::container_mut)
    }

    /// The live mobs, for the render-side scene adapter to bake and for
    /// read-only scans (read-only; the order is storage order, which a
    /// removal changes — address a mob by its [`id`](Instance::id)).
    #[inline]
    pub fn instances(&self) -> &[Instance] {
        &self.list
    }

    /// Append `mob` to the live set.
    fn push_instance(&mut self, mob: Instance) {
        self.slot_by_id.insert(mob.id(), self.list.len());
        self.list.push(mob);
    }

    /// Remove the mob in `slot` by `swap_remove`, renumbering the last mob
    /// into the hole.
    fn swap_remove_instance(&mut self, slot: usize) -> Instance {
        let mob = self.list.swap_remove(slot);
        self.slot_by_id.remove(&mob.id());
        if let Some(moved) = self.list.get(slot) {
            self.slot_by_id.insert(moved.id(), slot);
        }
        mob
    }

    /// Keep only the mobs `keep` accepts, preserving their order.
    fn retain_instances(&mut self, keep: impl FnMut(&Instance) -> bool) {
        let before = self.list.len();
        self.list.retain(keep);
        if self.list.len() != before {
            self.slot_by_id.clear();
            self.slot_by_id
                .extend(self.list.iter().enumerate().map(|(i, m)| (m.id(), i)));
        }
    }

    /// Whether placing `block` at cell `p` would clip into any live mob — its collision
    /// box(es) at `p` overlapping a mob's body. A no-collision block (a torch, grass, a
    /// fern, …) has no boxes, so this is always `false` and it may be placed freely even
    /// on a mob; only a block that physically collides is blocked. A ragdolling corpse
    /// (about to vanish) doesn't count. The placement code calls this to refuse dropping
    /// a solid block on top of a mob.
    pub fn any_overlapping_placement(&self, p: IVec3, block: Block) -> bool {
        self.any_overlapping_boxes(p, block.collision_boxes())
    }

    /// Whether the supplied cell-local collision boxes at `p` overlap a mob's body.
    /// Used by oriented bbmodel placement, where each occupied cell has its own rotated
    /// per-cell shape.
    pub fn any_overlapping_boxes(&self, p: IVec3, boxes: &[petramond_world::block::Aabb]) -> bool {
        self.list
            .iter()
            .filter(|m| !m.is_dead())
            .any(|m| super::body_overlaps_block_boxes(m.pos, m.yaw, def(m.kind).size, p, boxes))
    }
}
