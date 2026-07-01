use super::collision::Axis;
use super::state::{Input, Player, HALF_W};
use crate::mathh::Vec3;
use crate::world::World;

pub(super) const WALK: f32 = 4.3;
pub(super) const SPRINT: f32 = 5.6;
pub(super) const SPECTATOR_SPEED: f32 = 48.0;
pub(super) const SPECTATOR_SPRINT: f32 = 96.0;
pub(super) const GRAVITY: f32 = 28.0;
/// Jump take-off speed. Apex height = v0² / (2·g) = 8.4²/56 ≈ 1.26 blocks, so a
/// held jump clears a single full block with margin.
const JUMP_V0: f32 = 8.4;
const TERMINAL: f32 = 30.0;
/// Horizontal friction on the ground — purely a decay rate, applied only when
/// there is no input: the fraction of the player's speed shed in one reference
/// frame (see [`friction_retain`]). Modest, so a body that lands or stops with
/// residual speed skids to a *gradual* halt (~0.7 m, ~0.5 s from walk speed)
/// rather than stopping dead — firmer than the air, but still a slide, not a snap.
pub(super) const GROUND_FRICTION: f32 = 0.2;
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
/// --- Swimming (in-water) constants. ---
/// Horizontal swim speed: about half walk, so you move through water much more
/// slowly than on land.
const SWIM_SPEED: f32 = 2.2;
/// Horizontal acceleration toward the swim wish velocity (m/s2). Lower than the
/// ground so swimming feels sluggish.
const SWIM_ACCEL: f32 = 16.0;
/// Horizontal drag while submerged with no input: the fraction of speed shed per
/// reference frame (heavy, so water stops you quickly when you let go).
const WATER_FRICTION: f32 = 0.30;
/// Target horizontal drift speed from flowing water. This is deliberately below
/// swim speed: currents should move an idle body but not take control away.
const WATER_CURRENT_SPEED: f32 = 0.75;
/// How quickly flowing water contributes its drift speed.
const WATER_CURRENT_ACCEL: f32 = 9.0;
/// Upward swim speed reached while holding the jump key underwater.
const SWIM_RISE: f32 = 3.0;
/// Gentle downward drift speed while submerged and not swimming up (buoyant, far
/// below the dry-land terminal velocity).
const SWIM_SINK: f32 = 1.4;
/// How fast vertical velocity eases toward the rise/sink target (m/s2): a soft
/// approach so falling into water decelerates smoothly instead of snapping.
const SWIM_VACCEL: f32 = 14.0;
/// Probe height above the feet for the "submerged enough to swim" test: once
/// water reaches roughly thigh height the player switches to swim physics, so
/// shallow wading still walks. Probing the body (not the eye) lets the head break
/// the surface and gravity resume, so you bob at the waterline.
pub(super) const WATER_PROBE_Y: f32 = 0.6;
/// Probe just above the feet so shallow flowing sheets can nudge a walking
/// player even when they are not deep enough to switch to swim physics.
const WADING_CURRENT_PROBE_Y: f32 = 0.05;
/// Upward boost given when swimming toward a 1-block ledge you can climb onto: a
/// bit below a full land jump (`JUMP_V0`) but well above the gentle swim rise, so
/// you crest the surface with enough speed to land on the block instead of bobbing
/// at its base. It engages while still submerged (see [`Player::ledge_ahead`]) so
/// the velocity carries you up through the waterline.
pub(super) const SWIM_CLIMB: f32 = 7.5;
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

