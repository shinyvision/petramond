use super::collision::Axis;
use super::state::{Input, Player, HEIGHT};
use crate::entity::shore::ShoreClimb;
use crate::world::{Climb, World};
use petramond_math::math::Vec3;
use petramond_world::block::Aabb;
use petramond_world::collision::{self, DynBox};
use petramond_world::fluid::{Buoyancy, FluidCurrent, Immersion};

pub const WALK: f32 = 4.3;
pub const SPRINT: f32 = 5.6;
/// Land-speed multiplier while sneaking (applies to walk; sneak overrides sprint).
pub(super) const SNEAK_FACTOR: f32 = 0.5;
pub(super) const SPECTATOR_SPEED: f32 = 48.0;
pub const SPECTATOR_SPRINT: f32 = 96.0;
pub const GRAVITY: f32 = 28.0;
/// Jump take-off speed. Apex height = v0² / (2·g) = 8.4²/56 ≈ 1.26 blocks, so a
/// held jump clears a single full block with margin.
pub const JUMP_V0: f32 = 8.4;
pub const TERMINAL: f32 = 30.0;
/// Horizontal friction on the ground — purely a decay rate, applied only when
/// there is no input: the fraction of the player's speed shed in one reference
/// frame (see [`friction_retain`]). Modest, so a body that lands or stops with
/// residual speed skids to a *gradual* halt (~0.7 m, ~0.5 s from walk speed)
/// rather than stopping dead — firmer than the air, but still a slide, not a snap.
pub(super) const GROUND_FRICTION: f32 = 0.2;
/// Horizontal friction while idle on SLIPPERY ground (ice, packed ice — the
/// [`BlockTag::SLIPPERY`](petramond_world::block::BlockTag::SLIPPERY) rows): a tenth of
/// the ordinary ground decay, so momentum carries and a walk-off glides.
pub(super) const ICE_FRICTION: f32 = 0.02;
/// Ground acceleration on slippery ground: the whole "walking on ice" feel is
/// this one reduced snap rate — starts, stops, and turns all smear while top
/// speed stays the ordinary walk/sprint speed (`move_toward` still aims at
/// the same wish velocity).
pub(super) const ICE_ACCEL: f32 = 9.0;
/// Horizontal friction in the air — the decay rate while coasting (no input).
/// Very low, so after a jump the player keeps almost all of its horizontal
/// momentum and drifts a long way before stopping (retains ~99 % per frame, so
/// roughly half the speed survives a full second of free coasting and it bleeds
/// to zero only very gradually). This and the gentle, additive air acceleration
/// are what let a jump carry its momentum.
pub(super) const AIR_FRICTION: f32 = 0.05;
/// Horizontal acceleration on the ground (m/s²): how fast `move_toward` snaps the
/// velocity to the wish velocity while a direction is held. High, so the ground
/// feels snappy — top speed reached in a few frames, with crisp turns and stops.
/// Independent of friction, so top speed is exactly the walk/sprint speed.
pub(super) const GROUND_ACCEL: f32 = 60.0;
/// Horizontal acceleration in the air (m/s²). Low, and applied *additively* along
/// the input direction only (never braking), so mid-air input merely nudges the
/// trajectory: you keep the momentum a jump launched you with and gently steer,
/// never snap to a new direction. The air counterpart to [`GROUND_ACCEL`].
pub(super) const AIR_ACCEL: f32 = 20.0;
pub const SWIM_SPEED: f32 = 2.2;
pub const CLIMB_SPEED: f32 = WALK * 0.5;
const CLIMB_VACCEL: f32 = 40.0;
/// Sideways speed while on a ladder: the sneak factor of walk. Full walk speed
/// with air-style drift scooted the body off the panel's edge mid-climb — the
/// "slippery wall"; halved, ground-snapped movement makes lateral input a
/// deliberate reposition instead of a slide.
pub(super) const CLIMB_LATERAL_SPEED: f32 = WALK * SNEAK_FACTOR;
/// Horizontal friction while idle on a ladder: heavy, so releasing the stick
/// stops sideways drift almost immediately — hands on the rungs, not skates.
const CLIMB_FRICTION: f32 = 0.5;
/// Reference timestep the friction fractions are calibrated to: at exactly this
/// `dt` the player sheds `friction` of its speed in one frame (ground 10 %, air
/// 1 %). [`friction_retain`] rescales to any other `dt` so the slowdown per
/// second is identical regardless of frame rate or sub-step length. 60 Hz.
pub(super) const FRICTION_REF_DT: f32 = 1.0 / 60.0;
/// Apex easing band: within this |vel.y| (m/s) of the top of a jump, gravity is
/// scaled toward `APEX_GRAVITY`, rounding the up→down transition rather than
/// snapping through it.
const APEX_VY: f32 = 3.0;
/// Gravity multiplier at the exact apex (vel.y = 0), ramping linearly back to
/// 1.0 by `APEX_VY`. Slightly below 1 so the peak floats a touch; the band is
/// narrow enough that overall jump height barely changes.
const APEX_GRAVITY: f32 = 0.7;

