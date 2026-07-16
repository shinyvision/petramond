//! [`Mobs`]: the live-mob container owned by `Game`.
//!
//! Holds every active mob and drives them on the **game tick**. Spawning and
//! despawning go through here, so adding a species to the world is `mobs.spawn(kind,
//! …)` — never a new field. The render-side scene adapter reads [`Mobs::instances`].
//!
//! At construction it scans each species' model once for the metadata the AI needs
//! (currently the `idle_*` animation count), so the per-tick idle-animation behavior
//! only ever picks animations the model actually has.

use std::collections::HashMap;
use std::sync::LazyLock;

use rustc_hash::FxHashSet;

use crate::block::Block;
use crate::body::Body;
use crate::chunk::{ChunkPos, SectionPos};
use crate::mathh::{voxel_at, IVec3, Vec3};
use crate::world::World;

use super::brain::{AiMob, TickInputs};
use super::model_meta::{self, IdleAnimMeta, Skeleton};
use super::noise::{Noise, NoiseKind};
use super::{
    def, defs, model, populate, spawn, EntityRef, Instance, Mob, MobDamageFeedback, MobRng,
    SavedMob,
};

/// What a mob leaves behind the instant it dies, so `Game` can roll its loot table and
/// spawn the drops (the manager has only `&World` and can't spawn item entities itself).
#[derive(Copy, Clone, Debug)]
pub struct DeathDrop {
    pub kind: Mob,
    pub pos: Vec3,
    pub skylight: u8,
    pub blocklight: u8,
}

/// What a successful shear yields, so `Game` can spawn the drop (like [`DeathDrop`],
/// the manager can't spawn item entities itself). The count is already rolled from the
/// mob's own deterministic RNG.
#[derive(Copy, Clone, Debug)]
pub struct ShearDrop {
    pub item: crate::item::ItemType,
    pub count: u8,
    pub pos: Vec3,
    pub skylight: u8,
    pub blocklight: u8,
}

/// One player's presence as the mob simulation sees it: an AI/despawn anchor
/// plus (for non-spectators) a pushable body. The mobs target whichever anchor
/// is NEAREST per mob, so N players share one world of mobs.
#[derive(Copy, Clone, Debug)]
pub struct PlayerAnchor {
    pub id: crate::server::player::PlayerId,
    /// Body centre — the AI's target/despawn anchor (matches the old single
    /// `player_pos` argument).
    pub pos: Vec3,
    /// The pushable body; `None` for a spectator (nothing to jostle or strike).
    pub body: Option<Body>,
    /// Whether this player is sneaking — hostile detection shrinks for a
    /// sneaking target (`chase_player`'s `sneak_radius_penalty`).
    pub sneaking: bool,
}

#[derive(Copy, Clone)]
struct PushBody {
    pos: Vec3,
    yaw: f32,
    size: super::MobSize,
}

/// A melee strike a mob landed this tick. Drained from [`Mobs::tick`] by
/// `Game`, which applies it through the target's damage pipeline: a player
/// target runs `player_damage_pre` (a cancelled strike drops its knockback
/// too); a mob target runs the shared mob damage pipeline (`mob_damage_pre`,
/// feedback, loot, ragdoll).
#[derive(Copy, Clone, Debug)]
pub struct MobAttack {
    /// Index into the live mob set — valid this tick only (mirrors
    /// `MobDamagePre::mob`).
    pub mob_index: usize,
    pub mob: Mob,
    /// The attacker's STABLE id — carried into the mob damage pipeline so the
    /// struck mob's retaliation memory can name the biter across ticks.
    pub mob_id: u64,
    /// Attacker position for damage origin / presentation context.
    pub origin: Vec3,
    /// Who the strike lands on (whatever the attacker's brain locked).
    pub target: EntityRef,
    /// Damage in half-heart points (rounded when applied to a player).
    pub damage: f32,
    /// Horizontal unit direction the target is knocked toward (away from the mob;
    /// zero when the two exactly overlap — the strike still pops upward).
    pub knockback_dir: Vec3,
    /// Horizontal knockback speed (m/s) added to a player target's velocity.
    /// A mob target takes its own row's knockback feedback instead.
    pub knockback: f32,
}

/// A fall landing measured by a mob during its deterministic tick. The stable id is
/// resolved back to a live index by `ServerGame` before applying damage through the
/// mob damage pipeline.
#[derive(Copy, Clone, Debug)]
pub struct MobFall {
    pub mob_id: u64,
    pub distance: f32,
}

/// A mob fell into water this tick (its un-latched fall drop at the first wet
/// tick). `ServerGame` throws the water-splash burst above the entry point.
#[derive(Copy, Clone, Debug)]
pub struct MobSplash {
    pub pos: Vec3,
    /// Blocks fallen into the surface — the burst intensity.
    pub fall: f32,
}

#[derive(Default, Debug)]
pub struct MobTickEvents {
    pub attacks: Vec<MobAttack>,
    pub falls: Vec<MobFall>,
    pub splashes: Vec<MobSplash>,
}

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

/// Hard cap on simultaneous mobs, so a spawn loop / debug key can't run the world
/// out of memory. Spawns past this are dropped.
const MAX_MOBS: usize = 256;

/// Decorrelates the spawner's RNG stream from the per-mob AI streams (which seed
/// from the spawn counter), so the two don't march in lockstep on a given world.
const SPAWN_RNG_SALT: u64 = 0x5EED_5EED_5EED_5EED;

/// Per-species, model-derived metadata the sim reads.
struct MobMeta {
    /// This species' `idle_*` animations (name-sorted; length + loop mode).
    idle_anims: Vec<IdleAnimMeta>,
    /// Bone hierarchy (pivots + parents) for the death ragdoll, matching the renderer's
    /// bone order so a sim-computed pose drops into the render bake.
    skeleton: Skeleton,
}

