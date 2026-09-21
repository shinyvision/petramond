//! Instance body kinematics: the per-tick locomotion integration (knockback
//! stagger > mod drive > brain wish precedence, fluid current, buoyancy,
//! gravity, shared swept-AABB collision), the solid-peer motion commit, the
//! soft entity push, and fall/splash bookkeeping.

use std::f32::consts::{PI, TAU};

use petramond_math::math::{Tilt, Vec3};

use super::instance::Instance;
use super::MobDef;
use crate::entity::shore::{ShoreClimb, Swimmer};
use petramond_world::block::Aabb;
use petramond_world::collision::{self, DynBox};
use petramond_world::fluid::{Buoyancy, FluidCurrent, Immersion};

/// Downward acceleration (m/s²) applied to airborne mobs.
const GRAVITY: f32 = -22.0;
/// Per-tick decay of the horizontal knockback velocity during the stagger.
const KNOCKBACK_DAMP: f32 = 0.75;
/// A driven step slower than this is a body settling, not walking.
const GAIT_MIN_SPEED: f32 = 0.15;
/// The slowest a gait clip runs under a driven step.
const GAIT_MIN_PACE: f32 = 0.35;

/// One tick's mod-issued locomotion — full 3-D velocity access, each part
/// independently optional (see [`Instance::set_drive`] and the MobDrive ABI
/// doc): `horizontal` REPLACES the brain's wish locomotion (a vehicle);
/// `vertical` sets this tick's vertical velocity (gravity resumes next tick)
/// and composes with either horizontal source — an upward value from the
/// ground is a launch; `yaw` sets the absolute facing.
#[derive(Copy, Clone)]
pub(super) struct DriveIntent {
    pub horizontal: Option<[f32; 2]>,
    pub vertical: Option<f32>,
    pub yaw: Option<f32>,
    /// The intent's premise (see the MobDrive ABI doc): when set, consume
    /// only on a tick whose brain locomotion is walking the mob (`moving`);
    /// drop silently otherwise. A latched intent is decided from LAST
    /// tick's state — the walk it was premised on can end in between.
    pub while_walking: bool,
    /// The horizontal drive is the body walking itself there, not something
    /// carrying it: it reads as `moving`, its gait paced to the driven speed.
    pub gait: bool,
}

/// This tick's walking request from the brain and its route.
#[derive(Copy, Clone, Debug)]
pub(super) struct Locomotion {
    /// Unit horizontal direction to walk.
    pub wish: Vec3,
    /// A route step-up asks for a jump.
    pub jump: bool,
    /// Whether locomotion may steer at all (see [`route_steering_supported`]).
    pub can_steer: bool,
}

/// What a mob body's integration reads about its surroundings.
pub(super) struct Surroundings<'a> {
    pub boxes: &'a dyn Fn(i32, i32, i32) -> &'static [Aabb],
    /// Solid entity boxes the body collides with.
    pub obstacles: &'a [DynBox],
    /// Solid entity boxes the shallow-foot heal may lift the body onto.
    pub healing_obstacles: &'a [DynBox],
    pub immersion: Option<Immersion>,
    pub current: FluidCurrent,
}

#[cfg(test)]
impl<'a> Surroundings<'a> {
    /// Dry, still surroundings over `boxes` with no entities.
    pub(super) fn dry(boxes: &'a dyn Fn(i32, i32, i32) -> &'static [Aabb]) -> Self {
        Surroundings {
            boxes,
            obstacles: &[],
            healing_obstacles: &[],
            immersion: None,
            current: FluidCurrent::NONE,
        }
    }
}

/// One tick's mod-authored pose (see [`Instance::set_kinematic`] and the
/// `MobKinematic` ABI doc): the body is written here, and none of the engine's
/// own motion runs for it that tick.
#[derive(Copy, Clone, Debug, PartialEq)]
pub(super) struct KinematicPose {
    pub pos: petramond_math::world_pos::WorldPos,
    pub yaw: f32,
    pub tilt: Tilt,
}

/// How fast a body the engine moves itself returns to level (rad/s): a cart
/// that left its rails mid-slope settles flat over a few frames instead of
/// lying tilted wherever it lands.
const BODY_LEVEL_RATE: f32 = 4.0;

