use std::f32::consts::{PI, TAU};

use petramond_math::math::{Tilt, Vec3};

use super::instance::Instance;
use super::MobDef;
use crate::entity::shore::{ShoreClimb, Swimmer};
use petramond_world::block::Aabb;
use petramond_world::collision::{self, DynBox};
use petramond_world::fluid::{Buoyancy, FluidCurrent, Immersion};

const GRAVITY: f32 = -22.0;
const KNOCKBACK_DAMP: f32 = 0.75;
const GAIT_MIN_SPEED: f32 = 0.15;
const GAIT_MIN_PACE: f32 = 0.35;

#[derive(Copy, Clone)]
pub(super) struct DriveIntent {
    pub horizontal: Option<[f32; 2]>,
    pub vertical: Option<f32>,
    pub yaw: Option<f32>,
    pub while_walking: bool,
    pub gait: bool,
}

#[derive(Copy, Clone, Debug)]
pub(super) struct Locomotion {
    pub wish: Vec3,
    pub jump: bool,
    pub can_steer: bool,
}

pub(super) struct Surroundings<'a> {
    pub boxes: &'a dyn Fn(i32, i32, i32) -> &'static [Aabb],
    pub obstacles: &'a [DynBox],
    pub escape_obstacles: &'a [DynBox],
    pub immersion: Option<Immersion>,
    pub current: FluidCurrent,
}

