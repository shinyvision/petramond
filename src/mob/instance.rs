//! A live mob instance: shared kinematics + its brain + its navigator.
//!
//! Everything physical a mob does — gravity, axis-resolved block collision, the jump
//! impulse, turning to face travel, advancing the walk cycle — lives here once and
//! is shared by every species; a species differs only by its [`MobDef`](super::MobDef)
//! stats and
//! its brain's behaviors. One [`tick`](Instance::tick) is one **game tick** (20 TPS):
//! the brain picks a goal, the navigator turns it into a wish-direction + jump, and
//! the kinematics integrate it. The previous tick's pose is snapshotted each tick so
//! the renderer can interpolate between ticks for smooth motion at any frame rate.
//!
//! The `impl Instance` blocks are split by concern across sibling files —
//! [`kinematics`](super::kinematics) (locomotion integration and fall
//! bookkeeping), [`damage`](super::damage) (damage intake and the death
//! lifecycle), and [`anim`](super::anim) (expression + named animation
//! layers) — the `world::store` pattern. This file keeps the struct, spawn,
//! and the per-tick orchestration.

use crate::mathh::{voxel_at, IVec3, Vec3};
use crate::world::World;

use super::anim::AnimKind;
// Re-exported so `mob::instance::AnimLayer` consumers (the manager's
// anim-state readback) keep their path while the type lives with the
// animation impl.
pub use super::anim::AnimLayer;
use super::brain::{AiCtx, AttackIntent, Brain, TickInputs};
use super::damage::DeathState;
use super::kinematics::{route_steering_supported, DriveIntent};
use super::model_meta::{IdleAnimMeta, Skeleton};
use super::nav::Navigator;
use super::path;
use super::{def, EntityRef, Mob, MobRng, PlayerAnchor, DEFAULT_DAMAGE_FLASH_SECS};

/// Default duration used to normalize hurt-flash intensity. Individual feedback
/// components may start shorter or longer flash timers.
const HURT_FLASH_SECS: f32 = DEFAULT_DAMAGE_FLASH_SECS;

/// Hurt-flash intensity in `[0, 1]` from a previous/current hurt-timer pair at
/// `alpha` into the tick — the ONE derivation, shared by the live instance
/// ([`Instance::hurt_flash`]) and the client's replicated-store presentation
/// path (which interpolates consecutive replicated timers).
pub fn hurt_flash01(prev: f32, curr: f32, alpha: f32) -> f32 {
    let t = prev + (curr - prev) * alpha;
    (t / HURT_FLASH_SECS).clamp(0.0, 1.0)
}
/// A mob with a despawn radius that is farther than this from the player is also
/// eligible for *random* despawn each tick — the churn that recycles far unseen
/// hostiles (deep cave spawns) so the population cap keeps freeing room for new
/// spawns near the player. Inside this distance only the hard radius applies.
const RANDOM_DESPAWN_MIN_DIST: f32 = 32.0;
/// Per-tick random-despawn chance once eligible: ~40 s expected lifetime at 20 TPS.
const RANDOM_DESPAWN_CHANCE: f32 = 1.0 / 800.0;

/// A live mob. Render-facing fields (`pos`/`yaw`/`anim_time`/`moving`/`skylight` and
/// their `prev_*` snapshots) are public for the scene adapter; the AI/physics state
/// is private to the `mob` module (shared with the sibling `impl Instance` files).
pub struct Instance {
    /// Stable session identity for this live mob. Unlike its storage index,
    /// this does not change when `Mobs::remove` uses `swap_remove`.
    pub(super) id: u64,
    pub kind: Mob,
    pub pos: Vec3,
    pub yaw: f32,
    /// Seconds into the currently-playing animation (walk or idle_*; free-running, the
    /// renderer wraps it). Reset to 0 when the active animation changes.
    pub anim_time: f32,
    /// Did the mob have walking locomotion this tick? Selects walk vs idle/rest pose.
    pub moving: bool,
    /// Which `idle_*` animation is playing (index), or `None` for walk / neutral rest.
    pub idle_anim: Option<u8>,
    /// Head orientation **relative to the body** (radians), eased toward the head-look
    /// AI's target. The renderer applies it to the model's `head` bone.
    pub head_yaw: f32,
    pub head_pitch: f32,
    pub skylight: u8,
    /// 6-bit block (torch) light sampled alongside `skylight` — night-invariant.
    pub blocklight: u8,
    /// Previous-tick pose, for render interpolation.
    pub prev_pos: Vec3,
    pub prev_yaw: f32,
    pub prev_anim_time: f32,
    pub prev_head_yaw: f32,
    pub prev_head_pitch: f32,
    /// Hurt-flash intensity last tick, for render interpolation.
    pub prev_hurt: f32,