impl Instance {
    /// Latch a mod's pose for this tick (see [`KinematicPose`] and the
    /// consumption in `Instance::tick`). Refused on a dead mob.
    pub(super) fn set_kinematic(&mut self, pose: KinematicPose) -> bool {
        if self.death.is_dead() {
            return false;
        }
        self.kinematic = Some(pose);
        true
    }

    /// Write a mod-authored pose in place of this tick's locomotion step.
    ///
    /// The velocity the placement implies is kept, so a body the mod stops
    /// placing continues ballistically from its last pose — and it is left
    /// AIRBORNE so the engine's next integration carries that velocity into
    /// a flight instead of zeroing it as a standing body's. Fall bookkeeping
    /// re-anchors on the placement: a later drop only ever measures from
    /// here. Knockback is discarded — a shove on a constrained body is the
    /// constraining mod's rule to apply (it sees the hit's origin).
    pub(super) fn place_kinematic(&mut self, dt: f32, pose: KinematicPose) {
        self.vel = (pose.pos - self.pos) / dt.max(1e-6);
        self.pos = pose.pos;
        self.yaw = pose.yaw;
        self.tilt = pose.tilt;
        self.on_ground = false;
        self.fall_peak_y = pose.pos.y;
        self.moving = false;
        self.air_walk = false;
        self.walk_launch = false;
        self.knockback = Vec3::ZERO;
        self.stagger_timer = 0.0;
        self.push = Vec3::ZERO;
    }

    /// A body the engine moves itself is level; one released from a
    /// kinematic placement eases back there.
    pub(super) fn level_body(&mut self, dt: f32) {
        self.tilt = self.tilt.toward_level(BODY_LEVEL_RATE * dt);
    }

    pub(super) fn take_fall_distance(&mut self) -> Option<f32> {
        let distance = std::mem::replace(&mut self.fall_distance, 0.0);
        (distance > 0.0).then_some(distance)
    }

    pub(super) fn take_splash_drop(&mut self) -> Option<f32> {
        let drop = std::mem::replace(&mut self.splash_drop, 0.0);
        (drop > 0.0).then_some(drop)
    }

    /// Latch a mod's locomotion intent for this tick (see [`DriveIntent`] and
    /// the consumption in [`integrate_locomotion`](Self::integrate_locomotion)).
    /// Refused on a dead mob.
    pub(super) fn set_drive(&mut self, intent: DriveIntent) -> bool {
        if self.death.is_dead() {
            return false;
        }
        self.drive = Some(intent);
        true
    }

    /// Discard this tick's unconsumed mod intents (drive and kinematic pose).
    /// Frozen/skipped mobs call this explicitly because they never reach the
    /// locomotion step that normally consumes them.
    #[inline]
    pub(super) fn clear_drive(&mut self) {
        self.drive = None;
        self.kinematic = None;
    }

    #[cfg(test)]
    pub(super) fn drive_pending(&self) -> bool {
        self.drive.is_some()
    }

    /// Current velocity (m/s) — read-only; mods steer through
    /// `set_drive`, never by writing velocity directly.
    #[inline]
    pub fn vel(&self) -> Vec3 {
        self.vel
    }

    /// Commit the collision-free prefix selected for a solid body's proposed
    /// transform. The manager has already constrained the prefix against both
    /// terrain and peer solids.
    pub(super) fn commit_solid_motion(&mut self, motion: super::BodyMotion, fraction: f32) {
        if fraction >= 1.0 - 1e-6 {
            return;
        }
        debug_assert_eq!(self.id, motion.id);
        let proposed_delta = motion.end_pos - motion.start_pos;
        (self.pos, self.yaw) = motion.pose_at(fraction);
        if proposed_delta.x.abs() > 1e-6 {
            self.vel.x = 0.0;
        }
        if proposed_delta.z.abs() > 1e-6 {
            self.vel.z = 0.0;
        }
        if proposed_delta.y.abs() > 1e-6 {
            self.vel.y = 0.0;
            self.on_ground = false;
        }
    }

    /// Promote a downward peer contact to ground after every solid has
    /// committed. The final-pose support query lives in the manager, where all
    /// peer transforms are available simultaneously.
    pub(super) fn land_on_solid_peer(&mut self) {
        self.vel.y = 0.0;
        self.on_ground = true;
    }

    /// Set this tick's soft entity-push velocity (the sum of the pushes from every
    /// entity it overlaps). It is applied — and consumed — on the next
    /// [`integrate`](Self::integrate), on top of locomotion, moving through the normal
    /// collision-resolved step so it can't push the mob through terrain.
    pub(super) fn set_push(&mut self, push: Vec3) {
        self.push = push;
    }