#[cfg(test)]
impl<'a> Surroundings<'a> {
    pub(super) fn dry(boxes: &'a dyn Fn(i32, i32, i32) -> &'static [Aabb]) -> Self {
        Surroundings {
            boxes,
            obstacles: &[],
            escape_obstacles: &[],
            immersion: None,
            current: FluidCurrent::NONE,
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub(super) struct KinematicPose {
    pub pos: petramond_math::world_pos::WorldPos,
    pub yaw: f32,
    pub tilt: Tilt,
}

const BODY_LEVEL_RATE: f32 = 4.0;

impl Instance {
    pub(super) fn set_kinematic(&mut self, pose: KinematicPose) -> bool {
        if self.combat.death.is_dead() {
            return false;
        }
        self.motion.kinematic = Some(pose);
        true
    }

    pub(super) fn place_kinematic(&mut self, dt: f32, pose: KinematicPose) {
        self.motion.vel = (pose.pos - self.pos) / dt.max(1e-6);
        self.pos = pose.pos;
        self.yaw = pose.yaw;
        self.tilt = pose.tilt;
        self.motion.on_ground = false;
        self.motion.fall_peak_y = pose.pos.y;
        self.moving = false;
        self.motion.air_walk = false;
        self.motion.walk_launch = false;
        self.motion.knockback = Vec3::ZERO;
        self.combat.stagger_timer = 0.0;
        self.motion.push = Vec3::ZERO;
    }

    pub(super) fn level_body(&mut self, dt: f32) {
        self.tilt = self.tilt.toward_level(BODY_LEVEL_RATE * dt);
    }

    pub(super) fn take_fall_distance(&mut self) -> Option<f32> {
        let distance = std::mem::replace(&mut self.motion.fall_distance, 0.0);
        (distance > 0.0).then_some(distance)
    }

    pub(super) fn take_splash_drop(&mut self) -> Option<f32> {
        let drop = std::mem::replace(&mut self.motion.splash_drop, 0.0);
        (drop > 0.0).then_some(drop)
    }

    pub(super) fn set_drive(&mut self, intent: DriveIntent) -> bool {
        if self.combat.death.is_dead() {
            return false;
        }
        self.motion.drive = Some(intent);
        true
    }

    #[inline]
    pub(super) fn clear_drive(&mut self) {
        self.motion.drive = None;
        self.motion.kinematic = None;
    }

    pub(super) fn externally_driven(&self) -> bool {
        self.motion.drive.is_some() || self.motion.kinematic.is_some()
    }

    #[cfg(test)]
    pub(super) fn drive_pending(&self) -> bool {
        self.motion.drive.is_some()
    }

    #[inline]
    pub fn vel(&self) -> Vec3 {
        self.motion.vel
    }

    pub(super) fn commit_solid_motion(&mut self, motion: super::BodyMotion, fraction: f32) {
        if fraction >= 1.0 - 1e-6 {
            return;
        }
        debug_assert_eq!(self.id, motion.id);
        let proposed_delta = motion.end_pos - motion.start_pos;
        (self.pos, self.yaw) = motion.pose_at(fraction);
        if proposed_delta.x.abs() > 1e-6 {
            self.motion.vel.x = 0.0;
        }
        if proposed_delta.z.abs() > 1e-6 {
            self.motion.vel.z = 0.0;
        }
        if proposed_delta.y.abs() > 1e-6 {
            self.motion.vel.y = 0.0;
            self.motion.on_ground = false;
        }
    }

    pub(super) fn land_on_solid_peer(&mut self) {
        self.motion.vel.y = 0.0;
        self.motion.on_ground = true;
    }

    pub(super) fn set_push(&mut self, push: Vec3) {
        self.motion.push = push;
    }

    pub(super) fn finish_motion(&mut self, was_on_ground: bool, immersion: Option<Immersion>) {
        if let Some(sample) = immersion {
            let drop = (self.motion.fall_peak_y - self.pos.y) as f32;
            if sample.fluid.splash.is_some() && drop > 0.0 {
                self.motion.splash_drop = self.motion.splash_drop.max(drop);
            }
            self.motion.fall_peak_y = self.pos.y;
        } else if self.motion.on_ground {
            if !was_on_ground {
                let dist = (self.motion.fall_peak_y - self.pos.y) as f32;
                if dist > self.motion.fall_distance {
                    self.motion.fall_distance = dist;
                }
            }
            self.motion.fall_peak_y = self.pos.y;
        } else {
            self.motion.fall_peak_y = self.motion.fall_peak_y.max(self.pos.y);
        }
    }

    /// Integrate this tick's locomotion against shared fluid forces and
    /// collision. While unsupported and falling, path steering is suspended
    /// and existing horizontal velocity carries through the fall; the upward
    /// phase of a navigation jump keeps steering so the mob can clear a
    /// one-block ledge. The mob faces its **wish** direction — where it wants
    /// to go — so it keeps facing forward even when pressed against a wall
    /// (where its actual velocity would be zero). Returns the mandatory
    /// escape pre-pass displacement separately for the peer-motion proposal.
    pub(super) fn integrate_locomotion(
        &mut self,
        dt: f32,
        d: &MobDef,
        loco: Locomotion,
        env: &Surroundings<'_>,
    ) -> [f32; 3] {
        let was_grounded = self.motion.on_ground;
        let incoming = self.motion.vel;
        let nav_jumped = loco.jump && self.motion.on_ground && env.immersion.is_none();
        if nav_jumped {
            self.motion.vel.y = d.jump_speed;
            self.motion.on_ground = false;
        }
        let drive = self.motion.drive.take();
        let requested_yaw = self.steer_horizontal(dt, d, loco, drive, nav_jumped);
        let premise_holds = drive.is_none_or(|dr| !dr.while_walking || self.moving);
        let drive_steers = premise_holds && loco.can_steer && self.combat.stagger_timer <= 0.0;
        if !nav_jumped && drive_steers {
            self.drive_vertical(drive);
        }
        if self.moving && !self.motion.on_ground && self.motion.vel.y > 0.0 && was_grounded {
            self.motion.walk_launch = true;
        }
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
        if d.edge_guard && self.motion.on_ground && !nav_jumped && env.immersion.is_none() {
            self.guard_edge(dt, d, env);
        }
        let carried =
            (self.combat.stagger_timer <= 0.0 && !loco.can_steer && env.immersion.is_none())
                .then_some([self.motion.vel.x, self.motion.vel.z]);
        let heading = Vec3::new(self.motion.vel.x, 0.0, self.motion.vel.z);
        self.resist_and_push(dt, incoming, env);
        let shore = self.vertical_velocity(dt, d, loco.can_steer, heading, env);
        self.resolve_motion(dt, d, shore, carried, env)
    }

    /// Edge guard for `"edge_guard"` species. A grounded body won't step off a drop bigger than
    /// routes plan for. Smaller drops step down fine; a taller ledge pulls the axis back to the
    /// lip, like a sneaking player, so a body steering along a wall top or roof edge slides along.
    ///
    /// Routes count drop in cells, feet cell to landing cell. Floor can be anywhere in its cell
    /// (slab top, chest top). Lowest landing floor for a planned drop sits just above the top of
    /// the cell `max_drop + 1` below the feet.
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
        let (dx, dz) = (self.motion.vel.x * dt, self.motion.vel.z * dt);
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
            self.motion.vel.x = cx / dt;
        }
        if cz != dz {
            self.motion.vel.z = cz / dt;
        }
    }

    fn steer_horizontal(
        &mut self,
        dt: f32,
        d: &MobDef,
        loco: Locomotion,
        drive: Option<DriveIntent>,
        nav_jumped: bool,
    ) -> Option<f32> {
        self.motion.stepping = false;
        if self.combat.stagger_timer > 0.0 {
            self.motion.vel.x = self.motion.knockback.x;
            self.motion.vel.z = self.motion.knockback.z;
            self.motion.knockback *= KNOCKBACK_DAMP;
            self.moving = false;
        } else if let Some([vx, vz]) = drive.and_then(|dr| dr.horizontal) {
            if loco.can_steer {
                self.motion.vel.x = vx;
                self.motion.vel.z = vz;
            }
            let speed = vx.hypot(vz);
            self.moving =
                drive.is_some_and(|dr| dr.gait) && loco.can_steer && speed > GAIT_MIN_SPEED;
            self.motion.stepping = self.moving;
            if self.moving {
                self.motion.gait_pace = (speed / d.walk_speed.max(1e-3)).clamp(GAIT_MIN_PACE, 1.0);
            }
        } else if loco.can_steer {
            let wish = loco.wish;
            self.moving = wish.length_squared() > 1e-6;
            self.motion.gait_pace = 1.0;
            let mut speed = d.walk_speed * self.motion.walk_speed_scale;
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
            self.motion.vel.x = wish.x * speed;
            self.motion.vel.z = wish.z * speed;
            return requested_yaw;
        } else {
            // Unsteered mid-air (a fall's descent) a mob whose airborne arc
            // BEGAN as a walk still reads as walking while its horizontal
            // motion carries — the gait clip plays through the whole
            // ballistic arc instead of snapping to the rest pose at the
            // apex (a mod-authored hop, a navigation jump, a walked-off
            // ledge alike).
            self.moving = self.motion.air_walk
                && !self.motion.on_ground
                && self.motion.vel.x * self.motion.vel.x + self.motion.vel.z * self.motion.vel.z
                    > 1e-6;
        }
        None
    }

    fn drive_vertical(&mut self, drive: Option<DriveIntent>) {
        if let Some(vy) = drive.and_then(|dr| dr.vertical) {
            self.motion.vel.y = vy;
            if vy > 0.0 && self.motion.on_ground {
                self.motion.on_ground = false;
            }
        }
    }

    fn resist_and_push(&mut self, dt: f32, incoming: Vec3, env: &Surroundings<'_>) {
        if let Some(sample) = env.immersion {
            let desired = self.motion.vel;
            let incoming = Vec3::new(incoming.x, self.motion.vel.y, incoming.z);
            self.motion.vel = sample
                .fluid
                .motion
                .horizontal_velocity(incoming, desired, dt);
        }
        self.motion.vel.x += self.motion.push.x;
        self.motion.vel.z += self.motion.push.z;
        self.motion.push = Vec3::ZERO;
        self.motion.vel = env.current.apply(self.motion.vel, dt);
    }

    fn vertical_velocity(
        &mut self,
        dt: f32,
        d: &MobDef,
        can_steer: bool,
        heading: Vec3,
        env: &Surroundings<'_>,
    ) -> Option<ShoreClimb> {
        let Some(sample) = env.immersion else {
            self.motion.vel.y += GRAVITY * d.gravity_scale * dt;
            return None;
        };
        let shore = (d.buoyancy == Buoyancy::Swim && can_steer && self.combat.stagger_timer <= 0.0)
            .then(|| {
                Swimmer {
                    pos: self.pos,
                    vel_y: self.motion.vel.y,
                    half_width: d.size.half_width,
                    height: d.size.height,
                    gravity: -GRAVITY * d.gravity_scale,
                    jump_speed: d.jump_speed,
                }
                .shore_climb(heading, sample, &env.boxes, env.obstacles)
            })
            .flatten();
        if let Some(ShoreClimb::Launch(speed)) = shore {
            self.motion.vel.y = self.motion.vel.y.max(speed);
        } else {
            self.motion.vel.y = sample.vertical_velocity(
                self.motion.vel.y,
                self.pos.y as f32,
                d.buoyancy,
                true,
                dt,
            );
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
    ) -> [f32; 3] {
        let step = match shore {
            Some(ShoreClimb::Step(height)) => height.max(collision::STEP_HEIGHT),
            _ => collision::STEP_HEIGHT,
        };
        let (moved, grounded, hit, healed) = super::resolve_body_motion(
            self.pos,
            self.yaw,
            d.size,
            self.motion.vel.to_array(),
            dt,
            step,
            matches!(shore, Some(ShoreClimb::Step(_))),
            &mut self.motion.escape,
            &env.boxes,
            env.obstacles,
            env.escape_obstacles,
            self.id,
        );
        self.pos += Vec3::from(moved);
        if hit[0] {
            self.motion.vel.x = 0.0;
        }
        if hit[1] {
            self.motion.vel.y = 0.0;
        }
        if hit[2] {
            self.motion.vel.z = 0.0;
        }
        if let Some([x, z]) = carried {
            if !hit[0] {
                self.motion.vel.x = x;
            }
            if !hit[2] {
                self.motion.vel.z = z;
            }
        }
        self.motion.on_ground = grounded;
        if grounded && self.motion.vel.y < 0.0 {
            self.motion.vel.y = 0.0;
        }
        self.motion.air_walk = !grounded
            && (self.moving
                || (self.motion.air_walk
                    && self.motion.vel.x * self.motion.vel.x
                        + self.motion.vel.z * self.motion.vel.z
                        > 1e-6));
        healed
    }

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

    pub fn on_ground(&self) -> bool {
        self.motion.on_ground
    }

    pub fn entombed(&self) -> bool {
        self.motion.escape.entombed()
    }
}

fn heading_yaw(v: Vec3) -> f32 {
    (-v.x).atan2(-v.z)
}

fn facing_speed_factor(delta: f32) -> f32 {
    delta.cos().max(0.0)
}

pub(super) fn turn_toward(yaw: f32, target: f32, max_step: f32) -> f32 {
    let delta = wrap_angle(target - yaw);
    let step = max_step.min(delta.abs());
    wrap_angle(yaw + step * delta.signum())
}

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