    pub(super) vel: Vec3,
    pub(super) on_ground: bool,
    /// Current health; at `0` the mob enters a dead `DeathState`.
    pub(super) health: f32,
    /// Engine-owned global damage immunity. It is transient like hurt/stagger
    /// presentation and starts clear when a saved mob is restored.
    pub(super) damage_immunity: crate::damage::DamageImmunity,
    /// Highest feet Y reached since the mob last stood/swum. A landing compares this
    /// peak to the landed feet Y to produce deterministic fall damage.
    pub(super) fall_peak_y: f32,
    /// Landing distance latched by [`finish_motion`](Self::finish_motion) and drained by the
    /// manager after the tick so `ServerGame` can route damage through `mob_damage_pre`.
    pub(super) fall_distance: f32,
    /// Fall-INTO-WATER distance latched by [`finish_motion`](Self::finish_motion) and drained
    /// by the manager — `ServerGame` turns it into the water-splash burst.
    pub(super) splash_drop: f32,
    /// True once this mob is beyond its row-level despawn radius this tick. The manager
    /// culls it at the end of the tick. Never persisted.
    pub(super) distance_despawned: bool,
    /// Seconds of hurt flash remaining. Drives the replicated red tint only.
    pub(super) hurt_timer: f32,
    /// Seconds of knockback stagger remaining. Kept separate from the flash timer so
    /// feedback can compose knockback without forcing a red flash, or vice versa.
    pub(super) stagger_timer: f32,
    /// Horizontal knockback velocity (m/s), decaying over the stagger. Kept separate
    /// from `vel` so the per-tick wish-velocity overwrite can't wipe it.
    pub(super) knockback: Vec3,
    /// Soft entity-push velocity (m/s, horizontal) accumulated from overlapping other
    /// entities last tick — added on top of locomotion in [`integrate`](Self::integrate)
    /// and consumed there (the push pass re-derives it each tick from the live overlap).
    /// Kept separate from `vel` for the same reason as `knockback`: the wish-velocity
    /// overwrite would otherwise wipe it.
    pub(super) push: Vec3,
    /// Game ticks of coat regrowth remaining after a shear: while non-zero the mob is
    /// shorn (its coat cubes are hidden and it can't be shorn again); it counts down on
    /// the tick and the coat is back at `0`. Persisted (see [`super::SavedMob`]).
    shear_regrow: u32,
    /// ACTIVE particle-emitter bundles by catalog id (`crate::particle_emitters`),
    /// sorted, at most [`super::MAX_ACTIVE_MOB_EMITTERS`]. Presentation-only
    /// state toggled by mods through the `MobEmitterSet` HostCall, replicated
    /// per tick, and deliberately not persisted (the owning mod re-derives it,
    /// like its other transient decisions). It survives death so a corpse keeps
    /// its effect through the ragdoll.
    active_emitters: Vec<u8>,
    /// ACTIVE named model animations, sorted by name, at most
    /// [`super::MAX_ACTIVE_MOB_ANIMS`] — the animation sibling of
    /// [`active_emitters`](Self::active_emitters): presentation-only state
    /// controlled by mods through the `MobAnimSet`/`MobAnimRate` HostCalls,
    /// replicated per tick (name + phase), never persisted. Each layer is
    /// SELF-CLOCKED: its phase advances by `rate` per second on the tick —
    /// rate 0 freezes it mid-stroke (an oar pauses in place, never snaps
    /// home), negative reverses. Names are the mob model's own animation
    /// names; the renderer layers every active one over the walk/idle/rest
    /// base pose and silently skips names the model doesn't have.
    pub(super) active_anims: Vec<AnimLayer>,
    /// A mod's kinematic locomotion intent for THIS tick (the `MobDrive`
    /// HostCall), consumed by [`integrate_with_flow`](Self::integrate_with_flow):
    /// while present it replaces the brain's wish-velocity overwrite, so a mod
    /// can drive a mob directly (a vehicle) with the engine still owning
    /// vertical physics (gravity/buoyancy) and collision. Like the brain's
    /// wish it must be re-set every tick — a disabled mod's vehicle simply
    /// stops. Never persisted.
    pub(super) drive: Option<DriveIntent>,
    /// Per-mob mod KV (`mod_id:key` → bytes) — opaque to the engine, written
    /// by mod HostCalls on the tick, persisted with the mob's save record
    /// (see [`super::SavedMob`]). BTreeMap so the save encoding is deterministic.
    mod_kv: std::collections::BTreeMap<String, Vec<u8>>,
    /// Once the mob has died it runs no AI and takes no further damage. The default
    /// death presentation is a ragdoll, but a custom feedback bundle may omit it.
    pub(super) death: DeathState,
    /// The animation kind playing last tick, to detect changes (and reset `anim_time`).
    pub(super) anim_kind: AnimKind,
    /// A melee strike the brain wants landed THIS tick — latched during
    /// [`tick`](Self::tick), drained by the manager into a
    /// [`MobAttack`](super::MobAttack). Never persisted; cleared every tick.
    attack: Option<AttackIntent>,
    /// The target the brain settled on last tick (the merged
    /// `BehaviorOutput::target`), fed back as `AiCtx::target` so attack nodes
    /// strike what the winning perception node locked. Transient AI state,
    /// never persisted (a reloaded mob re-perceives, like its navigation).
    current_target: Option<EntityRef>,
    /// Who last damaged this mob + ticks since — the retaliation input,
    /// recorded by [`damage`](Self::damage). Ages out on the node's own
    /// memory policy; never persisted.
    pub(super) attacker: Option<EntityRef>,
    pub(super) attacker_ticks: u32,
    /// The entities whose bodies overlapped this mob, recorded by the
    /// manager's push pass each tick and read by the NEXT tick's AI as
    /// `AiCtx::contacts` (the touch perception channel). Never persisted.
    contacts: Vec<EntityRef>,
    brain: Brain,
    nav: Navigator,
    pub(super) rng: MobRng,
}