/// Every species' [`MobMeta`], derived once for the whole process from the precached
/// [`Model`](crate::bbmodel::Model)s (see [`model`](super::model)) and indexed by `Mob as
/// usize`. It's identical for every world, so computing it once keeps each `World::new` (of
/// which the tests make dozens) from re-deriving it — and nothing here re-reads a `.bbmodel`.
static MOB_META: LazyLock<Vec<MobMeta>> = LazyLock::new(|| {
    defs()
        .iter()
        .map(|d| MobMeta {
            idle_anims: model_meta::idle_anims(model(d.mob)),
            skeleton: model_meta::skeleton(model(d.mob)),
        })
        .collect()
});

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

    /// Spawn a mob of `kind` at `pos` (feet) facing `yaw`. Returns `false` if the
    /// mob cap is reached (the spawn is dropped).
    pub fn spawn(&mut self, kind: Mob, pos: Vec3, yaw: f32) -> bool {
        self.spawn_lit(kind, pos, yaw, 63, 0)
    }

    /// Spawn a mob with its render light initialized for the first presentation
    /// frame. Use this from world-owned spawn paths where the spawn cell's light
    /// is already available; otherwise a cave spawn can render full-bright until
    /// the next mob tick refreshes cached light.
    pub fn spawn_lit(
        &mut self,
        kind: Mob,
        pos: Vec3,
        yaw: f32,
        skylight: u8,
        blocklight: u8,
    ) -> bool {
        if self.list.len() >= MAX_MOBS {
            return false;
        }
        self.spawn_counter = self.spawn_counter.wrapping_add(1);
        let mut mob = Instance::new(kind, pos, yaw, self.spawn_counter);
        mob.skylight = skylight;
        mob.blocklight = blocklight;
        self.list.push(mob);
        true
    }

    /// Remaining room for `kind` under its species and category spawn caps.
    pub fn spawn_room_for(&self, kind: Mob) -> u32 {
        spawn::room_for(&self.list, kind)
    }

    /// Advance every mob by one game tick (passing each its species' idle-animation
    /// metadata + ragdoll skeleton) and refresh its cached skylight, then resolve soft
    /// entity pushing and remove any mob that should leave the live world: a finished
    /// death corpse, or a hostile mob that has distance-despawned (culled, and so not
    /// saved). Returns gameplay events the mobs produced this tick: melee strikes for
    /// the player damage pipeline, and landed falls for the mob damage pipeline.
    ///
    /// `player_pos` is the player's body centre — the AI's player anchor for head-look
    /// and distance-despawn. `player_body` is the player's *pushable* body, present only
    /// when the player has a physical presence (a survival body, not a noclip spectator):
    /// when present the mobs are shoved off it (player→mob), on the tick. The reverse —
    /// the mobs shoving the *player* — is NOT done here: that moves the player, which is
    /// integrated per-frame for smoothness, so the caller applies it per-frame via
    /// [`push_on_player`](Self::push_on_player).
    ///
    /// When `freeze_unloaded` is set (a save is attached), a mob standing over a
    /// not-yet-loaded chunk is frozen — not simulated, and excluded from pushing — until
    /// the unload harvests it into that chunk's record. This mirrors the dropped-item
    /// freeze and stops a mob from falling through missing terrain at the streamed edge.
    pub fn tick(
        &mut self,
        dt: f32,
        world: &World,
        anchors: &[PlayerAnchor],
        freeze_unloaded: bool,
    ) -> MobTickEvents {
        let mut ai_mobs = std::mem::take(&mut self.ai_scratch);
        ai_mobs.clear();
        ai_mobs.extend(self.list.iter().map(|m| AiMob {
            id: m.id(),
            kind: m.kind,
            pos: m.pos,
            active: !m.is_dead() && (!freeze_unloaded || chunk_loaded_at(world, m)),
        }));
        // Solid-collision bodies as of the start of this tick. Soft mobs use
        // this immutable obstacle view; solid peers propose independently and
        // meet in the relative-motion solver below.
        let solid = self.solid_obstacles();
        // The noise batch every mob hears this tick: everything pushed since the
        // last mob tick, snapshotted BEFORE any mob moves so hearing doesn't
        // depend on iteration order. Mob footsteps recorded below land in
        // `pending_noises` for the next tick.
        std::mem::swap(&mut self.pending_noises, &mut self.heard);
        self.pending_noises.clear();
        let mut pending_noises = std::mem::take(&mut self.pending_noises);
        let mut ticked = std::mem::take(&mut self.ticked_scratch);
        ticked.clear();
        ticked.resize(self.list.len(), false);
        let mut motion_finish = std::mem::take(&mut self.motion_finish_scratch);
        motion_finish.clear();
        motion_finish.resize(self.list.len(), None);
        let mut supporting_solid = std::mem::take(&mut self.solid_support_scratch);
        supporting_solid.clear();

        // Phase 1: every instance proposes its terrain-resolved transform from
        // the same start-of-tick state. Soft bodies see rigid peers as fixed
        // obstacles. A rigid body sees only an exact support during ordinary
        // Y-first integration; all other peer motion is resolved together.
        for (i, mob) in self.list.iter_mut().enumerate() {
            if freeze_unloaded && !chunk_loaded_at(world, mob) {
                // Drive is a this-tick intent. A frozen mob never reaches the
                // integration site that normally consumes it, so discard it
                // here rather than letting it wake up on a stale command.
                mob.clear_drive();
                continue;
            }
            ticked[i] = true;
            let meta = &MOB_META[mob.kind.0 as usize];
            let d = def(mob.kind);
            // Every player-facing decision (chase target, head look, despawn
            // distance, strike geometry) anchors on the NEAREST player.
            let anchor = *nearest_anchor(anchors, mob.pos);
            let peer_obstacles = if d.collision == super::MobCollision::Solid {
                supporting_solid.clear();
                super::append_body_supports(
                    mob.pos,
                    mob.yaw,
                    d.size,
                    &solid,
                    mob.id(),
                    &mut supporting_solid,
                );
                supporting_solid.as_slice()
            } else {
                solid.as_slice()
            };
            let inputs = TickInputs {
                world,
                players: anchors,
                noises: &self.heard,
                mobs: &ai_mobs,
                solid: peer_obstacles,
                solid_heal: &solid,
            };
            motion_finish[i] = mob.tick(
                dt,
                &inputs,
                &anchor,
                i,
                d.despawn_radius,
                &meta.idle_anims,
                &meta.skeleton,
            );
        }

        // Phase 2: solve solid peers from their complete proposals, then
        // commit every selected prefix together. Stable-id sorting keeps the
        // broadphase and tie paths independent of storage order.
        let mut solid_motions = std::mem::take(&mut self.solid_motion_scratch);
        solid_motions.clear();
        let mut solid_indices = std::mem::take(&mut self.solid_index_scratch);
        solid_indices.clear();
        solid_indices.extend(self.list.iter().enumerate().filter_map(|(i, mob)| {
            (!mob.is_dead() && def(mob.kind).collision == super::MobCollision::Solid).then_some(i)
        }));
        solid_indices.sort_by_key(|&i| self.list[i].id());
        solid_motions.extend(solid_indices.iter().map(|&i| {
            let mob = &self.list[i];
            let motion_start = motion_finish[i].map(|(_, start)| start);
            super::BodyMotion {
                id: mob.id(),
                start_pos: motion_start.unwrap_or(mob.pos),
                start_yaw: if motion_start.is_some() {
                    mob.prev_yaw
                } else {
                    mob.yaw
                },
                end_pos: mob.pos,
                end_yaw: mob.yaw,
                size: def(mob.kind).size,
            }
        }));
        let mut solid_limits = std::mem::take(&mut self.solid_limit_scratch);
        solid_limits.clear();
        solid_limits.resize(solid_motions.len(), 1.0);
        let mut terrain_checked = std::mem::take(&mut self.solid_checked_scratch);
        terrain_checked.clear();
        terrain_checked.resize(solid_motions.len(), 0.0);
        let mut solid_solver = std::mem::take(&mut self.solid_motion_solver);
        let terrain_boxes = |x: i32, y: i32, z: i32| world.collision_boxes_at(x, y, z);
        let mut settled = false;
        for _ in 0..=solid_motions.len() {
            solid_solver.resolve_with_limits(&solid_motions, &solid_limits);
            let mut limits_changed = false;
            for (i, motion) in solid_motions.iter().copied().enumerate() {
                let fraction = solid_solver.fractions()[i];
                if fraction >= 1.0 - 1e-6 || fraction <= terrain_checked[i] + 1e-6 {
                    continue;
                }
                let safe = super::terrain_safe_motion_prefix(motion, fraction, &terrain_boxes);
                if safe + 1e-6 < fraction {
                    solid_limits[i] = solid_limits[i].min(safe);
                    terrain_checked[i] = safe;
                    limits_changed = true;
                } else {
                    terrain_checked[i] = fraction;
                }
            }
            if !limits_changed {
                settled = true;
                break;
            }
        }
        debug_assert!(settled, "each terrain limit can tighten at most once");
        let fractions = solid_solver.fractions();
        for ((motion, &fraction), &i) in solid_motions.iter().zip(fractions).zip(&solid_indices) {
            self.list[i].commit_solid_motion(*motion, fraction);
        }

        // Final top-face support is queried only after every solid has committed,
        // so landing does not depend on storage order. Supplying the same exact
        // supports to the next proposal lets gravity land first and horizontal
        // drive/AI motion continue, just like the terrain resolver's Y-then-XZ
        // ordering.
        let mut committed_solid = solid;
        committed_solid.clear();
        for mob in &self.list {
            let d = def(mob.kind);
            if !mob.is_dead() && d.collision == super::MobCollision::Solid {
                super::solid_boxes(mob.id(), mob.pos, mob.yaw, d.size, &mut committed_solid);
            }
        }
        for ((motion, &i), moved) in solid_motions
            .iter()
            .zip(&solid_indices)
            .zip(solid_indices.iter().map(|&i| motion_finish[i].is_some()))
        {
            if moved
                && motion.moves_down()
                && super::body_has_peer_support(
                    self.list[i].pos,
                    self.list[i].yaw,
                    motion.size,
                    &committed_solid,
                    motion.id,
                )
            {
                self.list[i].land_on_solid_peer();
            }
        }
        self.solid_motion_solver = solid_solver;
        self.solid_motion_scratch = solid_motions;
        self.solid_index_scratch = solid_indices;
        self.solid_limit_scratch = solid_limits;
        self.solid_checked_scratch = terrain_checked;
        self.solid_support_scratch = supporting_solid;

        // Post-motion bookkeeping observes committed poses, never an
        // overlapping proposal that the pair solver subsequently shortened.
        let mut out = MobTickEvents::default();
        for (i, mob) in self.list.iter_mut().enumerate() {
            if !ticked[i] {
                continue;
            }
            if let Some((was_on_ground, _)) = motion_finish[i] {
                let feet = voxel_at(mob.pos);
                let in_water = world.water_cell_at(feet.x, feet.y, feet.z)
                    || world.water_cell_at(feet.x, feet.y - 1, feet.z);
                mob.finish_motion(was_on_ground, in_water);
            }
            // A walking mob is audible: record its footstep for next tick's batch.
            if mob.moving {
                pending_noises.push(Noise {
                    pos: mob.pos,
                    kind: NoiseKind::Step,
                    source: EntityRef::Mob(mob.id()),
                });
            }
            if let Some(intent) = mob.take_attack() {
                // The knockback direction is derived here, from the live
                // attacker→target geometry at strike time — horizontal, away
                // from the attacker. A target that vanished mid-tick (player
                // disconnected, mob culled) fizzles the strike whole.
                let target_pos = match intent.target {
                    EntityRef::Player(pid) => anchors.iter().find(|a| a.id == pid).map(|a| a.pos),
                    EntityRef::Mob(id) => ai_mobs
                        .iter()
                        .find(|m| m.id == id && m.active)
                        .map(|m| m.pos),
                };
                if let Some(target_pos) = target_pos {
                    let mut away = target_pos - mob.pos;
                    away.y = 0.0;
                    out.attacks.push(MobAttack {
                        mob_index: i,
                        mob: mob.kind,
                        mob_id: mob.id(),
                        origin: mob.pos,
                        target: intent.target,
                        damage: intent.damage,
                        knockback_dir: away.normalize_or_zero(),
                        knockback: intent.knockback,
                    });
                }
            }
            if let Some(distance) = mob.take_fall_distance() {
                out.falls.push(MobFall {
                    mob_id: mob.id(),
                    distance,
                });
            }
            if let Some(fall) = mob.take_splash_drop() {
                out.splashes.push(MobSplash { pos: mob.pos, fall });
            }
            let c = voxel_at(mob.pos + Vec3::new(0.0, 0.3, 0.0));
            mob.skylight = world.skylight6_at_world(c.x, c.y, c.z);
            mob.blocklight = world.blocklight6_at_world(c.x, c.y, c.z);
        }
        self.ticked_scratch = ticked;
        self.motion_finish_scratch = motion_finish;
        self.pending_noises = pending_noises;
        self.ai_scratch = ai_mobs;
        self.resolve_pushes(world, anchors, freeze_unloaded);
        self.list
            .retain(|m| !m.is_despawned() && !m.is_distance_despawned());
        out
    }

    /// Soft-push pass: for every overlapping pair of bodies — mob↔mob, and mob←player when
    /// `player` is present — set each mob's push *velocity* away from the others, to be
    /// applied (through the mob's own collision) on its next integrate. This shoves only
    /// *mobs* (which simulate on the tick); the player's own pushback is computed
    /// per-frame in [`push_on_player`](Self::push_on_player), not here.
    ///
    /// Computed from a single up-front snapshot of every compound body. Each mob pair is
    /// visited once and its deepest segment overlap yields one equal-and-opposite
    /// separation, so segment count and list order cannot multiply the shove. A mob that
    /// isn't pushable this tick (dead, or frozen over an unloaded chunk) neither pushes
    /// nor is pushed.
    /// The same overlap tests double as the TOUCH perception channel: every
    /// overlapping entity is recorded on the mob as a contact (`EntityRef`),
    /// which next tick's AI reads as `AiCtx::contacts` (the `chase_contact`
    /// node's input). A mob that doesn't participate this tick gets its
    /// contacts cleared, so nothing stales through death or a freeze.
    fn resolve_pushes(&mut self, world: &World, anchors: &[PlayerAnchor], freeze_unloaded: bool) {
        // `None` marks a mob that doesn't participate this tick; index aligns with `list`.
        let mut bodies = std::mem::take(&mut self.push_scratch);
        bodies.clear();
        bodies.extend(self.list.iter().map(|m| {
            is_pushable(m, world, freeze_unloaded).then(|| PushBody {
                pos: m.pos,
                yaw: m.yaw,
                size: def(m.kind).size,
            })
        }));
        let mut ids = std::mem::take(&mut self.id_scratch);
        ids.clear();
        ids.extend(self.list.iter().map(Instance::id));
        let mut contacts = std::mem::take(&mut self.contact_scratch);
        contacts.resize_with(self.list.len(), Vec::new);
        contacts.iter_mut().for_each(Vec::clear);
        let mut pushes = std::mem::take(&mut self.push_velocity_scratch);
        pushes.clear();
        pushes.resize(self.list.len(), Vec3::ZERO);
        let mut order = std::mem::take(&mut self.push_order_scratch);
        order.clear();
        order.extend((0..self.list.len()).filter(|&i| bodies[i].is_some()));
        order.sort_by_key(|&i| ids[i]);

        // Resolve each mob pair once. A compound body contributes its deepest
        // segment contact only, then the pair's one separation is applied
        // equally and oppositely to whichever members are soft.
        for (rank, &i) in order.iter().enumerate() {
            let a = bodies[i].unwrap();
            for &j in &order[rank + 1..] {
                let b = bodies[j].unwrap();
                let Some(push_a) =
                    super::body_separation(a.pos, a.yaw, a.size, b.pos, b.yaw, b.size)
                else {
                    continue;
                };
                if def(self.list[i].kind).collision != super::MobCollision::Solid {
                    pushes[i] += push_a;
                }
                if def(self.list[j].kind).collision != super::MobCollision::Solid {
                    pushes[j] -= push_a;
                }
                contacts[i].push(super::EntityRef::Mob(ids[j]));
                contacts[j].push(super::EntityRef::Mob(ids[i]));
            }
        }

        // Players are ordinary one-box bodies. Solid mobs still record touch
        // but receive no soft push, and the reverse player reaction remains a
        // per-frame query below.
        for (i, body) in bodies.iter().copied().enumerate() {
            let Some(body) = body else {
                continue;
            };
            let rigid = def(self.list[i].kind).collision == super::MobCollision::Solid;
            for anchor in anchors {
                let Some(player) = anchor.body else {
                    continue;
                };
                if let Some(push) =
                    super::body_separation_from_body(body.pos, body.yaw, body.size, player)
                {
                    if !rigid {
                        pushes[i] += push;
                    }
                    contacts[i].push(super::EntityRef::Player(anchor.id));
                }
            }
        }

        for i in 0..self.list.len() {
            self.list[i].set_push(pushes[i]);
            self.list[i].set_contacts(contacts[i].drain(..));
        }
        self.push_scratch = bodies;
        self.push_velocity_scratch = pushes;
        self.push_order_scratch = order;
        self.id_scratch = ids;
        self.contact_scratch = contacts;
    }

    /// The net horizontal push *velocity* the live mobs impart on the player right now,
    /// from the player's current body — read-only, mutating no mob. The caller applies it
    /// to the player **per-frame** (not on the tick) so the player drifts out of an
    /// overlap perfectly smoothly: player movement is integrated every frame, and a 20 Hz
    /// shove would pulse. A dead mob (a ragdolling corpse) doesn't push; a frozen mob over
    /// an unloaded chunk is far from the player and never overlaps, so it's moot here.
    pub fn push_on_player(&self, player: Body) -> Vec3 {
        let mut push = Vec3::ZERO;
        for m in &self.list {
            // A SOLID body never soft-pushes the player: it is a rigid
            // obstacle in the player's own resolver — a push would fight the
            // contact (skating a stander off the deck).
            if m.is_dead() || def(m.kind).collision == super::MobCollision::Solid {
                continue;
            }
            let d = def(m.kind);
            if let Some(mob_push) = super::body_separation_from_body(m.pos, m.yaw, d.size, player) {
                push -= mob_push;
            }
        }
        push
    }

    /// Apply `amount` damage to the mob at `index`. `attacker` (when the damage
    /// source names one) lands in the mob's retaliation memory.
    /// Returns the loot drop the mob leaves if the hit killed it, else `None`. Keeps
    /// `list` private — `Game` never holds a `&mut Instance`.
    pub fn damage_mob(
        &mut self,
        index: usize,
        amount: f32,
        origin: Option<Vec3>,
        attack: bool,
        attacker: Option<EntityRef>,
        feedback: &MobDamageFeedback,
    ) -> Option<DeathDrop> {
        let mob = self.mob_mut(index)?;
        if mob.damage(amount, origin, attack, attacker, feedback) {
            Some(DeathDrop {
                kind: mob.kind,
                pos: mob.pos,
                skylight: mob.skylight,
                blocklight: mob.blocklight,
            })
        } else {
            None
        }
    }

    /// Advance every mob's global damage-immunity window once. The server
    /// calls this at the start of the fixed tick, before any damage source or
    /// queued mod action can run.
    pub(crate) fn tick_damage_immunity(&mut self) {
        for mob in &mut self.list {
            mob.tick_damage_immunity();
        }
    }

    /// The dynamic collision boxes of every LIVE solid-collision body
    /// (`mobs.json` `"collision": "solid"`) — what players and mobs resolve
    /// their movement against, beside the world's cell boxes. A dead body
    /// stops blocking (a wreck is not a wall). Long bodies emit a run of
    /// boxes along their facing (see [`super::solid_boxes`]).
    pub fn solid_obstacles(&self) -> Vec<crate::collision::DynBox> {
        let mut out = Vec::new();
        for m in &self.list {
            let d = def(m.kind);
            if !m.is_dead() && d.collision == super::MobCollision::Solid {
                super::solid_boxes(m.id(), m.pos, m.yaw, d.size, &mut out);
            }
        }
        out
    }

    /// The live mob at `index` — the shared guard behind every by-index
    /// setter, so `list` stays private and `Game` never holds a
    /// `&mut Instance`.
    fn mob_mut(&mut self, index: usize) -> Option<&mut Instance> {
        self.list.get_mut(index)
    }

    /// Toggle the particle-emitter bundle registered under `key` (a
    /// `particle_emitters.json` row, any namespace) on the mob at `index`.
    /// `false` for a bad index, an unregistered key, or an activation past the
    /// per-mob cap. Keeps `list` private, like [`damage_mob`](Self::damage_mob).
    pub fn set_mob_emitter(&mut self, index: usize, key: &str, active: bool) -> bool {
        let Some(bundle) = crate::particle_emitters::by_key(key) else {
            return false;
        };
        // A one-shot burst bundle is an event, not attachable state.
        if bundle.burst.is_some() {
            return false;
        }
        self.mob_mut(index)
            .is_some_and(|m| m.set_emitter_active(bundle.id, active))
    }

    /// Toggle a NAMED model animation on the mob at `index` — the animation
    /// sibling of [`set_mob_emitter`](Self::set_mob_emitter). `false` for a
    /// bad index or an activation past the per-mob cap. The name is not
    /// validated against the model (the sim never loads models); the renderer
    /// skips unknown names.
    pub fn set_mob_anim(&mut self, index: usize, name: &str, active: bool) -> bool {
        self.mob_mut(index)
            .is_some_and(|m| m.set_anim_active(name, active))
    }

    /// Set an ACTIVE named animation's playback rate on the mob at `index`
    /// (see `Instance::set_anim_rate`): `0` freezes the layer mid-stroke,
    /// negative reverses. `false` for a bad index or an inactive anim.
    pub fn set_mob_anim_rate(&mut self, index: usize, name: &str, rate: f32) -> bool {
        self.mob_mut(index).is_some_and(|m| m.set_anim_rate(name, rate))
    }

    /// Seek an ACTIVE named animation's phase on the mob at `index` toward
    /// the absolute `target` at `|rate|`/s, landing exactly (see
    /// `Instance::set_anim_seek`). `false` for a bad index or an inactive
    /// anim.
    pub fn set_mob_anim_seek(&mut self, index: usize, name: &str, target: f32, rate: f32) -> bool {
        self.mob_mut(index)
            .is_some_and(|m| m.set_anim_seek(name, target, rate))
    }

    /// Authoritative playback state of an ACTIVE named animation on the mob
    /// at `index`. `None` covers a bad index or inactive name.
    pub fn mob_anim_state(&self, index: usize, name: &str) -> Option<&super::instance::AnimLayer> {
        self.list.get(index)?.anim_state(name)
    }

    /// Latch a mod's kinematic locomotion intent on the mob at `index` for
    /// THIS tick (see [`Instance::set_drive`]): a horizontal world-space
    /// velocity plus optionally an absolute yaw (the mob-facing convention:
    /// yaw `0` faces `-Z`, facing `(-sin yaw, 0, -cos yaw)`). `false` for a
    /// bad index or a dead mob.
    pub fn set_mob_drive(
        &mut self,
        index: usize,
        vel_x: f32,
        vel_z: f32,
        yaw: Option<f32>,
    ) -> bool {
        self.mob_mut(index)
            .is_some_and(|m| m.set_drive(vel_x, vel_z, yaw))
    }

    /// Shear the mob at `index`: `Some` drop when it is a coated shearable species
    /// (its coat is hidden and the regrow countdown starts), else `None`. Keeps
    /// `list` private, like [`damage_mob`](Self::damage_mob).
    pub fn shear_mob(&mut self, index: usize) -> Option<ShearDrop> {
        let mob = self.mob_mut(index)?;
        let spec = super::def(mob.kind).shear?;
        let count = mob.shear()?;
        Some(ShearDrop {
            item: spec.drop,
            count,
            pos: mob.pos,
            skylight: mob.skylight,
            blocklight: mob.blocklight,
        })
    }

    /// Run one natural-spawn step: a single spawn attempt at a random loaded position.
    /// Called once per game tick by `Game`, after [`tick`](Self::tick). Returns the
    /// spawns actually performed (kind + feet position), for the caller to report as
    /// `mob_spawned` events.
    ///
    /// Mobs that leave the loaded area are no longer dropped here — they are saved into
    /// their chunk as it unloads (see [`take_in_chunk`](Self::take_in_chunk)) and reload
    /// with it. Because the unload harvests them out of the live set, the set still only
    /// holds loaded-area mobs, so the "in the loaded area" caps stay honest — provided
    /// the spawn-relevant area is actually loaded. While saved records within the
    /// nine-chunk census neighborhood are still streaming back in, the attempt holds
    /// off, or every join would refill the caps before those nearby mobs restore.
    pub fn spawn_tick(&mut self, world: &World, player_pos: Vec3) -> Vec<(Mob, Vec3)> {
        // Disjoint borrows: the room test reads the live list, the picker draws `rng`.
        let list = &self.list;
        let chosen = spawn::attempt(world, player_pos, &mut self.rng, |kind| {
            spawn::room_for(list, kind)
        });
        let mut spawned = Vec::new();
        if let Some(spawns) = chosen {
            for s in spawns {
                let c = crate::mathh::voxel_at(s.pos + Vec3::new(0.0, 0.3, 0.0));
                let sky = world.skylight6_at_world(c.x, c.y, c.z);
                let block = world.blocklight6_at_world(c.x, c.y, c.z);
                if self.spawn_lit(s.kind, s.pos, s.yaw, sky, block) {
                    spawned.push((s.kind, s.pos));
                }
            }
        }
        spawned
    }

    /// Run one worldgen-population step around `player_pos` (see [`populate`]):
    /// roll a budgeted batch of nearby unchecked chunks and place their one-time
    /// herds, ignoring the population caps (worldgen stock — only the [`MAX_MOBS`]
    /// memory backstop applies). Returns the spawns performed plus the chunks to
    /// record as populated; the caller owns the persisted populated set, and a
    /// chunk is only recorded once at least one member actually spawned, so a
    /// fully-failed placement retries in a later session.
    pub fn populate_tick(
        &mut self,
        world: &World,
        player_pos: Vec3,
    ) -> (Vec<(Mob, Vec3)>, Vec<ChunkPos>) {
        let herds = populate::attempt(world, player_pos, &mut self.populate_checked);
        let mut spawned = Vec::new();
        let mut populated = Vec::new();
        for herd in herds {
            let mut any = false;
            for s in herd.spawns {
                let c = voxel_at(s.pos + Vec3::new(0.0, 0.3, 0.0));
                let sky = world.skylight6_at_world(c.x, c.y, c.z);
                let block = world.blocklight6_at_world(c.x, c.y, c.z);
                if self.spawn_lit(s.kind, s.pos, s.yaw, sky, block) {
                    spawned.push((s.kind, s.pos));
                    any = true;
                }
            }
            if any {
                populated.push(herd.chunk);
            }
        }
        (spawned, populated)
    }

    /// Drain and return the live mobs resting in section `pos`, as [`SavedMob`]s — used
    /// to bundle them into that section's save record as it unloads. A dead/ragdolling
    /// corpse in the section is removed too, but *not* saved: a corpse is ephemeral (its
    /// loot already dropped when it died), so only living mobs persist.
    pub fn take_in_section(&mut self, pos: SectionPos) -> Vec<SavedMob> {
        let mut taken = Vec::new();
        let mut i = self.list.len();
        while i > 0 {
            i -= 1;
            let c = voxel_at(self.list[i].pos);
            if SectionPos::from_world(c.x, c.y, c.z) == Some(pos) {
                let mob = self.list.swap_remove(i);
                if !mob.is_dead() {
                    taken.push(SavedMob::of(&mob));
                }
            }
        }
        taken
    }

    /// Clone the live mobs grouped by owning section (as [`SavedMob`]s), for the periodic
    /// save flush — the mobs stay active; the clones persist with the section records so a
    /// crash can't lose them. Dead corpses are skipped, as in
    /// [`take_in_section`](Self::take_in_section). A mob outside the world vertical range
    /// (none in normal play) is skipped.
    pub fn saved_by_section(&self) -> HashMap<SectionPos, Vec<SavedMob>> {
        let mut map: HashMap<SectionPos, Vec<SavedMob>> = HashMap::new();
        for m in &self.list {
            if m.is_dead() {
                continue;
            }
            let c = voxel_at(m.pos);
            if let Some(pos) = SectionPos::from_world(c.x, c.y, c.z) {
                map.entry(pos).or_default().push(SavedMob::of(m));
            }
        }
        map
    }

    /// Re-spawn mobs read back from a section's save record now that its section has
    /// loaded. Each gets a fresh AI brain (a reloaded owl simply resumes wandering) and
    /// is subject to the mob cap like any spawn. The saved shear-regrow counter and mod
    /// KV carry over, so a shorn sheep reloads shorn and mod data survives.
    pub fn restore(&mut self, mobs: impl IntoIterator<Item = SavedMob>) {
        for m in mobs {
            self.restore_saved_mob_lit(m, 63, 0);
        }
    }

    pub(crate) fn restore_saved_mob_lit(&mut self, m: SavedMob, skylight: u8, blocklight: u8) {
        if self.spawn_lit(m.kind, m.pos, m.yaw, skylight, blocklight) {
            if let Some(inst) = self.list.last_mut() {
                inst.set_shear_regrow(m.shear_regrow);
                *inst.mod_kv_mut() = m.kv;
            }
        }
    }

    /// Remove the mob at `index` from the live set immediately — the mod
    /// `DespawnMob` HostCall (no death, no loot, not saved). `swap_remove`, so
    /// it renumbers the last mob into the hole; callers must re-query indices.
    pub fn remove(&mut self, index: usize) -> bool {
        if index < self.list.len() {
            self.list.swap_remove(index);
            true
        } else {
            false
        }
    }

    /// A live mob's mod KV entry (see [`Instance::mod_kv`]).
    pub fn mod_kv_get(&self, index: usize, key: &str) -> Option<&[u8]> {
        self.list.get(index)?.mod_kv().get(key).map(Vec::as_slice)
    }

    /// Store a mod KV entry on the mob at `index`; `false` = no such mob.
    pub fn mod_kv_set(&mut self, index: usize, key: String, value: Vec<u8>) -> bool {
        match self.list.get_mut(index) {
            Some(m) => {
                m.mod_kv_mut().insert(key, value);
                true
            }
            None => false,
        }
    }

    /// Remove a mod KV entry from the mob at `index`; returns whether it was
    /// present.
    pub fn mod_kv_remove(&mut self, index: usize, key: &str) -> bool {
        self.list
            .get_mut(index)
            .is_some_and(|m| m.mod_kv_mut().remove(key).is_some())
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

/// Whether the chunk `mob` stands over is loaded — the freeze gate shared by the tick
/// loop and the push pass, so a mob over not-yet-generated terrain is skipped by both.
fn chunk_loaded_at(world: &World, mob: &Instance) -> bool {
    let c = voxel_at(mob.pos);
    world.chunk_loaded(c.x >> 4, c.z >> 4)
}

/// Whether `mob` takes part in soft pushing this tick: it must be alive (a corpse
/// ragdolls in place — its `pos` is the ragdoll origin, so shoving it would warp the
/// corpse) and actually simulating (not frozen over an unloaded chunk).
fn is_pushable(mob: &Instance, world: &World, freeze_unloaded: bool) -> bool {
    !mob.is_dead() && (!freeze_unloaded || chunk_loaded_at(world, mob))
}

#[cfg(test)]
mod tests {

    #[test]
    fn mobs_anchor_on_the_nearest_player() {
        use super::PlayerAnchor;
        let a = PlayerAnchor {
            id: crate::server::player::PlayerId(0),
            pos: Vec3::new(0.0, 64.0, 0.0),
            body: None,
            sneaking: false,
        };
        let b = PlayerAnchor {
            id: crate::server::player::PlayerId(1),
            pos: Vec3::new(10.0, 64.0, 0.0),
            body: None,
            sneaking: false,
        };
        let near_b = Vec3::new(8.0, 64.0, 0.0);
        assert_eq!(super::nearest_anchor(&[a, b], near_b).id.0, 1);
        assert_eq!(
            super::nearest_anchor(&[b, a], near_b).id.0,
            1,
            "order-independent"
        );
        let near_a = Vec3::new(1.0, 64.0, 0.0);
        assert_eq!(super::nearest_anchor(&[a, b], near_a).id.0, 0);
    }

    #[test]
    fn a_frozen_tick_discards_its_drive_intent() {
        let world = World::new(0, 1);
        let mut mobs = Mobs::new(0);
        assert!(mobs.spawn(Mob::Owl, Vec3::new(8.5, 64.0, 8.5), 0.0));
        assert!(mobs.set_mob_drive(0, 2.0, 0.0, Some(1.0)));
        assert!(mobs.instances()[0].drive_pending());

        mobs.tick(
            0.05,
            &world,
            &[PlayerAnchor {
                id: Default::default(),
                pos: Vec3::new(0.0, 64.0, 0.0),
                body: None,
                sneaking: false,
            }],
            true,
        );

        assert!(
            !mobs.instances()[0].drive_pending(),
            "a skipped integration cannot carry this tick's command forward"
        );
    }
    use super::*;

    #[test]
    fn take_in_section_harvests_only_that_sections_mobs() {
        let mut mobs = Mobs::new(0);
        // y=64 → cy 4. x 2.5 → cx 0; x 20.5 → cx 1.
        assert!(mobs.spawn(Mob::Owl, Vec3::new(2.5, 64.0, 2.5), 0.5)); // section (0,4,0)
        assert!(mobs.spawn(Mob::Owl, Vec3::new(20.5, 64.0, 2.5), 1.0)); // section (1,4,0)

        let taken = mobs.take_in_section(SectionPos::new(0, 4, 0));
        assert_eq!(taken.len(), 1, "only the (0,4,0) owl is harvested");
        assert_eq!(taken[0].kind, Mob::Owl);
        assert_eq!(taken[0].pos, Vec3::new(2.5, 64.0, 2.5));
        assert_eq!(taken[0].yaw, 0.5, "facing is captured");
        assert_eq!(mobs.len(), 1, "the (1,4,0) owl stays live");
    }

    #[test]
    fn saved_by_section_groups_live_mobs_without_removing_them() {
        let mut mobs = Mobs::new(0);
        assert!(mobs.spawn(Mob::Owl, Vec3::new(2.5, 64.0, 2.5), 0.0)); // (0,4,0)
        assert!(mobs.spawn(Mob::Owl, Vec3::new(5.5, 64.0, 9.5), 0.0)); // (0,4,0)
        assert!(mobs.spawn(Mob::Owl, Vec3::new(20.5, 64.0, 2.5), 0.0)); // (1,4,0)

        let map = mobs.saved_by_section();
        assert_eq!(map[&SectionPos::new(0, 4, 0)].len(), 2);
        assert_eq!(map[&SectionPos::new(1, 4, 0)].len(), 1);
        assert_eq!(mobs.len(), 3, "the flush clones; the mobs stay live");
    }

    #[test]
    fn restore_respawns_saved_mobs_with_their_pose() {
        let mut mobs = Mobs::new(0);
        mobs.restore([
            SavedMob {
                kind: Mob::Owl,
                pos: Vec3::new(8.5, 70.0, 8.5),
                yaw: 1.25,
                shear_regrow: 0,
                kv: Default::default(),
            },
            SavedMob {
                kind: Mob::Sheep,
                pos: Vec3::new(9.5, 70.0, 8.5),
                yaw: -0.5,
                shear_regrow: 500,
                kv: Default::default(),
            },
        ]);
        assert_eq!(mobs.len(), 2);
        let poses: Vec<(Vec3, f32)> = mobs.instances().iter().map(|m| (m.pos, m.yaw)).collect();
        assert!(
            poses.contains(&(Vec3::new(8.5, 70.0, 8.5), 1.25)),
            "first mob restored in place"
        );
        assert!(
            poses.contains(&(Vec3::new(9.5, 70.0, 8.5), -0.5)),
            "second mob restored in place"
        );
        let shorn: Vec<bool> = mobs.instances().iter().map(Instance::is_shorn).collect();
        assert!(
            shorn.contains(&true) && shorn.contains(&false),
            "a saved regrow counter carries over on restore: {shorn:?}"
        );
    }

    #[test]
    fn mob_mod_kv_survives_section_unload_and_reload() {
        // The unload → save-record → reload cycle at the manager level: a mod
        // KV entry set on a live mob rides its SavedMob projection and is back
        // on the restored instance (the on-disk byte layer is covered by
        // `save::mobs`).
        let mut mobs = Mobs::new(0);
        assert!(mobs.spawn(Mob::Owl, Vec3::new(2.5, 64.0, 2.5), 0.5));
        assert!(mobs.mod_kv_set(0, "zombies:anger".into(), vec![3, 1]));
        assert_eq!(mobs.mod_kv_get(0, "zombies:anger"), Some(&[3u8, 1][..]));

        let taken = mobs.take_in_section(SectionPos::new(0, 4, 0));
        assert_eq!(taken.len(), 1);
        assert_eq!(taken[0].kv.get("zombies:anger"), Some(&vec![3, 1]));
        assert_eq!(mobs.len(), 0, "harvested out of the live set");

        mobs.restore(taken);
        assert_eq!(
            mobs.mod_kv_get(0, "zombies:anger"),
            Some(&[3u8, 1][..]),
            "the KV is back on the restored mob"
        );
        // Removal reports presence honestly; out-of-range indices are inert.
        assert!(mobs.mod_kv_remove(0, "zombies:anger"));
        assert!(!mobs.mod_kv_remove(0, "zombies:anger"));
        assert!(!mobs.mod_kv_set(9, "zombies:anger".into(), vec![1]));
    }

    #[test]
    fn shearing_a_sheep_yields_wool_once_until_the_coat_regrows() {
        let world = World::new(0, 1);
        let mut mobs = Mobs::new(0);
        assert!(mobs.spawn(Mob::Sheep, Vec3::new(8.5, 64.0, 8.5), 0.0));
        let spec = crate::mob::def(Mob::Sheep)
            .shear
            .expect("sheep are shearable");

        let drop = mobs.shear_mob(0).expect("a coated sheep shears");
        assert_eq!(drop.item, spec.drop);
        assert!(
            (spec.min..=spec.max).contains(&drop.count),
            "count rolled inside the spec range: {}",
            drop.count
        );
        assert!(mobs.instances()[0].is_shorn());
        assert!(mobs.shear_mob(0).is_none(), "no double-shear while shorn");

        // The coat regrows on the tick, within the spec's rolled range.
        let mut ticks: u32 = 0;
        while mobs.instances()[0].is_shorn() {
            mobs.tick(
                0.05,
                &world,
                &[crate::mob::PlayerAnchor {
                    id: Default::default(),
                    pos: far(),
                    body: None,
                    sneaking: false,
                }],
                false,
            );
            ticks += 1;
            assert!(
                ticks <= spec.regrow_max,
                "the coat must be back within the max regrow duration"
            );
        }
        assert!(
            ticks >= spec.regrow_min,
            "the coat can't regrow before the min duration: {ticks}"
        );
        assert!(
            mobs.shear_mob(0).is_some(),
            "a regrown sheep can be shorn again"
        );
    }

    #[test]
    fn a_species_without_a_shear_spec_cannot_be_shorn() {
        let mut mobs = Mobs::new(0);
        assert!(mobs.spawn(Mob::Owl, Vec3::new(8.5, 64.0, 8.5), 0.0));
        assert!(mobs.shear_mob(0).is_none());
        assert!(!mobs.instances()[0].is_shorn());
    }

    #[test]
    fn a_corpse_cannot_be_shorn() {
        let mut mobs = Mobs::new(0);
        assert!(mobs.spawn(Mob::Sheep, Vec3::new(8.5, 64.0, 8.5), 0.0));
        assert!(mobs
            .damage_mob(
                0,
                100.0,
                Some(Vec3::new(5.0, 64.0, 8.5)),
                true,
                None,
                &MobDamageFeedback::default()
            )
            .is_some());
        assert!(
            mobs.shear_mob(0).is_none(),
            "a ragdolling corpse keeps its coat"
        );
    }

    /// The horizontal distance between the first two live mobs.
    fn horizontal_gap(mobs: &Mobs) -> f32 {
        let p = mobs.instances();
        let (a, b) = (p[0].pos, p[1].pos);
        ((a.x - b.x).powi(2) + (a.z - b.z).powi(2)).sqrt()
    }

    /// A point far from the origin — used as a parked player anchor / body so a tick
    /// exercises only mob↔mob pushing.
    fn far() -> Vec3 {
        Vec3::new(1000.0, 64.0, 1000.0)
    }

    #[test]
    fn overlapping_mobs_drift_apart_smoothly() {
        // Two owls spawned almost on top of each other must ease apart *gradually* and
        // monotonically — never snapping back (the jitter we're avoiding) — and settle
        // just clear of each other (≈ their combined half-widths), not blow past. The
        // empty world has no floor, so they also fall; the gap checked is horizontal. No
        // player body this tick.
        let world = World::new(0, 1);
        let mut mobs = Mobs::new(0);
        assert!(mobs.spawn(Mob::Owl, Vec3::new(8.0, 64.0, 8.0), 0.0));
        assert!(mobs.spawn(Mob::Owl, Vec3::new(8.05, 64.0, 8.0), 0.0));
        let reach = 2.0 * crate::mob::def(Mob::Owl).size.half_width;

        let gap0 = horizontal_gap(&mobs);
        let mut gap = gap0;
        let mut last_step = f32::INFINITY;
        for _ in 0..40 {
            mobs.tick(
                0.05,
                &world,
                &[crate::mob::PlayerAnchor {
                    id: Default::default(),
                    pos: far(),
                    body: None,
                    sneaking: false,
                }],
                false,
            );
            let next = horizontal_gap(&mobs);
            // No snap-back: the gap only ever grows — the jitter we were getting was the
            // gap oscillating as positions were snapped each tick.
            assert!(
                next >= gap - 1e-4,
                "the gap never shrinks (no snap-back): {gap} -> {next}"
            );
            last_step = next - gap;
            gap = next;
        }
        assert!(
            gap > gap0 + 0.2,
            "the overlapping owls clearly separated: {gap0} -> {gap}"
        );
        assert!(
            gap > 0.9 * reach,
            "they ended up cleanly apart: gap {gap}, reach {reach}"
        );
        assert!(
            gap < 1.3 * reach,
            "they settled at contact, not flung apart: gap {gap}, reach {reach}"
        );
        // Eased to rest: the push fades out as they separate (proportional to the
        // shrinking overlap), so by the end they've coasted to a stop — a gradual drift
        // that converges, not a constant ram.
        assert!(
            last_step < 0.005,
            "the push eases off as they part: final tick step {last_step}"
        );
    }

    #[test]
    fn the_push_pass_records_touch_contacts_both_ways() {
        // The touch perception channel: overlapping bodies land in each
        // other's contact lists (and the player in the mob's), while a
        // distant mob records nothing.
        let world = World::new(0, 1);
        let mut mobs = Mobs::new(0);
        assert!(mobs.spawn(Mob::Owl, Vec3::new(8.0, 64.0, 8.0), 0.0));
        assert!(mobs.spawn(Mob::Owl, Vec3::new(8.1, 64.0, 8.0), 0.0)); // overlapping
        assert!(mobs.spawn(Mob::Owl, Vec3::new(20.0, 64.0, 8.0), 0.0)); // far away
        let ids: Vec<u64> = mobs.instances().iter().map(Instance::id).collect();

        let player = crate::mob::PlayerAnchor {
            id: crate::server::player::PlayerId(3),
            pos: Vec3::new(8.0, 64.9, 8.1),
            body: Some(Body::new(Vec3::new(8.0, 64.0, 8.1), 0.3, 1.8)),
            sneaking: true, // touch is felt, not heard — sneak is irrelevant
        };
        mobs.tick(0.05, &world, &[player], false);

        let contacts: Vec<&[crate::mob::EntityRef]> =
            mobs.instances().iter().map(Instance::contacts).collect();
        assert!(
            contacts[0].contains(&crate::mob::EntityRef::Mob(ids[1]))
                && contacts[1].contains(&crate::mob::EntityRef::Mob(ids[0])),
            "overlapping mobs record each other: {contacts:?}"
        );
        assert!(
            contacts[0].contains(&crate::mob::EntityRef::Player(
                crate::server::player::PlayerId(3)
            )),
            "the touching (sneaking) player is felt: {contacts:?}"
        );
        assert!(
            contacts[2].is_empty(),
            "a distant mob touches nothing: {contacts:?}"
        );
    }

    #[test]
    fn a_mob_overlapping_the_player_pushes_it_away() {
        // The mobs push the player too — but that's a per-frame query (`push_on_player`),
        // not the tick, so the player drifts out smoothly. It points away from the owl.
        let mut mobs = Mobs::new(0);
        // Owl just east (+X) of the player's column, footprints overlapping.
        assert!(mobs.spawn(Mob::Owl, Vec3::new(8.2, 64.0, 8.0), 0.0));
        let player_body = Body::new(Vec3::new(8.0, 64.0, 8.0), 0.3, 1.8);
        let push = mobs.push_on_player(player_body);
        assert!(
            push.x < 0.0,
            "the player is pushed -X, away from the owl: {push:?}"
        );
        assert_eq!(push.y, 0.0, "the push is horizontal");
    }

    #[test]
    fn a_distant_mob_does_not_push_the_player() {
        // No overlap, no push — a mob across the world leaves the player be.
        let mut mobs = Mobs::new(0);
        assert!(mobs.spawn(Mob::Owl, Vec3::new(8.0, 64.0, 8.0), 0.0));
        let player_body = Body::new(far(), 0.3, 1.8);
        assert_eq!(
            mobs.push_on_player(player_body),
            Vec3::ZERO,
            "an out-of-reach mob imparts no push"
        );
    }

    #[test]
    fn a_bodiless_player_does_not_shove_mobs() {
        // A noclip spectator (no push body) overlapping a mob leaves it be — the tick's
        // player→mob shove is skipped when there's no body (the caller likewise skips the
        // per-frame mob→player push for a spectator).
        let world = World::new(0, 1);
        let mut mobs = Mobs::new(0);
        let spot = Vec3::new(8.0, 64.0, 8.0);
        assert!(mobs.spawn(Mob::Owl, spot, 0.0));
        let before = mobs.instances()[0].pos;
        mobs.tick(
            0.05,
            &world,
            &[crate::mob::PlayerAnchor {
                id: Default::default(),
                pos: spot,
                body: None,
                sneaking: false,
            }],
            false,
        );
        let after = mobs.instances()[0].pos;
        assert_eq!(
            (before.x, before.z),
            (after.x, after.z),
            "a player with no body doesn't shove the mob sideways"
        );
    }

    #[test]
    fn a_harvested_corpse_is_dropped_not_saved() {
        let mut mobs = Mobs::new(0);
        assert!(mobs.spawn(Mob::Owl, Vec3::new(2.5, 64.0, 2.5), 0.0));
        // Kill it: now a ragdolling corpse. Harvesting its section removes it but does not
        // persist it (its loot already fell when it died).
        assert!(mobs
            .damage_mob(
                0,
                100.0,
                Some(Vec3::new(5.0, 64.0, 2.5)),
                true,
                None,
                &MobDamageFeedback::default()
            )
            .is_some());
        let taken = mobs.take_in_section(SectionPos::new(0, 4, 0));
        assert!(taken.is_empty(), "a corpse is not persisted");
        assert_eq!(mobs.len(), 0, "but it is removed from the live set");
    }

    #[test]
    fn placement_is_blocked_only_where_a_solid_block_clips_a_live_mob() {
        let mut mobs = Mobs::new(0);
        assert!(mobs.spawn(Mob::Owl, Vec3::new(8.5, 64.0, 8.5), 0.0)); // body in cell (8,64,8)
        let here = IVec3::new(8, 64, 8);
        let away = IVec3::new(20, 64, 8);

        // A solid full cube dropped into the owl's cell clips its body.
        assert!(
            mobs.any_overlapping_placement(here, Block::Dirt),
            "a solid block in the owl's cell is blocked"
        );
        // The same cube well clear of the owl is fine.
        assert!(
            !mobs.any_overlapping_placement(away, Block::Dirt),
            "a cell away from the owl is clear"
        );
        // A no-collision block (a torch) never clips anything, even right on the owl.
        assert!(
            !mobs.any_overlapping_placement(here, Block::Torch),
            "a no-collision block is always placeable"
        );

        // A ragdolling corpse doesn't block placement (it's about to vanish).
        assert!(mobs
            .damage_mob(
                0,
                100.0,
                Some(Vec3::new(9.0, 64.0, 8.5)),
                true,
                None,
                &MobDamageFeedback::default()
            )
            .is_some());
        assert!(
            !mobs.any_overlapping_placement(here, Block::Dirt),
            "a corpse doesn't block placement"
        );
    }
}