    /// Update fall bookkeeping after a tick's movement has resolved `on_ground` and
    /// feet position. Immersion breaks falls by re-anchoring the peak while submerged.
    pub(super) fn finish_motion(&mut self, was_on_ground: bool, immersion: Option<Immersion>) {
        if let Some(sample) = immersion {
            // The un-latched drop at the first wet tick is the fall INTO the
            // fluid; while swimming the per-tick re-anchor keeps it near zero
            // (the splash threshold filters the bobbing).
            let drop = (self.fall_peak_y - self.pos.y) as f32;
            if sample.fluid.splash.is_some() && drop > 0.0 {
                self.splash_drop = self.splash_drop.max(drop);
            }
            self.fall_peak_y = self.pos.y;
        } else if self.on_ground {
            if !was_on_ground {
                let dist = (self.fall_peak_y - self.pos.y) as f32;
                if dist > self.fall_distance {
                    self.fall_distance = dist;
                }
            }
            self.fall_peak_y = self.pos.y;
        } else {
            self.fall_peak_y = self.fall_peak_y.max(self.pos.y);
        }
    }

    /// Integrate this tick's locomotion against shared fluid forces and
    /// collision. While unsupported and falling, path steering is suspended
    /// and existing horizontal velocity carries through the fall; the upward
    /// phase of a navigation jump keeps steering so the mob can clear a
    /// one-block ledge. The mob faces its **wish** direction — where it wants
    /// to go — so it keeps facing forward even when pressed against a wall
    /// (where its actual velocity would be zero). Returns the mandatory
    /// shallow-foot healing lift separately for the peer-motion proposal.
    pub(super) fn integrate_locomotion(
        &mut self,
        dt: f32,
        d: &MobDef,
        loco: Locomotion,
        env: &Surroundings<'_>,
    ) -> f32 {
        let was_grounded = self.on_ground;
        let incoming = self.vel;
        let nav_jumped = loco.jump && self.on_ground && env.immersion.is_none();
        if nav_jumped {
            self.vel.y = d.jump_speed;
            self.on_ground = false;
        }
        // The drive is consumed even when stagger owns the tick — like the
        // wish, it is a this-tick intent, never a queue.
        let drive = self.drive.take();
        let requested_yaw = self.steer_horizontal(dt, d, loco, drive, nav_jumped);
        // The intent's premise: a `while_walking` drive was decided from
        // LAST tick's state on the promise the mob is walking — if the walk
        // ended in between (arrival, an abandoned route, a backoff pause),
        // the stale intent is dropped whole, or it fires one in-place bounce
        // exactly where the mob came to rest (the "one hop too often"
        // playtest report).
        let premise_holds = drive.is_none_or(|dr| !dr.while_walking || self.moving);
        let drive_steers = premise_holds && loco.can_steer && self.stagger_timer <= 0.0;
        if !nav_jumped && drive_steers {
            self.drive_vertical(drive);
        }
        // An upward launch that starts from a walking gait re-phases the walk
        // clip forward onto a cycle boundary (see `apply_expression`), so an
        // authored takeoff clip stays locked to the physical arc.
        if self.moving && !self.on_ground && self.vel.y > 0.0 && was_grounded {
            self.walk_launch = true;
        }
        // A drive's absolute yaw obeys the same gates as its velocities: no
        // steering while unsupported, the knockback stagger owns its tick,
        // and a walking-gated intent's premise must hold.
        let drive_yaw = drive.filter(|_| drive_steers).and_then(|dr| dr.yaw);
        if let Some(yaw) = drive_yaw.or(requested_yaw) {
            self.yaw = super::clamp_body_yaw(
                self.pos,
                self.yaw,
                yaw,
                d.size,
                &env.boxes,
                env.obstacles,
                self.id,
            );
        }
        if d.edge_guard && self.on_ground && !nav_jumped && env.immersion.is_none() {
            self.guard_edge(dt, d, env);
        }
        let carried = (self.stagger_timer <= 0.0 && !loco.can_steer && env.immersion.is_none())
            .then_some([self.vel.x, self.vel.z]);
        // The shore climb follows where locomotion (a walk or a drive) heads.
        let heading = Vec3::new(self.vel.x, 0.0, self.vel.z);
        self.resist_and_push(dt, incoming, env);
        let shore = self.vertical_velocity(dt, d, loco.can_steer, heading, env);
        self.resolve_motion(dt, d, shore, carried, env)
    }

