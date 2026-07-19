//! [`Mobs`]: the live-mob container owned by `Game`.
//!
//! Holds every active mob and drives them on the **game tick**. Spawning and
//! despawning go through here, so adding a species to the world is `mobs.spawn(kind,
//! …)` — never a new field. The render-side scene adapter reads [`Mobs::instances`].
//!
//! At construction it scans each species' model once for the metadata the AI needs
//! (currently the `idle_*` animation count), so the per-tick idle-animation behavior
//! only ever picks animations the model actually has.

use rustc_hash::FxHashSet;

use crate::block::Block;
use crate::chunk::ChunkPos;
use crate::mathh::{IVec3, Vec3};

use super::brain::AiMob;
use super::noise::Noise;
use super::{
    append_body_supports, body_has_peer_support, body_separation, body_separation_from_body, def,
    instance, solid_boxes, terrain_safe_motion_prefix, BodyMotion, EntityRef, Instance,
    MobCollision, MobRng, MobSize,
};

mod drops;
mod lifecycle;
mod mod_control;
mod simulation;
#[cfg(test)]
mod tests;

pub use drops::{DeathDrop, ShearDrop};
use simulation::PushBody;
pub use simulation::{MobAttack, MobFall, MobTickEvents, PlayerAnchor};

/// The anchor nearest `pos`. Anchors are never empty: the local session always
/// exists.
fn nearest_anchor(anchors: &[PlayerAnchor], pos: Vec3) -> &PlayerAnchor {
    debug_assert!(!anchors.is_empty(), "at least the local session anchors");
    let mut best = &anchors[0];
    let mut best_d = (best.pos - pos).length_squared();
    for a in &anchors[1..] {
        let d = (a.pos - pos).length_squared();
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

pub struct Mobs {
    list: Vec<Instance>,
    /// Monotonic counter seeding each mob's deterministic AI.
    spawn_counter: u64,
    /// Deterministic RNG driving the per-tick natural-spawn picker.
    rng: MobRng,
    /// Reused per-tick AI snapshot buffer (one entry per live mob).
    ai_scratch: Vec<AiMob>,
    /// Reused per-tick body snapshot buffer (index-aligned with `list`).
    push_scratch: Vec<Option<PushBody>>,
    /// Index-aligned soft-push sums, reused across ticks.
    push_velocity_scratch: Vec<Vec3>,
    /// Push participants in stable-id order.
    push_order_scratch: Vec<usize>,
    /// Whether an instance actually ran this tick (frozen instances do not).
    ticked_scratch: Vec<bool>,
    /// Pre-integration ground state and post-healing peer-motion start for
    /// instances whose live body moved.
    motion_finish_scratch: Vec<Option<(bool, Vec3)>>,
    /// Terrain-resolved solid-body proposals, stable-id sorted before the
    /// pair solver runs.
    solid_motion_scratch: Vec<super::BodyMotion>,
    solid_index_scratch: Vec<usize>,
    solid_limit_scratch: Vec<f32>,
    solid_checked_scratch: Vec<f32>,
    solid_support_scratch: Vec<crate::collision::DynBox>,
    solid_motion_solver: super::SolidMotionSolver,
    /// Reused per-tick stable-id snapshot (index-aligned with `list`), so the
    /// push pass can name contacts while mutating instances.
    id_scratch: Vec<u64>,
    /// Index-aligned touch contacts, reused across ticks.
    contact_scratch: Vec<Vec<super::EntityRef>>,
    /// Gameplay noises accumulated since the last mob tick (player/block noises
    /// pushed by the game's earlier stages this tick, plus mob footsteps from
    /// the previous mob tick). Swapped into [`heard`](Self::heard) at the start
    /// of [`tick`](Self::tick).
    pending_noises: Vec<Noise>,
    /// The batch every mob's AI hears THIS tick — snapshotted before any mob
    /// moves, so hearing is independent of iteration order.
    heard: Vec<Noise>,
    /// Chunks whose one-time population roll already completed THIS SESSION
    /// (see [`populate`]) — a memo so the per-tick scan doesn't re-roll them.
    /// The cross-session "this chunk spawned its herd" fact lives on the
    /// world's persisted populated set, not here.
    populate_checked: FxHashSet<ChunkPos>,
}

impl Default for Mobs {
    fn default() -> Self {
        Self::new(0)
    }
}

impl Mobs {
    /// `seed` (the world seed) makes natural spawning reproducible per world.
    pub fn new(seed: u64) -> Self {
        Mobs {
            list: Vec::new(),
            spawn_counter: 0,
            rng: MobRng::new(seed ^ SPAWN_RNG_SALT),
            ai_scratch: Vec::new(),
            push_scratch: Vec::new(),
            push_velocity_scratch: Vec::new(),
            push_order_scratch: Vec::new(),
            ticked_scratch: Vec::new(),
            motion_finish_scratch: Vec::new(),
            solid_motion_scratch: Vec::new(),
            solid_index_scratch: Vec::new(),
            solid_limit_scratch: Vec::new(),
            solid_checked_scratch: Vec::new(),
            solid_support_scratch: Vec::new(),
            solid_motion_solver: super::SolidMotionSolver::default(),
            id_scratch: Vec::new(),
            contact_scratch: Vec::new(),
            pending_noises: Vec::new(),
            heard: Vec::new(),
            populate_checked: FxHashSet::default(),
        }
    }

    /// Record one gameplay noise for the NEXT mob AI batch (this tick's, when
    /// pushed before the mob stage). Emitters go through
    /// [`World::push_noise`](crate::world::World::push_noise).
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

    #[inline]
    pub fn len(&self) -> usize {
        self.list.len()
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.list.is_empty()
    }

    /// The live mob at `index` — the shared guard behind every by-index
    /// setter, so `list` stays private and `Game` never holds a
    /// `&mut Instance`.
    fn mob_mut(&mut self, index: usize) -> Option<&mut Instance> {
        self.list.get_mut(index)
    }

    /// The live mobs, for the render-side scene adapter to bake (read-only).
    #[inline]
    pub fn instances(&self) -> &[Instance] {
        &self.list
    }

    /// Resolve a STABLE mob id to its current list index, or `None` when the
    /// mob is gone. Actions arriving over the wire carry ids (indices shift
    /// under despawns between the click and the consuming tick).
    pub fn index_of_id(&self, id: u64) -> Option<usize> {
        self.list.iter().position(|m| m.id() == id)
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
    pub fn any_overlapping_boxes(&self, p: IVec3, boxes: &[crate::block::Aabb]) -> bool {
        self.list
            .iter()
            .filter(|m| !m.is_dead())
            .any(|m| super::body_overlaps_block_boxes(m.pos, m.yaw, def(m.kind).size, p, boxes))
    }
}
