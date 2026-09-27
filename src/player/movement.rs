use super::collision::Axis;
use super::state::{Input, Player, HEIGHT};
use crate::entity::shore::ShoreClimb;
use crate::world::{Climb, WorldData};
use petramond_math::math::Vec3;
use petramond_world::block::Aabb;
use petramond_world::collision::{self, DynBox};
use petramond_world::fluid::{Buoyancy, FluidCurrent, Immersion};

pub const WALK: f32 = 4.3;
pub const SPRINT: f32 = 5.6;
pub(super) const SNEAK_FACTOR: f32 = 0.5;
pub(super) const SPECTATOR_SPEED: f32 = 48.0;
pub const SPECTATOR_SPRINT: f32 = 96.0;
pub const GRAVITY: f32 = 28.0;
pub const JUMP_V0: f32 = 8.4;
pub const TERMINAL: f32 = 30.0;
pub(super) const GROUND_FRICTION: f32 = 0.2;
pub(super) const ICE_FRICTION: f32 = 0.02;
pub(super) const ICE_ACCEL: f32 = 9.0;
pub(super) const AIR_FRICTION: f32 = 0.05;
pub(super) const GROUND_ACCEL: f32 = 60.0;
pub(super) const AIR_ACCEL: f32 = 20.0;
pub const SWIM_SPEED: f32 = 2.2;
pub const CLIMB_SPEED: f32 = WALK * 0.5;
const CLIMB_VACCEL: f32 = 40.0;
pub(super) const CLIMB_LATERAL_SPEED: f32 = WALK * SNEAK_FACTOR;
const CLIMB_FRICTION: f32 = 0.5;
pub(super) const FRICTION_REF_DT: f32 = 1.0 / 60.0;
const APEX_VY: f32 = 3.0;
const APEX_GRAVITY: f32 = 0.7;

pub(super) struct Surroundings<'a> {
    pub boxes: &'a dyn Fn(i32, i32, i32) -> &'static [Aabb],
    pub fluid: &'a dyn Fn(petramond_math::world_pos::WorldPos) -> Option<Immersion>,
    pub current: &'a dyn Fn(petramond_math::world_pos::WorldPos) -> FluidCurrent,
    pub climb: &'a dyn Fn(i32, i32, i32) -> Option<Climb>,
    pub slippery: &'a dyn Fn(i32, i32, i32) -> bool,
    pub obstacles: &'a [DynBox],
}

#[cfg(test)]
impl<'a> Surroundings<'a> {
    pub(super) fn dry(boxes: &'a dyn Fn(i32, i32, i32) -> &'static [Aabb]) -> Self {
        fn no_fluid(_: petramond_math::world_pos::WorldPos) -> Option<Immersion> {
            None
        }
        fn still(_: petramond_math::world_pos::WorldPos) -> FluidCurrent {
            FluidCurrent::NONE
        }
        fn no_climb(_: i32, _: i32, _: i32) -> Option<Climb> {
            None
        }
        fn grippy(_: i32, _: i32, _: i32) -> bool {
            false
        }
        Surroundings {
            boxes,
            fluid: &no_fluid,
            current: &still,
            climb: &no_climb,
            slippery: &grippy,
            obstacles: &[],
        }
    }
}

#[derive(Clone, Copy)]
struct Medium {
    swim: Option<Immersion>,
    ladder: Option<Climb>,
    flow: FluidCurrent,
    shore: Option<ShoreClimb>,
}

impl Player {
    pub fn wish_speed(&self, input: Input) -> f32 {
        let wishing = input.wishdir.length_squared() > 1e-12;
        self.move_scale()
            * if input.sneak && wishing {
                WALK * SNEAK_FACTOR
            } else if input.sprint && wishing {
                SPRINT
            } else {
                WALK
            }
    }