    /// The edge guard of a cautious species (`"edge_guard"` row): a grounded
    /// body never walks off more than its routes plan to drop. Anything within
    /// that still steps down; a ledge taller pulls the offending axis back to
    /// its lip, as a sneaking player's does, so a body steering along a wall
    /// top or a roof edge slides along it instead of over.
    ///
    /// Routes count a drop in cells, from the cell the feet stand in to the
    /// one they land in, and a floor sits anywhere within its cell (a slab's
    /// top, a chest's): the lowest floor a planned drop lands on lies just
    /// above the top of the cell `max_drop + 1` below the feet's.
    fn guard_edge(&mut self, dt: f32, d: &MobDef, env: &Surroundings<'_>) {
        let feet_cell = (self.pos.y - 0.01).floor() + 1.0;
        let lowest = feet_cell - f64::from(d.path_params().max_drop) - 1.0;
        let drop = (self.pos.y - lowest) as f32 - collision::SUPPORT_PROBE_MARGIN;
        let h = f64::from(d.size.half_width);
        let min = [self.pos.x - h, self.pos.y, self.pos.z - h];
        let max = [
            self.pos.x + h,
            self.pos.y + f64::from(d.size.height),
            self.pos.z + h,
        ];
        let (dx, dz) = (self.vel.x * dt, self.vel.z * dt);
        let (cx, cz) = collision::clamp_to_supported_dyn(
            min,
            max,
            dx,
            dz,
            drop,
            env.boxes,
            env.obstacles,
            self.id,
        );
        if cx != dx {
            self.vel.x = cx / dt;
        }
        if cz != dz {
            self.vel.z = cz / dt;
        }
    }

    /// Choose this tick's horizontal velocity source: the knockback stagger,
    /// a mod's horizontal drive (a vehicle), or the wish. Keeping knockback
    /// separate from `vel` is why these overwrites can't wipe it. Returns the
    /// facing a walk turned toward.
    fn steer_horizontal(
        &mut self,
        dt: f32,
        d: &MobDef,
        loco: Locomotion,
        drive: Option<DriveIntent>,
        nav_jumped: bool,
    ) -> Option<f32> {
        self.stepping = false;
        if self.stagger_timer > 0.0 {
            self.vel.x = self.knockback.x;
            self.vel.z = self.knockback.z;
            self.knockback *= KNOCKBACK_DAMP;
            self.moving = false;
        } else if let Some([vx, vz]) = drive.and_then(|dr| dr.horizontal) {
            // A horizontally-driven mob is deliberately not `moving`: the
            // drive is not a walk — no walk animation, no footstep noise, no
            // wish-facing — unless the intent says the body walks itself
            // (`gait`): a step sideways is still a step. Gated on `can_steer` like the wish so a driven
            // body has no more air or stagger control than a walking one.
            // Long-body yaw is clamped by the same segmented geometry that
            // resolves its translation.
            if loco.can_steer {
                self.vel.x = vx;
                self.vel.z = vz;
            }
            let speed = vx.hypot(vz);
            self.moving =
                drive.is_some_and(|dr| dr.gait) && loco.can_steer && speed > GAIT_MIN_SPEED;
            self.stepping = self.moving;
            if self.moving {
                self.gait_pace = (speed / d.walk_speed.max(1e-3)).clamp(GAIT_MIN_PACE, 1.0);
            }
        } else if loco.can_steer {
            let wish = loco.wish;
            self.moving = wish.length_squared() > 1e-6;
            self.gait_pace = 1.0;
            let mut speed = d.walk_speed * self.walk_speed_scale;
            let mut requested_yaw = None;
            if self.moving {
                let target = heading_yaw(wish);
                let turned = turn_toward(self.yaw, target, d.turn_rate * dt);
                requested_yaw = Some(turned);
                // Walk only as fast as the body faces the wish: a reversal
                // pivots on the spot and then goes, instead of sliding
                // backward at full speed for the half-second the turn takes
                // (with the wish snapping around a lure or a peer, that slide
                // is the "shaking" a viewer sees). A step-up approach keeps
                // full speed — the jump needs its run-up.
                if !nav_jumped {
                    speed *= facing_speed_factor(wrap_angle(target - turned));
                }
            }
            self.vel.x = wish.x * speed;
            self.vel.z = wish.z * speed;
            return requested_yaw;
        } else {
            // Unsteered mid-air (a fall's descent) a mob whose airborne arc
            // BEGAN as a walk still reads as walking while its horizontal
            // motion carries — the gait clip plays through the whole
            // ballistic arc instead of snapping to the rest pose at the
            // apex (a mod-authored hop, a navigation jump, a walked-off
            // ledge alike).
            self.moving = self.air_walk
                && !self.on_ground
                && self.vel.x * self.vel.x + self.vel.z * self.vel.z > 1e-6;
        }
        None
    }