/// What player physics reads about the body's surroundings: a `World` in
/// production, stubs in the land-physics tests.
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
    /// Dry, still, ladderless, grippy surroundings over `boxes`.
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

/// What the body is in this tick, sampled once before it moves.
#[derive(Clone, Copy)]
struct Medium {
    swim: Option<Immersion>,
    ladder: Option<Climb>,
    flow: FluidCurrent,
    shore: Option<ShoreClimb>,
}

impl Player {
    /// The land speed this INPUT wishes: sneak, sprint or walk, scaled by the
    /// body's [`move_scale`](crate::player::state::Player::move_scale) — ONE
    /// number, into which status effects, the mode and every pack's claim have
    /// already been folded. The mode keys are modifiers on a WISH — with no
    /// deliberate movement there is nothing for them to modify, so a held
    /// sprint key over planted feet selects plain walk — while the scale is the
    /// body's own state and applies whether or not it moves.
    ///
    /// This is the one statement of that selection: movement applies it (the
    /// wish gate costs it nothing — a zero wish never reads the speed), and
    /// any presentation that follows the body's speed (the speed-widened FOV)
    /// reads the same number, so a new cause of speed — an effect, a future
    /// mode — reaches every consumer without per-cause wiring anywhere.
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

    /// Shove the player horizontally by `delta` — the soft push from a mob it overlaps
    /// (mobs and the player push each other apart, but neither has a solid collision box).
    /// Applied per frame as a small collision-resolved displacement (the per-frame push
    /// velocity × `dt`), sliding along blocks via the same swept collision as movement so
    /// it can't shove the player through terrain. Velocity is untouched, so the push
    /// neither accumulates nor fights the movement controller — the player just drifts out
    /// of the overlap smoothly and can still walk against it. Vertical is ignored (pushing
    /// is horizontal); a noclip spectator has no body to jostle.
    pub fn shove(&mut self, delta: Vec3, world: &World) {
        if self.is_spectator() || (delta.x == 0.0 && delta.z == 0.0) {
            return;
        }
        // Position-aware so a multi-cell bbmodel block collides per its own cell shape.
        let boxes = |x: i32, y: i32, z: i32| world.collision_boxes_at(x, y, z);
        self.sweep_boxes(Axis::X, delta.x, &boxes);
        self.sweep_boxes(Axis::Z, delta.z, &boxes);
    }

    /// Advance the player by `dt` seconds against the world's solid voxels
    /// only — production drivers always pass the solid-entity boxes through
    /// [`update_with_obstacles`](Self::update_with_obstacles), so this stays
    /// a test entry. The caller must ensure the overlapped columns are
    /// loaded (see [`Player::columns_loaded`]) before stepping survival
    /// physics. Spectator mode ignores world solidity and may move through
    /// unloaded columns.
    #[cfg(any(test, feature = "test-support"))]
    pub fn update(&mut self, dt: f32, world: &World, input: Input) {
        self.update_with_obstacles(dt, world, input, &[]);
    }