impl Instance {
    /// Spawn a mob of `kind` at `pos` (feet) facing `yaw`. `seed` makes its AI
    /// deterministic and distinct per mob.
    pub fn new(kind: Mob, pos: Vec3, yaw: f32, seed: u64) -> Self {
        let d = def(kind);
        Instance {
            id: seed,
            kind,
            pos,
            yaw,
            anim_time: 0.0,
            moving: false,
            idle_anim: None,
            head_yaw: 0.0,
            head_pitch: 0.0,
            skylight: 63,
            blocklight: 0,
            prev_pos: pos,
            prev_yaw: yaw,
            prev_anim_time: 0.0,
            prev_head_yaw: 0.0,
            prev_head_pitch: 0.0,
            prev_hurt: 0.0,
            vel: Vec3::ZERO,
            on_ground: false,
            health: d.max_health,
            damage_immunity: Default::default(),
            fall_peak_y: pos.y,
            fall_distance: 0.0,
            splash_drop: 0.0,
            distance_despawned: false,
            hurt_timer: 0.0,
            stagger_timer: 0.0,
            knockback: Vec3::ZERO,
            push: Vec3::ZERO,
            shear_regrow: 0,
            active_emitters: Vec::new(),
            active_anims: Vec::new(),
            drive: None,
            mod_kv: std::collections::BTreeMap::new(),
            death: DeathState::Alive,
            anim_kind: AnimKind::Rest,
            attack: None,
            current_target: None,
            attacker: None,
            attacker_ticks: 0,
            contacts: Vec::new(),
            brain: super::build_brain(d),
            nav: Navigator::new(d.size.head_cells(), d.size.half_width),
            rng: MobRng::new(seed),
        }
    }