impl Player {
    /// Is there a 1-block-high ledge to climb onto just ahead in `dir`? True when
    /// the cell in front (at the feet, or one above them) is solid with open space
    /// directly above it — i.e. a single step, not a taller wall. Used by the in-
    /// water climb-out assist: it returns true while the player is still ~1 block
    /// below the ledge top, so the upward boost fires before the head clears the
    /// surface and carries the player up onto the block. A genuine 2+ block wall
    /// (solid above too) is *not* a ledge, so swimming into a cliff face won't lift
    /// you up it.
    fn ledge_ahead<F: Fn(i32, i32, i32) -> bool>(&self, dir: Vec3, solid: &F) -> bool {
        let d = Vec3::new(dir.x, 0.0, dir.z);
        if d.length_squared() <= 1e-12 {
            return false;
        }
        let d = d.normalize();
        // A point just beyond the AABB face in the move direction.
        let fx = (self.pos.x + d.x * (HALF_W + 0.2)).floor() as i32;
        let fz = (self.pos.z + d.z * (HALF_W + 0.2)).floor() as i32;
        let base = self.pos.y.floor() as i32;
        // Step at feet level, or one block above the feet (so the boost engages
        // from roughly a block below the ledge top, giving runway to crest it).
        let step_at = |y: i32| solid(fx, y, fz) && !solid(fx, y + 1, fz);
        step_at(base) || step_at(base + 1)
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

    /// Advance the player by `dt` seconds against the world's solid voxels.
    /// The caller must ensure the overlapped columns are loaded (see
    /// [`Player::columns_loaded`]) before stepping survival physics. Spectator
    /// mode ignores world solidity and may move through unloaded columns.
    pub fn update(&mut self, dt: f32, world: &World, input: Input) {
        // Position-aware so a multi-cell bbmodel block collides per its own cell shape.
        let boxes = |x: i32, y: i32, z: i32| world.collision_boxes_at(x, y, z);
        let water = |x: i32, y: i32, z: i32| world.water_cell_at(x, y, z);
        let water_flow = |x: i32, y: i32, z: i32| world.water_flow_dir_at(x, y, z);
        self.update_core_with_current(dt, &boxes, &water, &water_flow, input);
    }

    /// Physics integration against arbitrary solidity + water predicates, so the
    /// feel can be unit-tested without a World. See [`Player::update`].
    #[cfg(test)]
    pub(super) fn update_core<F, W>(&mut self, dt: f32, solid: &F, water: &W, input: Input)
    where
        F: Fn(i32, i32, i32) -> bool,
        W: Fn(i32, i32, i32) -> bool,
    {
        let still_water = |_: i32, _: i32, _: i32| Vec3::ZERO;
        // Adapt the test's bool solidity to the general collision-box predicate: a
        // solid cell is one full cube, an empty cell no box.
        let boxes = |x: i32, y: i32, z: i32| {
            if solid(x, y, z) {
                crate::block::Block::Stone
            } else {
                crate::block::Block::Air
            }
            .collision_boxes()
        };
        self.update_core_with_current(dt, &boxes, water, &still_water, input);
    }

    pub(super) fn update_core_with_current<F, W, C>(
        &mut self,
        dt: f32,
        boxes: &F,
        water: &W,
        water_flow: &C,
        input: Input,
    ) where
        F: Fn(i32, i32, i32) -> &'static [crate::block::Aabb],
        W: Fn(i32, i32, i32) -> bool,
        C: Fn(i32, i32, i32) -> Vec3,
    {
        if self.is_spectator() {
            self.update_spectator(dt, input);
            return;
        }

        let was_on_ground = self.on_ground;
        // A cell counts as solid for the water climb-out ledge probe iff it has any
        // collision box (full cube, slab, chest, …).
        let solid = |x: i32, y: i32, z: i32| !boxes(x, y, z).is_empty();

        // Submerged enough to swim? Sample water ~thigh height above the feet, so
        // wading in shallow water still walks but deeper water switches to buoyant
        // swim physics. Probing the body (not the eye) means the head can break the
        // surface and gravity resumes -> you bob at the waterline.
        let water_x = self.pos.x.floor() as i32;
        let water_z = self.pos.z.floor() as i32;
        let swim_y = (self.pos.y + WATER_PROBE_Y).floor() as i32;
        let feet_y = (self.pos.y + WADING_CURRENT_PROBE_Y).floor() as i32;
        let in_water = water(water_x, swim_y, water_z);
        let flow_dir = if in_water {
            water_flow(water_x, swim_y, water_z)
        } else if water(water_x, feet_y, water_z) {
            water_flow(water_x, feet_y, water_z)
        } else {
            Vec3::ZERO
        };

        // --- Vertical. In water: ease toward a rise (holding jump) or a gentle
        // sink target -- buoyant and slow. On land: the original jump impulse +
        // gravity (eased near the apex). ---
        if in_water {
            // Climb-out assist: when the player explicitly jumps (Space) while
            // moving toward a low ledge they could get out onto (a 1-block step
            // just ahead with open space above it — see `ledge_ahead`) and is not
            // currently sinking, give a firm upward boost instead of the gentle
            // rise. It engages while still submerged, so you carry that speed
            // through the waterline and land on the block rather than bobbing at its
            // base. Mark it as a jump arc so gravity eases at the apex once you
            // surface, floating you the last bit onto the ledge.
            //   - Requiring jump keeps it an explicit action — wading through
            //     shallow/edge water toward shore never hops you out on its own.
            //   - Requiring vel.y >= 0 makes a *failed* hop behave: if you don't
            //     make the ledge and fall back in against the wall, your downward
            //     fall velocity is preserved (this branch is skipped, so the normal
            //     swim handling lets you sink) instead of being discarded by the
            //     `max` below and relaunching you instantly. You sink back down
            //     once — the harder you fell in, the deeper — before the boost can
            //     fire again.
            let climbing_out = input.jump
                && self.vel.y >= 0.0
                && input.wishdir.length_squared() > 1e-12
                && self.ledge_ahead(input.wishdir, &solid);
            if climbing_out {
                self.vel.y = self.vel.y.max(SWIM_CLIMB);
                self.jumping = true;
            } else {
                // Ease toward a rise (holding jump) or a gentle buoyant sink.
                let target = if input.jump { SWIM_RISE } else { -SWIM_SINK };
                self.vel.y = approach(self.vel.y, target, SWIM_VACCEL * dt);
                self.jumping = false;
            }
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
        let dy = self.vel.y * dt;
        let blocked_y = self.sweep_boxes(Axis::Y, dy, boxes);
        if blocked_y {
            // Landed if we were moving down; bonked head if moving up. Either way
            // the jump arc is over, so stop easing gravity.
            self.on_ground = dy < 0.0;
            self.vel.y = 0.0;
            self.jumping = false;
        } else {
            self.on_ground = false;
        }

        // --- Horizontal: input accelerates toward the wish velocity; friction
        // decays it. In water this is a slow swim with heavy drag; on land it is
        // the original ground/air handling. ---
        let speed = if input.sprint { SPRINT } else { WALK };
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
        if in_water {
            // Swim: accelerate toward the (slow) swim speed; heavy drag when idle.
            // Ground/air friction and the air speed-cap are bypassed — water has its
            // own feel (a sluggish ramp to a low top speed, then a quick stop).
            if wish.length_squared() <= 1e-12 {
                let retain = friction_retain(WATER_FRICTION, dt);
                self.vel.x *= retain;
                self.vel.z *= retain;
            } else {
                let (vx, vz) = move_toward(
                    self.vel.x,
                    self.vel.z,
                    wish.x * SWIM_SPEED,
                    wish.z * SWIM_SPEED,
                    SWIM_ACCEL * dt,
                );
                self.vel.x = vx;
                self.vel.z = vz;
            }
        } else if wish.length_squared() <= 1e-12 {
            // No input: friction is the only horizontal force. Keep the retained
            // fraction (1 - friction) per reference frame, rescaled to this dt so
            // the slowdown per second is the same at any frame rate or sub-step
            // length. friction 0 → retain 1 (coast forever); 1 → retain 0 (stop).
            let retain = friction_retain(
                if grounded {
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
            // here: speeding up is fully decoupled from it.
            let (vx, vz) = move_toward(
                self.vel.x,
                self.vel.z,
                wish.x * speed,
                wish.z * speed,
                GROUND_ACCEL * dt,
            );
            self.vel.x = vx;
            self.vel.z = vz;
        } else {
            // Air: additive acceleration along the wish direction only — it tops the
            // wish-direction speed up to `speed` but never brakes, so a jump keeps
            // the momentum it launched with. The total horizontal speed is then
            // capped at whatever we already had (or `speed` if slower): input can
            // *redirect* momentum but never *inflate* it. Without that cap, scraping
            // a wall pumps speed without bound — the wall zeroes the into-wall
            // velocity each step, keeping the wish-direction projection low so `add`
            // stays large, while the perpendicular (along-wall) speed climbs every
            // frame. The cap makes steering a constant-speed turn and kills that
            // exploit; friction (above) is the only thing that slows you.
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

        let (vx, vz) = add_flow_push(
            self.vel.x,
            self.vel.z,
            flow_dir,
            WATER_CURRENT_SPEED,
            WATER_CURRENT_ACCEL * dt,
        );
        self.vel.x = vx;
        self.vel.z = vz;

        let dx = self.vel.x * dt;
        let dz = self.vel.z * dt;
        // Horizontal slide with auto step-up: a grounded player walks up a half-block
        // ledge (a slab / a model block's low edge) without jumping. Airborne → step 0.
        // Same `collision::step_horizontal` the mob/item resolver uses.
        let step = if self.on_ground {
            crate::collision::STEP_HEIGHT
        } else {
            0.0
        };
        let mn = self.aabb_min();
        let mx = self.aabb_max();
        let (moved, hit_x, hit_z) = crate::collision::step_horizontal(
            [mn.x, mn.y, mn.z],
            [mx.x, mx.y, mx.z],
            dx,
            dz,
            step,
            boxes,
        );
        self.pos.x += moved[0];
        self.pos.y += moved[1];
        self.pos.z += moved[2];
        if hit_x {
            self.vel.x = 0.0;
        }
        if hit_z {
            self.vel.z = 0.0;
        }

        // Measure the fall now that `on_ground` and the final feet `y` are settled; the
        // tick turns a latched landing into damage (see `crate::game::health`).
        self.track_fall(was_on_ground, in_water);
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
fn move_toward(x: f32, z: f32, tx: f32, tz: f32, max_delta: f32) -> (f32, f32) {
    let (dx, dz) = (tx - x, tz - z);
    let dist_sq = dx * dx + dz * dz;
    if dist_sq <= max_delta * max_delta || dist_sq == 0.0 {
        (tx, tz)
    } else {
        let scale = max_delta / dist_sq.sqrt();
        (x + dx * scale, z + dz * scale)
    }
}

/// Add a capped push along a water-flow direction without slowing bodies that
/// already have at least that much velocity along the current.
#[inline]
fn add_flow_push(x: f32, z: f32, dir: Vec3, target_speed: f32, max_delta: f32) -> (f32, f32) {
    let len_sq = dir.x * dir.x + dir.z * dir.z;
    if len_sq <= 1e-12 || target_speed <= 0.0 || max_delta <= 0.0 {
        return (x, z);
    }
    let inv_len = len_sq.sqrt().recip();
    let nx = dir.x * inv_len;
    let nz = dir.z * inv_len;
    let along = x * nx + z * nz;
    let add = (target_speed - along).clamp(0.0, max_delta);
    (x + nx * add, z + nz * add)
}

/// Move the scalar `v` toward `target` by at most `max_delta`, clamping onto the
/// target when within reach (never overshoots). The 1-D analogue of
/// [`move_toward`], used to ease vertical swim velocity toward its rise/sink goal.
#[inline]
fn approach(v: f32, target: f32, max_delta: f32) -> f32 {
    let d = target - v;
    if d.abs() <= max_delta {
        target
    } else {
        v + d.signum() * max_delta
    }
}