    /// `update` that also resolves against dynamic collision
    /// boxes — solid entities (a boat's hull): the body walks into them and
    /// stops, lands on them and stands. The DRIVERS supply the boxes because
    /// the sources differ by side: the server reads its live mob instances,
    /// the client its interpolated replicated rows.
    pub fn update_with_obstacles(
        &mut self,
        dt: f32,
        world: &World,
        input: Input,
        obstacles: &[DynBox],
    ) {
        // Position-aware so a multi-cell bbmodel block collides per its own cell shape.
        let boxes = |x: i32, y: i32, z: i32| world.collision_boxes_at(x, y, z);
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

    /// Dry physics integration against a cell-solidity predicate, so land feel
    /// is unit-tested without a World. Fluid behavior is tested through
    /// [`Player::update`] on a real world, which owns the immersion query.
    #[cfg(test)]
    pub(super) fn update_core(
        &mut self,
        dt: f32,
        solid: &dyn Fn(i32, i32, i32) -> bool,
        input: Input,
    ) {
        self.update_core_env(dt, solid, &|_, _, _| None, &|_, _, _| false, input);
    }

    /// [`update_core`](Self::update_core) with a slippery-support predicate,
    /// for the ice-glide physics tests.
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

    /// [`update_core`](Self::update_core) with a climbable-cell predicate, for
    /// the ladder physics tests.
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

    /// The one test shim behind the `update_core*` helpers: adapts the test's
    /// bool solidity to the collision-box source (a solid cell is one full
    /// cube) with no fluid and no obstacles.
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

    /// One physics step: sample the medium, integrate vertical then horizontal
    /// velocity, and resolve each against collision.
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
        let medium = self.sample_medium(env, input);
        self.vertical_velocity(dt, input, medium, was_on_ground);
        self.move_vertical(dt, env);
        self.horizontal_velocity(dt, input, medium, env);
        self.vel = medium.flow.apply(self.vel, dt);
        self.move_horizontal(dt, input, medium, env);
        // Measure the fall now that `on_ground` and the final feet `y` are settled; the
        // tick turns a latched landing into damage (see `crate::game::health`). A
        // ladder breaks a fall exactly like swimming: descent on it is controlled.
        self.track_fall(
            was_on_ground,
            medium.swim.is_some() || medium.ladder.is_some(),
        );
    }

    /// The centre column the one-column probes (fluid, ladder, grip) sample.
    fn centre_column(&self) -> (i32, i32) {
        (self.pos.x.floor() as i32, self.pos.z.floor() as i32)
    }