    /// Stable session identity for this live mob. This is the value exposed to
    /// mods; storage indices remain tick-local.
    #[inline]
    pub fn id(&self) -> u64 {
        self.id
    }

    /// Take the melee strike the brain latched this tick, if any — the manager
    /// drains it right after [`tick`](Self::tick) into a
    /// [`MobAttack`](super::MobAttack) for `Game` to apply.
    #[inline]
    pub(super) fn take_attack(&mut self) -> Option<AttackIntent> {
        self.attack.take()
    }

    /// The active particle-emitter bundle ids, sorted (catalog session ids —
    /// `crate::particle_emitters`).
    #[inline]
    pub fn active_emitters(&self) -> &[u8] {
        &self.active_emitters
    }

    /// Toggle one emitter bundle by catalog id (the manager resolves keys —
    /// see [`super::Mobs::set_mob_emitter`]). Returns `false` only when an
    /// activation would exceed [`super::MAX_ACTIVE_MOB_EMITTERS`].
    pub(super) fn set_emitter_active(&mut self, id: u8, active: bool) -> bool {
        match (self.active_emitters.binary_search(&id), active) {
            (Ok(_), true) | (Err(_), false) => true,
            (Ok(at), false) => {
                self.active_emitters.remove(at);
                true
            }
            (Err(at), true) => {
                if self.active_emitters.len() >= super::MAX_ACTIVE_MOB_EMITTERS {
                    return false;
                }
                self.active_emitters.insert(at, id);
                true
            }
        }
    }

    /// The mob's centre-square body projection. Systems that need the complete
    /// long-body footprint use `mob::body_geometry` instead.
    pub fn aabb(&self) -> (Vec3, Vec3) {
        self.body().aabb()
    }

    /// This mob's gameplay body (feet at `pos`, sized to its species).
    pub(super) fn body(&self) -> crate::body::Body {
        let s = def(self.kind).size;
        crate::body::Body::new(self.pos, s.half_width, s.height)
    }

    /// Replace this mob's touch-contact record (see the field docs) — the
    /// manager's push pass writes it from the same overlap tests that compute
    /// the pushes. The buffer is reused; nothing allocates on a quiet tick.
    pub(super) fn set_contacts(&mut self, contacts: impl IntoIterator<Item = EntityRef>) {
        self.contacts.clear();
        self.contacts.extend(contacts);
    }

    /// The entities whose bodies overlapped this mob last tick.
    #[inline]
    pub fn contacts(&self) -> &[EntityRef] {
        &self.contacts
    }

    /// Is the mob currently shorn (its coat still regrowing)? The renderer hides the
    /// model's coat cubes while this holds.
    #[inline]
    pub fn is_shorn(&self) -> bool {
        self.shear_regrow > 0
    }

    /// Ticks of coat regrowth remaining (`0` = fully coated), for the save record.
    #[inline]
    pub(super) fn shear_regrow(&self) -> u32 {
        self.shear_regrow
    }

    /// Restore a saved regrow counter onto a freshly respawned mob (reload path).
    #[inline]
    pub(super) fn set_shear_regrow(&mut self, ticks: u32) {
        self.shear_regrow = ticks;
    }

    /// The mob's mod KV entries (see the field docs).
    #[inline]
    pub fn mod_kv(&self) -> &std::collections::BTreeMap<String, Vec<u8>> {
        &self.mod_kv
    }

    /// Mutable mod KV access, for the manager's KV HostCall entry points and
    /// the save-restore path.
    #[inline]
    pub(super) fn mod_kv_mut(&mut self) -> &mut std::collections::BTreeMap<String, Vec<u8>> {
        &mut self.mod_kv
    }

