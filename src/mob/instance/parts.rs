//! The cohesive state components an [`Instance`](super::Instance) is built
//! from — one per concern, so each phase of the tick (see `instance::tick`)
//! and each sibling `impl Instance` file borrows the state it works on by
//! name instead of reaching across one flat struct:
//!
//! - [`Interp`]: last tick's render pose, for interpolation;
//! - [`Motion`]: the body's kinematic state and this tick's mod intents;
//! - [`Combat`]: vitality feedback — immunity, flash, stagger, death, the
//!   strike latched this tick and the retaliation memory;
//! - [`Mind`]: the brain, its navigator and the decision state they carry;
//! - [`Confinement`]: the periodic "is this mob penned?" verdict;
//! - [`Presentation`]: mod-controlled presentation and the expression clock.
//!
//! None of it is persisted: what a mob saves is its tag map and container,
//! which stay on the instance.

use std::sync::Arc;

use petramond_math::math::{IVec3, Tilt, Vec3};
use petramond_math::world_pos::WorldPos;

use super::super::anim::{AnimKind, AnimLayer};
use super::super::brain::{AttackIntent, BehaviorOutput, Brain, HeadLook};
use super::super::confined::{self, ConfinedRegion, RegionCache};
use super::super::damage::DeathState;
use super::super::kinematics::{DriveIntent, KinematicPose};
use super::super::nav::{Navigator, Unstick};
use super::super::EntityRef;

/// Last tick's render pose: the renderer interpolates from here to the
/// instance's current pose, so motion is smooth at any frame rate.
#[derive(Copy, Clone, Debug)]
#[allow(dead_code)] // Retained as one complete pose for future interpolation consumers.
pub(in crate::mob) struct Interp {
    pub pos: WorldPos,
    pub yaw: f32,
    pub tilt: Tilt,
    pub anim_time: f32,
    pub head_yaw: f32,
    pub head_pitch: f32,
    /// The hurt-flash timer.
    pub hurt: f32,
}

impl Interp {
    /// A pose that has always been where it is (a newborn's).
    pub fn at_rest(pos: WorldPos, yaw: f32) -> Self {
        Interp {
            pos,
            yaw,
            tilt: Tilt::LEVEL,
            anim_time: 0.0,
            head_yaw: 0.0,
            head_pitch: 0.0,
            hurt: 0.0,
        }
    }
}

/// The body's kinematic state and this tick's mod-issued intents — what the
/// locomotion integration (`mob::kinematics`) reads and writes.
pub(in crate::mob) struct Motion {
    pub vel: Vec3,
    pub on_ground: bool,
    /// Highest feet Y reached since the mob last stood/swum. A landing compares this
    /// peak to the landed feet Y to produce deterministic fall damage.
    pub fall_peak_y: f64,
    /// Landing distance latched by `finish_motion` and drained by the
    /// manager after the tick so `ServerGame` can route damage through `mob_damage_pre`.
    pub fall_distance: f32,
    /// Fall-into-fluid distance latched by `finish_motion` and drained
    /// by the manager — `ServerGame` turns it into the fluid-splash burst.
    pub splash_drop: f32,
    /// The brain's locomotion and gait rate multiplier this tick.
    pub walk_speed_scale: f32,
    /// How fast the gait clip runs against the species' walking pace: below
    /// 1 while a slow driven step carries the body.
    pub gait_pace: f32,
    /// This tick's motion was a driven step under the body's own power.
    pub stepping: bool,
    /// Horizontal knockback velocity (m/s), decaying over the stagger. Kept separate
    /// from `vel` so the per-tick wish-velocity overwrite can't wipe it.
    pub knockback: Vec3,
    /// Soft entity-push velocity (m/s, horizontal) accumulated from overlapping other
    /// entities last tick — added on top of locomotion and consumed by the
    /// integration (the push pass re-derives it each tick from the live
    /// overlap). Kept separate from `vel` for the same reason as `knockback`.
    pub push: Vec3,
    /// An upward launch from a WALKING gait happened THIS tick (a mod's
    /// vertical drive, a navigation step jump) — consumed by the expression,
    /// which re-phases the walk clip forward to the next cycle boundary so an
    /// authored takeoff clip stays in phase with the physical arc.
    pub walk_launch: bool,
    /// The current airborne phase counts as a WALK (it began from walking
    /// locomotion and horizontal motion still carries) — keeps the gait
    /// expression through the whole ballistic arc.
    pub air_walk: bool,
    /// A mod's kinematic locomotion intent for THIS tick (the `MobDrive`
    /// HostCall): while present it replaces the brain's wish-velocity
    /// overwrite, so a mod can drive a mob directly (a vehicle) with the
    /// engine still owning vertical physics and collision. Like the brain's
    /// wish it must be re-set every tick.
    pub drive: Option<DriveIntent>,
    /// This tick's mod-authored pose (`set_kinematic`): while present the
    /// whole locomotion step — brain, navigation, integration, collision —
    /// is replaced by writing the pose as given. An intent consumed by the
    /// tick, never a state.
    pub kinematic: Option<KinematicPose>,
    /// The committed way out of geometry this body is stuck inside (see
    /// `collision::EscapeRoute`) — derived from the world every tick.
    pub escape: petramond_world::collision::EscapeRoute,
}