    fn sample_medium(&self, env: &Surroundings<'_>, input: Input) -> Medium {
        let swim = (env.fluid)(self.pos);
        // On a ladder? Sample the feet cell of the body's centre column, like the
        // fluid probe: walking toward a mounted ladder carries the (collisionless)
        // panel cell around the feet. Swimming wins when both apply — a submerged
        // ladder swims, it doesn't climb.
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
                let t = (self.vel.y.abs() / APEX_VY).min(1.0); // 0 at apex -> 1 outside
                GRAVITY * (APEX_GRAVITY + (1.0 - APEX_GRAVITY) * t)
            } else {
                GRAVITY
            };
            self.vel.y = (self.vel.y - g * dt).max(-TERMINAL);
        }
    }

    fn move_vertical(&mut self, dt: f32, env: &Surroundings<'_>) {
        // Heal shallow foot penetration first — a block that GREW under the
        // standing feet (farmland pressed back to full-cube dirt, a machine
        // variant swap) would otherwise be skipped by the sweep and the
        // player tunnels through the floor (see `collision::depenetrate_up`).
        let (mn, mx) = (self.aabb_min(), self.aabb_max());
        self.pos.y += f64::from(collision::depenetrate_up_dyn(
            mn,
            mx,
            collision::STEP_HEIGHT,
            env.boxes,
            env.obstacles,
            collision::NOT_AN_ENTITY,
        ));
        let dy = self.vel.y * dt;
        if self.sweep_boxes_dyn(Axis::Y, dy, &env.boxes, env.obstacles) {
            // Landed if we were moving down; bonked head if moving up. Either way
            // the jump arc is over, so stop easing gravity.
            self.on_ground = dy < 0.0;
            self.vel.y = 0.0;
            self.jumping = false;
        } else {
            self.on_ground = false;
        }
    }

    /// Input accelerates toward the wish velocity; friction decays it. In a
    /// fluid the row's resistance applies; on land it is the ground/air
    /// handling.
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
        // Pick ground vs air coefficients from the *current* (post-vertical-step)
        // state, so the instant you leave the ground — a jump take-off or walking
        // off a ledge — you switch to air handling and your horizontal momentum is
        // no longer subject to the grippy ground friction. A landing flips it
        // straight back, so a touchdown stops you promptly.
        let grounded = self.on_ground;
        // The support block's grip, sampled at the centre column just below the
        // feet — the same one-column simplification as the fluid/ladder probes.
        // Slippery support (ice) swaps the grounded friction + snap constants;
        // airborne and swimming handling are untouched.
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
            // No input: friction is the only horizontal force. Keep the retained
            // fraction (1 - friction) per reference frame, rescaled to this dt so
            // the slowdown per second is the same at any frame rate or sub-step
            // length. friction 0 → retain 1 (coast forever); 1 → retain 0 (stop).
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
            // Ground: snap toward the wish velocity at the high ground acceleration
            // — responsive starts, stops, and reversals, with no stray momentum
            // (move_toward redirects the whole velocity vector, so turning leaves no
            // leftover speed on the axis you stopped steering). Friction is not read
            // here: speeding up is fully decoupled from it. On slippery support the
            // snap rate collapses, so starts/stops/turns smear into a slide.
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

    /// Ladder grip: sideways movement snaps like ground handling toward the
    /// halved lateral speed, and releasing input brakes hard. The default
    /// airborne handling (additive accel, near-zero friction) made the wall
    /// feel slippery while climbing.
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

    /// Air: additive acceleration along the wish direction only — it tops the
    /// wish-direction speed up to `speed` but never brakes, so a jump keeps
    /// the momentum it launched with. The total horizontal speed is then
    /// capped at whatever we already had (or `speed` if slower): input can
    /// *redirect* momentum but never *inflate* it. Without that cap, scraping
    /// a wall pumps speed without bound — the wall zeroes the into-wall
    /// velocity each step, keeping the wish-direction projection low so `add`
    /// stays large, while the perpendicular (along-wall) speed climbs every
    /// frame. The cap makes steering a constant-speed turn and kills that
    /// exploit; friction is the only thing that slows you.
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
        // Sneak edge guard: while grounded (and not swimming), refuse any horizontal
        // move whose destination has no support within a step-down below the feet —
        // stepping down a slab still works (the mirror of the auto step-up), walking
        // off anything taller is pulled back to the ledge lip. Jumping escapes: the
        // take-off's Y sweep already cleared `on_ground`.
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

    /// Clamp a sneaking move to supported ground. A clamped axis also zeroes
    /// its velocity, like a wall hit, so speed doesn't pile up against the edge.
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

    /// Sneak step-down is INSTANT, mirroring the instant auto step-up: settle
    /// the body straight onto the support the edge guard just vouched for. An
    /// airborne half-block drop would take ~10 frames of gravity, and for all
    /// of them `on_ground` is false — the guard disengages and the retained
    /// horizontal momentum can carry the body across the landing block and
    /// off ITS far edge (the diagonal step-down fall-off). Snapping down in
    /// the same move keeps the sneaker grounded through the whole descent, so
    /// the guard holds every frame. The probe shares the clamp's margin: any
    /// drop the guard allowed lands here; anything deeper stays put (only the
    /// untouched clamp can refuse it).
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
            // Blocked within a step: rest on it (0 while anything is still
            // underfoot, so flat walking never moves). `on_ground` stays
            // true and `vel.y` stays 0 — the body never counted as falling.
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
        self.vel = dir * speed;
        self.pos += self.vel * dt;
        self.on_ground = false;
        self.jumping = false;
    }
}

/// Fraction of horizontal speed *retained* after one timestep `dt` of `friction`.
/// `friction` is the fraction shed in one [`FRICTION_REF_DT`] frame; raising the
/// retained fraction `1 - friction` to `dt / FRICTION_REF_DT` makes the decay
/// compose to the same amount per second at any frame rate or sub-step length.
/// Endpoints hold at every `dt`: friction 0 → retain 1 (velocity untouched —
/// momentum kept forever), friction 1 → retain 0 (an instant stop).
#[inline]
pub(super) fn friction_retain(friction: f32, dt: f32) -> f32 {
    // friction >= 1 is a full stop at any dt (also dodges the 0.powf(0) == 1
    // surprise should this ever be called with dt == 0).
    if friction >= 1.0 {
        0.0
    } else {
        (1.0 - friction).powf(dt / FRICTION_REF_DT)
    }
}

/// Move the 2-D point `(x, z)` toward `(tx, tz)` by at most `max_delta`, clamping
/// exactly onto the target when it is within reach. Never overshoots, so a
/// velocity ramped this way reaches top speed without blowing past it at any `dt`.
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

/// Move the scalar `v` toward `target` by at most `max_delta`, clamping onto the
/// target when within reach (never overshoots). The 1-D analogue of
/// [`move_toward`], used to ease vertical swim velocity toward its rise/sink goal.
#[inline]
pub(super) fn approach(v: f32, target: f32, max_delta: f32) -> f32 {
    let d = target - v;
    if d.abs() <= max_delta {
        target
    } else {
        v + d.signum() * max_delta
    }
}