    /// Shear this mob: roll how many of its [`ShearSpec`](super::ShearSpec) drop it
    /// yields and start the regrow countdown. `None` when the species can't be shorn,
    /// the coat is still regrowing, or the mob is dead.
    pub(super) fn shear(&mut self) -> Option<u8> {
        let spec = def(self.kind).shear?;
        if self.death.is_dead() || self.shear_regrow > 0 {
            return None;
        }
        let count = self
            .rng
            .next_range(spec.min.min(spec.max) as i32, spec.max as i32) as u8;
        self.shear_regrow = self.rng.next_range(
            spec.regrow_min.min(spec.regrow_max) as i32,
            spec.regrow_max as i32,
        ) as u32;
        Some(count)
    }

    /// Advance one game tick: snapshot the previous pose, let the brain pick a goal,
    /// have the navigator steer toward it, and integrate the kinematics. A dead mob
    /// runs no AI — only its death ragdoll advances.
    ///
    /// `inputs` is the tick-wide shared perception state; `anchor` is the player
    /// nearest this mob (the default target for player-anchored decisions).
    pub fn tick(
        &mut self,
        dt: f32,
        inputs: &TickInputs,
        anchor: &PlayerAnchor,
        mob_index: usize,
        despawn_radius: Option<f32>,
        idle_anims: &[IdleAnimMeta],
        skeleton: &Skeleton,
    ) -> Option<(bool, Vec3)> {
        let world: &World = inputs.world;
        let player_pos = anchor.pos;
        self.prev_pos = self.pos;
        self.prev_yaw = self.yaw;
        self.prev_anim_time = self.anim_time;
        self.prev_head_yaw = self.head_yaw;
        self.prev_head_pitch = self.head_pitch;
        self.prev_hurt = self.hurt_timer;
        // The attack latch is strictly this-tick state: clear before any early
        // return so a mob that died mid-swing can't land a stale strike.
        self.attack = None;

        let d = def(self.kind);

        // Dead: freeze the body (pos/yaw stay put — they're the ragdoll's `global`) and
        // advance only the physics ragdoll. No brain, no locomotion. The kill's red flash
        // still fades out over these first ticks.
        if self.death.is_dead() {
            self.drive = None;
            self.hurt_timer = (self.hurt_timer - dt).max(0.0);
            self.stagger_timer = (self.stagger_timer - dt).max(0.0);
            if matches!(self.death, DeathState::Ragdoll(_)) {
                self.tick_ragdoll(dt, world, d, skeleton);
            }
            return None;
        }

        // Hurt flash and knockback stagger count down independently on the fixed tick
        // (frame-rate independent).
        if self.hurt_timer > 0.0 {
            self.hurt_timer = (self.hurt_timer - dt).max(0.0);
        }
        if self.stagger_timer > 0.0 {
            self.stagger_timer = (self.stagger_timer - dt).max(0.0);
        }

        // Attacker memory ages on the tick; the retaliation node applies its own
        // forget policy against this counter.
        if self.attacker.is_some() {
            self.attacker_ticks = self.attacker_ticks.saturating_add(1);
        }

        // Shear regrowth counts down on the tick; at zero the coat is back. Pauses
        // with the rest of the sim while the mob's chunk is unloaded (this tick is
        // simply not run), like the dropped-item timers.
        self.shear_regrow = self.shear_regrow.saturating_sub(1);

        // Distance-despawn: a mob with a row-level radius is culled immediately once it
        // is outside that radius, and randomly once beyond the eligibility distance.
        // Species with no radius persist while loaded.
        if let Some(radius) = despawn_radius {
            let dist2 = (self.pos - player_pos).length_squared();
            // The roll is drawn only when eligible, so a near mob's brain RNG
            // stream is untouched by this rule.
            self.distance_despawned = despawn_now(dist2, radius, || self.rng.next_f32());
        } else {
            self.distance_despawned = false;
        }

        let solid = |c: IVec3| world.blocks_movement_at(c.x, c.y, c.z);
        // The model-aware box source for body collision (legs/top of a bbmodel block); the
        // cell-based `solid` above still drives navigation (foothold/pathfinding/ledge).
        let boxes = |x: i32, y: i32, z: i32| world.collision_boxes_at(x, y, z);
        let water = |c: IVec3| world.water_cell_at(c.x, c.y, c.z);
        // On or in water — feet submerged, or resting on the surface (water just
        // below). Stays true while the mob floats at the surface; drives idle-animation
        // suppression and allows path refreshes while swimming.
        let feet_cell = voxel_at(self.pos);
        let in_water = water(feet_cell) || water(feet_cell - IVec3::Y);
        // The cell navigation starts from: the standing foothold on land (robust to
        // standing at a block edge, where the cell under the centre overhangs into
        // air), or the water-surface cell while in water — see `navigation_cell` for
        // why a mob in water must never path from its (submerged) standing cell.
        let cell = path::navigation_cell(
            self.pos,
            d.size.half_width,
            d.size.head_cells(),
            in_water,
            &solid,
            &water,
        )
        .unwrap_or_else(|| voxel_at(self.pos));
        let nav_idle = self.nav.is_idle();
        let decision = {
            let mut ctx = AiCtx {
                mob_id: self.id,
                pos: self.pos,
                cell,
                yaw: self.yaw,
                head_height: d.size.height,
                half_width: d.size.half_width,
                world,
                player_id: anchor.id,
                player_pos,
                player_sneaking: anchor.sneaking,
                players: inputs.players,
                noises: inputs.noises,
                contacts: &self.contacts,
                target: self.current_target,
                attacker: self.attacker.map(|who| (who, self.attacker_ticks)),
                nav_idle,
                in_water,
                head: d.size.head_cells(),
                idle_anims,
                mob_index,
                mobs: inputs.mobs,
                rng: &mut self.rng,
            };
            self.brain.decide(&mut ctx)
        };
        self.attack = decision.attack;
        self.current_target = decision.target;
        let can_repath = self.on_ground || in_water;
        let can_steer = route_steering_supported(self.on_ground, in_water, self.vel.y);
        self.nav
            .update_goal_when_supported(decision.goal, cell, world, can_repath);
        let (wish, jump) = if can_steer {
            self.nav.follow(self.pos, self.on_ground)
        } else {
            (Vec3::ZERO, false)
        };
        let water_flow = |p: Vec3| world.water_flow_at_point(p);
        let water_surface = |c: IVec3| world.water_surface_y_world(c);
        let was_on_ground = self.on_ground;
        let motion_start = self.pos;
        let healed = self.integrate_with_flow(
            dt,
            d,
            wish,
            jump,
            can_steer,
            &boxes,
            inputs.solid,
            inputs.solid_heal,
            &solid,
            &water,
            &water_surface,
            &water_flow,
        );
        self.apply_expression(dt, d, &decision);
        Some((was_on_ground, motion_start + Vec3::Y * healed))
    }
}