    /// A mod's vertical drive: set this tick's vertical velocity (gravity
    /// resumes below), composing with EITHER horizontal source. An upward
    /// value from the ground is a launch. The navigator's step jump keeps
    /// priority on a tick both fire — its full `jump_speed` is sized to clear
    /// the one-block ledge the route depends on.
    fn drive_vertical(&mut self, drive: Option<DriveIntent>) {
        if let Some(vy) = drive.and_then(|dr| dr.vertical) {
            self.vel.y = vy;
            if vy > 0.0 && self.on_ground {
                self.on_ground = false;
            }
        }
    }

    /// The medium's horizontal resistance, the soft entity push, and the
    /// fluid current.
    fn resist_and_push(&mut self, dt: f32, incoming: Vec3, env: &Surroundings<'_>) {
        if let Some(sample) = env.immersion {
            let desired = self.vel;
            let incoming = Vec3::new(incoming.x, self.vel.y, incoming.z);
            self.vel = sample
                .fluid
                .motion
                .horizontal_velocity(incoming, desired, dt);
        }
        // Soft entity push: a velocity from being jostled by overlapping entities,
        // layered on top of locomotion (or knockback) so a crowded mob drifts apart
        // smoothly. Consumed each tick — the push pass re-derives it from the live
        // overlap — and left out of `moving`, so being shoved doesn't read as walking.
        self.vel.x += self.push.x;
        self.vel.z += self.push.z;
        self.push = Vec3::ZERO;
        self.vel = env.current.apply(self.vel, dt);
    }

    /// Buoyancy or gravity, and the shore climb that overrides buoyancy.
    /// Creatures always try to leave toward a reachable shore; floating and
    /// neutral bodies have no stroke to climb with.
    fn vertical_velocity(
        &mut self,
        dt: f32,
        d: &MobDef,
        can_steer: bool,
        heading: Vec3,
        env: &Surroundings<'_>,
    ) -> Option<ShoreClimb> {
        let Some(sample) = env.immersion else {
            self.vel.y += GRAVITY * d.gravity_scale * dt;
            return None;
        };
        let shore = (d.buoyancy == Buoyancy::Swim && can_steer && self.stagger_timer <= 0.0)
            .then(|| {
                Swimmer {
                    pos: self.pos,
                    vel_y: self.vel.y,
                    half_width: d.size.half_width,
                    height: d.size.height,
                    gravity: -GRAVITY * d.gravity_scale,
                    jump_speed: d.jump_speed,
                }
                .shore_climb(heading, sample, &env.boxes, env.obstacles)
            })
            .flatten();
        if let Some(ShoreClimb::Launch(speed)) = shore {
            self.vel.y = self.vel.y.max(speed);
        } else {
            self.vel.y =
                sample.vertical_velocity(self.vel.y, self.pos.y as f32, d.buoyancy, true, dt);
        }
        shore
    }