    pub fn shove(&mut self, delta: Vec3, world: &WorldData) {
        if self.is_spectator() || (delta.x == 0.0 && delta.z == 0.0) {
            return;
        }
        let cells = world.cursor();
        let boxes = |x: i32, y: i32, z: i32| cells.collision_boxes_xyz(x, y, z);
        self.sweep_boxes(Axis::X, delta.x, &boxes);
        self.sweep_boxes(Axis::Z, delta.z, &boxes);
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn update(&mut self, dt: f32, world: &WorldData, input: Input) {
        self.update_with_obstacles(dt, world, input, &[]);
    }

    pub fn update_with_obstacles(
        &mut self,
        dt: f32,
        world: &WorldData,
        input: Input,
        obstacles: &[DynBox],
    ) {
        let cells = world.cursor();
        let boxes = |x: i32, y: i32, z: i32| cells.collision_boxes_xyz(x, y, z);
        let fluid = |feet: petramond_math::world_pos::WorldPos| {
            world.body_fluid(feet, HEIGHT, Buoyancy::Swim)
        };
        let current = |p: petramond_math::world_pos::WorldPos| world.fluid_current_at(p);
        let climb = |x: i32, y: i32, z: i32| world.climb_at(x, y, z);
        let slippery = |x: i32, y: i32, z: i32| world.physics_block(x, y, z).is_slippery();
        let env = Surroundings {
            boxes: &boxes,
            fluid: &fluid,
            current: &current,
            climb: &climb,
            slippery: &slippery,
            obstacles,
        };
        self.simulate(dt, &env, input);
    }

    #[cfg(test)]
    pub(super) fn update_core(
        &mut self,
        dt: f32,
        solid: &dyn Fn(i32, i32, i32) -> bool,
        input: Input,
    ) {
        self.update_core_env(dt, solid, &|_, _, _| None, &|_, _, _| false, input);
    }

    #[cfg(test)]
    pub(super) fn update_core_slippery(
        &mut self,
        dt: f32,
        solid: &dyn Fn(i32, i32, i32) -> bool,
        slippery: &dyn Fn(i32, i32, i32) -> bool,
        input: Input,
    ) {
        self.update_core_env(dt, solid, &|_, _, _| None, slippery, input);
    }

    #[cfg(test)]
    pub(super) fn update_core_climb(
        &mut self,
        dt: f32,
        solid: &dyn Fn(i32, i32, i32) -> bool,
        climb: &dyn Fn(i32, i32, i32) -> Option<Climb>,
        input: Input,
    ) {
        self.update_core_env(dt, solid, climb, &|_, _, _| false, input);
    }

    #[cfg(test)]
    fn update_core_env(
        &mut self,
        dt: f32,
        solid: &dyn Fn(i32, i32, i32) -> bool,
        climb: &dyn Fn(i32, i32, i32) -> Option<Climb>,
        slippery: &dyn Fn(i32, i32, i32) -> bool,
        input: Input,
    ) {
        let boxes = |x: i32, y: i32, z: i32| {
            if solid(x, y, z) {
                petramond_world::block::Block::Stone
            } else {
                petramond_world::block::Block::Air
            }
            .collision_boxes()
        };
        let env = Surroundings {
            climb,
            slippery,
            ..Surroundings::dry(&boxes)
        };
        self.simulate(dt, &env, input);
    }

    pub(super) fn simulate(&mut self, dt: f32, env: &Surroundings<'_>, input: Input) {
        if self.is_spectator() {
            self.update_spectator(dt, input);
            return;
        }
        if self.is_flying() {
            self.update_creative_flight(dt, env, input);
            return;
        }
        let was_on_ground = self.on_ground;
        if self.escape_geometry(dt, env) {
            return;
        }
        let medium = self.sample_medium(env, input);
        self.vertical_velocity(dt, input, medium, was_on_ground);
        self.move_vertical(dt, env);
        self.horizontal_velocity(dt, input, medium, env);
        self.vel = medium.flow.apply(self.vel, dt);
        self.move_horizontal(dt, input, medium, env);
        self.track_fall(
            was_on_ground,
            medium.swim.is_some() || medium.ladder.is_some(),
        );
    }

    fn centre_column(&self) -> (i32, i32) {
        (self.pos.x.floor() as i32, self.pos.z.floor() as i32)
    }

    fn sample_medium(&self, env: &Surroundings<'_>, input: Input) -> Medium {
        let swim = (env.fluid)(self.pos);
        let ladder = if swim.is_some() {
            None
        } else {
            let (x, z) = self.centre_column();
            (env.climb)(x, self.pos.y.floor() as i32, z)
        };
        Medium {
            swim,
            ladder,
            flow: petramond_world::fluid::sample_body_current(self.pos, HEIGHT, swim, env.current),
            shore: swim.and_then(|swim| self.shore_climb(swim, input, &env.boxes, env.obstacles)),
        }
    }

    fn vertical_velocity(&mut self, dt: f32, input: Input, medium: Medium, was_on_ground: bool) {
        if let Some(swim) = medium.swim {
            self.swim_vertical(dt, swim, input, medium.shore);
        } else if let Some(grip) = medium.ladder {
            // Climbing: while the feet stand in a climbable cell, vertical speed
            // is fully controlled — no gravity, no jump impulse. Moving INTO the
            // panel (the wish direction pointing at the wall it hangs on) or
            // holding jump climbs; otherwise the body slides down gently, and a
            // fall through the cell is caught by the hard clamp (the "grab").
            // The speed is a fraction of base WALK on purpose: sprint and sneak
            // change nothing here.
            // A FREE-hanging climbable (a vine curtain) has no wall to press
            // against, so jump is its only ascent — pressing a compass
            // direction must do nothing, which is why the grip carries the
            // DECLARED facing rather than `Block::panel_facing`'s North default.
            let into_wall = |facing: petramond_math::facing::Facing| {
                let d = facing.dir();
                -(input.wishdir.x * d.x as f32 + input.wishdir.z * d.z as f32)
            };
            let ascending = input.jump || matches!(grip, Climb::Panel(f) if into_wall(f) > 1e-3);
            let target = if ascending { CLIMB_SPEED } else { -CLIMB_SPEED };
            self.vel.y = approach(
                self.vel.y.clamp(-CLIMB_SPEED, CLIMB_SPEED),
                target,
                CLIMB_VACCEL * dt,
            );
            self.jumping = false;
        } else {
            if input.jump && was_on_ground {
                self.vel.y = JUMP_V0;
                self.jumping = true;
            }
            let g = if self.jumping {
                let t = (self.vel.y.abs() / APEX_VY).min(1.0);
                GRAVITY * (APEX_GRAVITY + (1.0 - APEX_GRAVITY) * t)
            } else {
                GRAVITY
            };
            self.vel.y = (self.vel.y - g * dt).max(-TERMINAL);
        }
    }

    pub(super) fn escape_geometry(&mut self, dt: f32, env: &Surroundings<'_>) -> bool {
        let (mut mn, mut mx) = (self.aabb_min(), self.aabb_max());
        let (off, escaping) = collision::escape_pre_pass(
            &mut mn,
            &mut mx,
            dt,
            &mut self.escape,
            &env.boxes,
            env.obstacles,
            collision::NOT_AN_ENTITY,
        );
        self.pos += Vec3::from(off);
        if escaping {
            self.vel = Vec3::ZERO;
            self.on_ground = false;
            self.jumping = false;
            self.fall_peak_y = self.pos.y;
        }
        escaping
    }

    fn move_vertical(&mut self, dt: f32, env: &Surroundings<'_>) {
        let dy = self.vel.y * dt;
        if self.sweep_boxes_dyn(Axis::Y, dy, &env.boxes, env.obstacles) {
            self.on_ground = dy < 0.0;
            self.vel.y = 0.0;
            self.jumping = false;
        } else {
            self.on_ground = false;
        }
    }

    fn horizontal_velocity(
        &mut self,
        dt: f32,
        input: Input,
        medium: Medium,
        env: &Surroundings<'_>,
    ) {
        let speed = self.wish_speed(input);
        let wish = if input.wishdir.length_squared() > 1.0 {
            input.wishdir.normalize()
        } else {
            input.wishdir
        };
        let grounded = self.on_ground;
        let on_slippery = grounded && {
            let (x, z) = self.centre_column();
            (env.slippery)(x, (self.pos.y - 0.05).floor() as i32, z)
        };
        if let Some(swim) = medium.swim {
            self.vel = swim
                .fluid
                .motion
                .horizontal_velocity(self.vel, wish * SWIM_SPEED, dt);
        } else if medium.ladder.is_some() {
            self.ladder_lateral(dt, wish);
        } else if wish.length_squared() <= 1e-12 {
            let retain = friction_retain(
                if on_slippery {
                    ICE_FRICTION
                } else if grounded {
                    GROUND_FRICTION
                } else {
                    AIR_FRICTION
                },
                dt,
            );
            self.vel.x *= retain;
            self.vel.z *= retain;
        } else if grounded {
            let accel = if on_slippery { ICE_ACCEL } else { GROUND_ACCEL };
            (self.vel.x, self.vel.z) = move_toward(
                self.vel.x,
                self.vel.z,
                wish.x * speed,
                wish.z * speed,
                accel * dt,
            );
        } else {
            self.air_steer(dt, wish, speed);
        }
    }

    fn ladder_lateral(&mut self, dt: f32, wish: Vec3) {
        if wish.length_squared() <= 1e-12 {
            let retain = friction_retain(CLIMB_FRICTION, dt);
            self.vel.x *= retain;
            self.vel.z *= retain;
        } else {
            (self.vel.x, self.vel.z) = move_toward(
                self.vel.x,
                self.vel.z,
                wish.x * CLIMB_LATERAL_SPEED,
                wish.z * CLIMB_LATERAL_SPEED,
                GROUND_ACCEL * dt,
            );
        }
    }

    fn air_steer(&mut self, dt: f32, wish: Vec3, speed: f32) {
        let speed_sq_before = self.vel.x * self.vel.x + self.vel.z * self.vel.z;
        let along = self.vel.x * wish.x + self.vel.z * wish.z;
        let add = (speed - along).max(0.0);
        let step = (AIR_ACCEL * dt).min(add);
        self.vel.x += wish.x * step;
        self.vel.z += wish.z * step;
        let speed_sq_after = self.vel.x * self.vel.x + self.vel.z * self.vel.z;
        let cap_sq = speed_sq_before.max(speed * speed);
        if speed_sq_after > cap_sq {
            let scale = (cap_sq / speed_sq_after).sqrt();
            self.vel.x *= scale;
            self.vel.z *= scale;
        }
    }

    fn move_horizontal(&mut self, dt: f32, input: Input, medium: Medium, env: &Surroundings<'_>) {
        let sneak_guard = input.sneak && self.on_ground && medium.swim.is_none();
        let (dx, dz) = if sneak_guard {
            self.sneak_clamp(self.vel.x * dt, self.vel.z * dt, env)
        } else {
            (self.vel.x * dt, self.vel.z * dt)
        };
        let ground_step = if self.on_ground {
            collision::STEP_HEIGHT
        } else {
            0.0
        };
        let step = match medium.shore {
            Some(ShoreClimb::Step(height)) => ground_step.max(height),
            _ => ground_step,
        };
        let (mn, mx) = (self.aabb_min(), self.aabb_max());
        let (moved, hit_x, hit_z) = collision::step_horizontal_dyn(
            mn,
            mx,
            dx,
            dz,
            step,
            env.boxes,
            env.obstacles,
            collision::NOT_AN_ENTITY,
        );
        self.pos += Vec3::from(moved);
        if hit_x {
            self.vel.x = 0.0;
        }
        if hit_z {
            self.vel.z = 0.0;
        }
        if sneak_guard {
            self.sneak_settle(env);
        }
    }

    fn sneak_clamp(&mut self, dx: f32, dz: f32, env: &Surroundings<'_>) -> (f32, f32) {
        let (mn, mx) = (self.aabb_min(), self.aabb_max());
        let (cx, cz) = collision::clamp_to_supported_dyn(
            mn,
            mx,
            dx,
            dz,
            collision::STEP_HEIGHT,
            env.boxes,
            env.obstacles,
            collision::NOT_AN_ENTITY,
        );
        if cx != dx {
            self.vel.x = 0.0;
        }
        if cz != dz {
            self.vel.z = 0.0;
        }
        (cx, cz)
    }

    /// Sneak step-down snaps instantly, like auto step-up. With gravity, `on_ground` stays false
    /// for ~10 frames, the edge guard drops out, and momentum carries the body off the far edge of
    /// the landing block.
    /// Probe uses the clamp's margin: drops the guard allowed land here, deeper ones stay put since
    /// only the clamp can refuse those.
    fn sneak_settle(&mut self, env: &Surroundings<'_>) {
        let probe = -(collision::STEP_HEIGHT + collision::SUPPORT_PROBE_MARGIN);
        let (mn, mx) = (self.aabb_min(), self.aabb_max());
        let down = collision::sweep_axis_dyn(
            mn,
            mx,
            1,
            probe,
            env.boxes,
            env.obstacles,
            collision::NOT_AN_ENTITY,
        );
        if down > probe {
            self.pos.y += f64::from(down);
        }
    }

    fn update_spectator(&mut self, dt: f32, input: Input) {
        let dir = input.wishdir.normalize_or_zero();
        let speed = if input.sprint {
            SPECTATOR_SPRINT
        } else {
            SPECTATOR_SPEED
        };
        self.vel = dir * speed * self.fly_scale();
        self.pos += self.vel * dt;
        self.on_ground = false;
        self.jumping = false;
    }
}

#[inline]
pub(super) fn friction_retain(friction: f32, dt: f32) -> f32 {
    if friction >= 1.0 {
        0.0
    } else {
        (1.0 - friction).powf(dt / FRICTION_REF_DT)
    }
}

#[inline]
pub(super) fn move_toward(x: f32, z: f32, tx: f32, tz: f32, max_delta: f32) -> (f32, f32) {
    let (dx, dz) = (tx - x, tz - z);
    let dist_sq = dx * dx + dz * dz;
    if dist_sq <= max_delta * max_delta || dist_sq == 0.0 {
        (tx, tz)
    } else {
        let scale = max_delta / dist_sq.sqrt();
        (x + dx * scale, z + dz * scale)
    }
}

#[inline]
pub(super) fn approach(v: f32, target: f32, max_delta: f32) -> f32 {
    let d = target - v;
    if d.abs() <= max_delta {
        target
    } else {
        v + d.signum() * max_delta
    }
}