impl Motion {
    /// A body at rest with its feet at height `feet_y`.
    pub fn at_rest(feet_y: f64) -> Self {
        Motion {
            vel: Vec3::ZERO,
            on_ground: false,
            fall_peak_y: feet_y,
            fall_distance: 0.0,
            splash_drop: 0.0,
            walk_speed_scale: 1.0,
            gait_pace: 1.0,
            stepping: false,
            knockback: Vec3::ZERO,
            push: Vec3::ZERO,
            walk_launch: false,
            air_walk: false,
            drive: None,
            kinematic: None,
            escape: Default::default(),
        }
    }
}

/// Vitality feedback: the engine's damage immunity, the hurt flash and
/// knockback stagger, the death state, the strike the brain latched this
/// tick, and who last hurt the mob.
pub(in crate::mob) struct Combat {
    /// Engine-owned global damage immunity. Transient like hurt/stagger
    /// presentation; starts clear when a saved mob is restored.
    pub damage_immunity: petramond_world::damage::DamageImmunity,
    /// Seconds of hurt flash remaining. Drives the replicated red tint only.
    pub hurt_timer: f32,
    /// Seconds of knockback stagger remaining. Kept separate from the flash
    /// timer so feedback can compose knockback without forcing a red flash,
    /// or vice versa.
    pub stagger_timer: f32,
    /// Once the mob has died it runs no AI and takes no further damage. The default
    /// death presentation is a ragdoll, but a custom feedback bundle may omit it.
    pub death: DeathState,
    /// A melee strike the brain wants landed THIS tick — latched by the
    /// tick, drained by the manager into a [`MobAttack`](super::super::MobAttack).
    /// Cleared every tick.
    pub attack: Option<AttackIntent>,
    /// Who last damaged this mob + ticks since — the retaliation input,
    /// recorded by `damage`. Ages out on the node's own memory policy.
    pub attacker: Option<EntityRef>,
    pub attacker_ticks: u32,
}

/// A living body with no feedback running.
impl Default for Combat {
    fn default() -> Self {
        Combat {
            damage_immunity: Default::default(),
            hurt_timer: 0.0,
            stagger_timer: 0.0,
            death: DeathState::Alive,
            attack: None,
            attacker: None,
            attacker_ticks: 0,
        }
    }
}

impl Combat {
    /// Count the hurt flash and the knockback stagger down on the fixed tick
    /// (frame-rate independent). A corpse's flash fades out this way too.
    pub fn count_down_feedback(&mut self, dt: f32) {
        self.hurt_timer = (self.hurt_timer - dt).max(0.0);
        self.stagger_timer = (self.stagger_timer - dt).max(0.0);
    }

    /// Age the attacker memory by one tick; the retaliation node applies its
    /// own forget policy against this counter.
    pub fn age_attacker(&mut self) {
        if self.attacker.is_some() {
            self.attacker_ticks = self.attacker_ticks.saturating_add(1);
        }
    }
}