    /// Body collision via the shared swept-AABB resolver (the same one the
    /// player and dropped items use) against the block's REAL collision shape
    /// — so a mob stops at a bbmodel block's legs/top, not its full cube.
    /// Navigation keeps its cell-indexed skeleton but validates candidate edges
    /// against the same real shapes (the `mob::nav` gate), so what the planner
    /// accepts is what this resolver permits. A grounded mob auto-steps up a
    /// half-block ledge without jumping — same `STEP_HEIGHT` the player uses.
    /// `carried` is the airborne horizontal velocity a collision may not
    /// replace on an unblocked axis.
    fn resolve_motion(
        &mut self,
        dt: f32,
        d: &MobDef,
        shore: Option<ShoreClimb>,
        carried: Option<[f32; 2]>,
        env: &Surroundings<'_>,
    ) -> f32 {
        let step = match shore {
            Some(ShoreClimb::Step(height)) => height.max(collision::STEP_HEIGHT),
            _ => collision::STEP_HEIGHT,
        };
        let (moved, grounded, hit, healed) = super::resolve_body_motion(
            self.pos,
            self.yaw,
            d.size,
            self.vel.to_array(),
            dt,
            step,
            matches!(shore, Some(ShoreClimb::Step(_))),
            &env.boxes,
            env.obstacles,
            env.healing_obstacles,
            self.id,
        );
        self.pos += Vec3::from(moved);
        if hit[0] {
            self.vel.x = 0.0;
        }
        if hit[1] {
            self.vel.y = 0.0;
        }
        if hit[2] {
            self.vel.z = 0.0;
        }
        if let Some([x, z]) = carried {
            if !hit[0] {
                self.vel.x = x;
            }
            if !hit[2] {
                self.vel.z = z;
            }
        }
        self.on_ground = grounded;
        if grounded && self.vel.y < 0.0 {
            self.vel.y = 0.0;
        }
        // The air-walk latch: an airborne phase counts as a WALK while it
        // began from (or continues) walking locomotion and horizontal motion
        // carries — read by the unsteered branch of `steer_horizontal` so the
        // gait expression survives the whole ballistic arc. Landing clears it.
        self.air_walk = !grounded
            && (self.moving
                || (self.air_walk && self.vel.x * self.vel.x + self.vel.z * self.vel.z > 1e-6));
        healed
    }

    /// [`integrate_locomotion`](Self::integrate_locomotion) on dry cell-solid
    /// terrain, for the land kinematics tests.
    #[cfg(test)]
    pub(super) fn integrate(
        &mut self,
        dt: f32,
        d: &MobDef,
        wish: Vec3,
        jump: bool,
        solid: &impl Fn(petramond_math::math::IVec3) -> bool,
    ) {
        let loco = Locomotion {
            wish,
            jump,
            can_steer: true,
        };
        self.integrate_locomotion(dt, d, loco, &Surroundings::dry(&boxes_of(solid)));
    }

    /// Whether the body rests on the ground this tick — the same fact the
    /// engine's own locomotion gates jumps on, exposed to the ABI snapshot
    /// so a mod gait policy can decide a vertical-drive launch.
    pub fn on_ground(&self) -> bool {
        self.on_ground
    }
}

/// The yaw that faces the horizontal component of `v`. The model faces `-Z` at
/// `yaw = 0` (the renderer applies `rotation_y(yaw)`), so heading `(vx, vz)` maps to
/// `atan2(-vx, -vz)`.
fn heading_yaw(v: Vec3) -> f32 {
    (-v.x).atan2(-v.z)
}

/// How much of the walk speed a body still misaligned with its wish by
/// `delta` radians gets: full when facing it, none at 90° and beyond.
fn facing_speed_factor(delta: f32) -> f32 {
    delta.cos().max(0.0)
}

/// Turn `yaw` toward `target` by at most `max_step`, along the shortest arc.
pub(super) fn turn_toward(yaw: f32, target: f32, max_step: f32) -> f32 {
    let delta = wrap_angle(target - yaw);
    let step = max_step.min(delta.abs());
    wrap_angle(yaw + step * delta.signum())
}

/// Wrap an angle into `[-PI, PI]`.
fn wrap_angle(a: f32) -> f32 {
    (a + PI).rem_euclid(TAU) - PI
}

pub(super) fn route_steering_supported(
    on_ground: bool,
    in_fluid: bool,
    vertical_velocity: f32,
) -> bool {
    on_ground || in_fluid || vertical_velocity > 0.0
}

/// Bridge a cell-solid bool stub into the shared collision box source (a full cube per
/// solid cell), so the kinematics tests keep driving body physics with a simple `solid`
/// predicate while it routes through the same `collision::resolve_body` as production.
#[cfg(test)]
fn boxes_of(
    solid: &impl Fn(petramond_math::math::IVec3) -> bool,
) -> impl Fn(i32, i32, i32) -> &'static [petramond_world::block::Aabb] + '_ {
    move |x, y, z| {
        if solid(petramond_math::math::IVec3::new(x, y, z)) {
            petramond_world::block::Block::Stone.collision_boxes()
        } else {
            &[]
        }
    }
}

#[cfg(test)]
mod tests;