/// The per-tick despawn decision for a mob with despawn radius `radius` at squared
/// player distance `dist2`: certain at/beyond the hard radius, a small random chance
/// (`roll`, drawn lazily in `[0, 1)`) once beyond the eligibility distance, never
/// closer than that. Factored out pure so the eligibility rules are tested without
/// simulating a mob.
fn despawn_now(dist2: f32, radius: f32, roll: impl FnOnce() -> f32) -> bool {
    if dist2 >= radius * radius {
        return true;
    }
    dist2 >= RANDOM_DESPAWN_MIN_DIST * RANDOM_DESPAWN_MIN_DIST && roll() < RANDOM_DESPAWN_CHANCE
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn despawn_is_certain_at_radius_random_when_far_never_when_near() {
        let r = 128.0;
        // At/beyond the hard radius: certain, no roll consumed.
        assert!(despawn_now(r * r, r, || unreachable!(
            "no roll at the hard radius"
        )));
        // Beyond the eligibility distance but inside the radius: decided by the roll.
        let far2 = (RANDOM_DESPAWN_MIN_DIST + 1.0).powi(2);
        assert!(despawn_now(far2, r, || 0.0));
        assert!(!despawn_now(far2, r, || RANDOM_DESPAWN_CHANCE));
        // Near the player: never, and the RNG stream is untouched.
        let near2 = (RANDOM_DESPAWN_MIN_DIST - 1.0).powi(2);
        assert!(!despawn_now(near2, r, || unreachable!(
            "no roll near the player"
        )));
    }
}