/// The deciding half of a mob: its brain, the navigator that turns goals into
/// steering, and the decision state they carry between ticks. Transient AI
/// state — a reloaded mob re-perceives.
pub(in crate::mob) struct Mind {
    pub brain: Brain,
    pub nav: Navigator,
    /// Crowd-veer side commitment (see `nav::Unstick`).
    pub unstick: Unstick,
    /// The target the brain settled on last tick (the merged
    /// `BehaviorOutput::target`), fed back as `AiCtx::target` so attack
    /// nodes strike what the winning perception node locked.
    pub current_target: Option<EntityRef>,
    /// The continuous part of the last decision, replayed on the ticks a
    /// reduced-rate mob coasts without thinking (see `manager::lod`).
    pub held_decision: HeldDecision,
    /// The entities whose bodies overlapped this mob, recorded by the
    /// manager's push pass each tick and read by the NEXT tick's AI as
    /// `AiCtx::contacts` (the touch perception channel).
    pub contacts: Vec<EntityRef>,
}

/// The continuous channels of a settled brain decision — what a mob keeps
/// doing between decisions when its brain runs at a reduced rate. One-shot
/// channels (a strike, an animation start, tag writes) and the goal (the
/// navigator already holds it) are deliberately absent.
#[derive(Copy, Clone, Debug, Default)]
pub(in crate::mob) struct HeldDecision {
    head_look: Option<HeadLook>,
    facing: Option<f32>,
    speed_scale: Option<f32>,
    idle_anim: Option<u8>,
}

impl HeldDecision {
    pub fn of(decision: &BehaviorOutput) -> Self {
        HeldDecision {
            head_look: decision.head_look,
            facing: decision.facing,
            speed_scale: decision.speed_scale,
            idle_anim: decision.idle_anim,
        }
    }

    /// The decision a coasting tick acts on: these channels, still engaged
    /// on `target`.
    pub fn replay(self, target: Option<EntityRef>) -> BehaviorOutput {
        BehaviorOutput {
            head_look: self.head_look,
            facing: self.facing,
            speed_scale: self.speed_scale,
            idle_anim: self.idle_anim,
            target,
            ..BehaviorOutput::default()
        }
    }
}

/// The periodic confined-state verdict (the `petramond:confined` tag) and the
/// cadence state that keeps it cheap. Never persisted: rebuilt from the world.
pub(in crate::mob) struct Confinement {
    /// Ticks until the next recompute. Spread across mobs by id so checks
    /// don't clump on one tick.
    cooldown: u8,
    /// The confined region this mob is captive in — a shared handle into the
    /// manager's [`RegionCache`], `Some` exactly while the verdict is
    /// confined. Wander picks destinations straight from it. A block change
    /// drops the cache entry, which forces a re-check off-cadence.
    region: Option<Arc<ConfinedRegion>>,
    /// Where this mob last PROVED itself free, and the world's nav revision at
    /// that moment — the two inputs of the free-verdict staleness gate (see
    /// [`confined::free_verdict_stale`]). `u64::MAX` until the first check, so
    /// a newborn always checks.
    checked_at: IVec3,
    checked_rev: u64,
    /// Ticks a free verdict has been carried unproven, so it is re-proven at
    /// [`confined::FREE_CHECK_INTERVAL`] regardless.
    free_age: u16,
}

/// What one confinement refresh reads about the mob and its world.
pub(in crate::mob) struct ConfinementProbe {
    /// The mob's navigation cell.
    pub cell: IVec3,
    /// Whether the brain runs this tick (a coasting mob keeps its verdict).
    pub think: bool,
    /// Only grounded, dry mobs are judged: a swimming or falling mob's space
    /// is transient.
    pub judgeable: bool,
    /// The world's navigation revision.
    pub nav_rev: u64,
}

impl Confinement {
    pub fn new(seed: u64) -> Self {
        Confinement {
            cooldown: ((seed % confined::CHECK_INTERVAL as u64) as u8).max(1),
            region: None,
            checked_at: IVec3::ZERO,
            checked_rev: u64::MAX,
            free_age: 0,
        }
    }

    /// The region the mob is captive in, when confined.
    pub fn region(&self) -> Option<&ConfinedRegion> {
        self.region.as_deref()
    }

