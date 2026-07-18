use std::sync::LazyLock;

use crate::body::Body;
use crate::mathh::{voxel_at, Vec3};
use crate::mob::brain::{AiMob, TickInputs};
use crate::mob::model_meta::{self, IdleAnimMeta, Skeleton};
use crate::mob::noise::{Noise, NoiseKind};
use crate::mob::{def, defs, model, EntityRef, Instance, Mob};
use crate::world::World;

use super::{nearest_anchor, Mobs};

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
    /// The player's selected (held) item — the visible-to-the-world hand
    /// fact behaviors may react to (a wheat lure). `None` for an empty hand
    /// or a spectator (who shows nothing to the world).
    pub held: Option<crate::item::ItemType>,
}

/// A neutral anchor (player 0 at the origin, bodiless, empty-handed) — the
/// base tests override per field, so a new perception fact costs one field
/// here instead of a struct-literal edit at every anchor site.
impl Default for PlayerAnchor {
    fn default() -> Self {
        PlayerAnchor {
            id: Default::default(),
            pos: Vec3::ZERO,
            body: None,
            sneaking: false,
            held: None,
        }
    }
}

#[derive(Copy, Clone)]
pub(super) struct PushBody {
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

impl Mobs {
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