    /// The periodic refresh: `Some(confined)` when a verdict was (re)proven
    /// this tick, `None` when the cadence kept the last one. A mob already
    /// known FREE re-proves only when something that could have changed the
    /// answer happened (see [`confined::free_verdict_stale`]); a CONFINED
    /// mob always re-evaluates, because its answer comes from the shared
    /// region cache and costs a membership test, not a fill. `flood` runs
    /// the fill on a cache miss — the only step that reads the world.
    pub fn refresh(
        &mut self,
        probe: ConfinementProbe,
        regions: &mut RegionCache,
        flood: impl FnOnce() -> Option<ConfinedRegion>,
    ) -> Option<bool> {
        self.cooldown = self.cooldown.saturating_sub(1);
        let region_dropped = self.region.as_ref().is_some_and(|r| !regions.is_live(r));
        if region_dropped {
            self.region = None;
        }
        let verdict_stale = region_dropped
            || self.region.is_some()
            || confined::free_verdict_stale(
                probe.cell,
                self.checked_at,
                probe.nav_rev,
                self.checked_rev,
                self.free_age,
            );
        let due = probe.think && (self.cooldown == 0 || region_dropped);
        if due && !verdict_stale {
            self.cooldown = confined::CHECK_INTERVAL;
            self.free_age = self
                .free_age
                .saturating_add(confined::CHECK_INTERVAL as u16);
        }
        if !(due && verdict_stale && probe.judgeable) {
            return None;
        }
        self.checked_at = probe.cell;
        self.checked_rev = probe.nav_rev;
        self.free_age = 0;
        // Steady state for a penned mob: the shared cache still holds a
        // region covering its cell (a pen-mate may have filled this pen
        // already) — the lookup enforces region age, so a stale handle can
        // never short-circuit it. Only a miss floods.
        self.region = regions
            .region_at(probe.cell)
            .or_else(|| flood().map(|r| regions.insert(r)));
        self.cooldown = confined::CHECK_INTERVAL;
        Some(self.region.is_some())
    }
}

/// Mod-controlled presentation and the engine expression clock's private
/// state. Presentation-only: replicated per tick, never persisted (whoever
/// set it re-derives it).
pub(in crate::mob) struct Presentation {
    /// ACTIVE particle-emitter bundles by catalog id
    /// (`petramond_world::particle_emitters`), sorted, at most
    /// [`MAX_ACTIVE_MOB_EMITTERS`](super::super::MAX_ACTIVE_MOB_EMITTERS),
    /// toggled through the `MobEmitterSet` HostCall. Survives death so a
    /// corpse keeps its effect through the ragdoll.
    pub active_emitters: Vec<u8>,
    /// ACTIVE named model animations, sorted by name, at most
    /// [`MAX_ACTIVE_MOB_ANIMS`](super::super::MAX_ACTIVE_MOB_ANIMS) —
    /// controlled through the `MobAnimSet`/`MobAnimRate` HostCalls,
    /// replicated per tick (name + phase). Each layer is SELF-CLOCKED: its
    /// phase advances by `rate` per second on the tick — rate 0 freezes it
    /// mid-stroke, negative reverses. The renderer layers every active one
    /// over the walk/idle/rest base pose and skips names the model lacks.
    pub active_anims: Vec<AnimLayer>,
    /// The animation kind playing last tick, to detect changes (and reset
    /// `anim_time`).
    pub anim_kind: AnimKind,
    /// How fast the head is swinging (yaw, pitch; rad/s): its easing state.
    pub head_vel: [f32; 2],
    /// Items drawn in the main and off hands.
    pub held: [Option<petramond_world::item::ItemType>; 2],
    /// The retained draw set this body wears.
    pub draw: crate::world::draw::BodyDraw,
}

/// Nothing attached, at the rest pose.
impl Default for Presentation {
    fn default() -> Self {
        Presentation {
            active_emitters: Vec::new(),
            active_anims: Vec::new(),
            anim_kind: AnimKind::Rest,
            head_vel: [0.0; 2],
            held: [None; 2],
            draw: Default::default(),
        }
    }
}
