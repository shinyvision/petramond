//! First-person player: AABB physics with gravity/jump, swept voxel collision,
//! and a block raycast used for break/place.
//!
//! The player is a 0.6 × 1.8 × 0.6 box. `pos` is the *feet centre*: x/z are the
//! horizontal centre of the box and y is its bottom. The camera eye sits `EYE`
//! above the feet. Horizontal movement decouples acceleration from friction.
//! While a direction is held, the velocity ramps toward the wish velocity (input
//! direction × speed). On the ground this is a snappy redirect toward wish×speed
//! (responsive starts, stops, and turns); in the air it is a gentle, *additive*
//! nudge along the input direction that tops you up to walk speed but never
//! brakes, with total air speed capped at what you launched with — so a jump
//! keeps its momentum and input can steer the arc but neither brakes nor pumps it
//! up (no wall-scrape speed exploit). With no input, *friction* alone decays the velocity
//! toward zero: friction is purely how fast you slow down — 0 keeps motion
//! forever, 1 stops it instantly — and it never gates how fast you speed up.
//! Ground friction is high (quick stop), air friction low (a long coast). The
//! decay is frame-rate independent; the ramp's rate is too, though the exact
//! frame it reaches top speed can vary by up to one sub-step. Gravity pulls the
//! player down — eased near the jump apex for a softer arc — and Space jumps.
//! There is
//! no auto step-up: every block is a full unit cube, so a sub-block step would
//! never trigger and a full-block step would contradict the jump-to-climb feel
//! (`JUMP_V0` clears ~1.26 blocks, enough to step onto a 1-block ledge).

use crate::block::Block;
use crate::mathh::{IVec3, Vec3};
use crate::world::World;

/// Half the horizontal width (box is 0.6 wide on x and z).
pub const HALF_W: f32 = 0.3;
/// Full body height.
pub const HEIGHT: f32 = 1.8;
/// Eye height above the feet (matches Minecraft's 1.62).
pub const EYE: f32 = 1.62;
/// Max block-interaction distance, measured from the eye.
pub const REACH: f32 = 4.0;
/// Largest physics sub-step; `app` splits a frame's `dt` into chunks this size
/// so a long stall can't make one update step move (and tunnel) too far.
pub const DT_MAX: f32 = 0.05;

const WALK: f32 = 4.3;
const SPRINT: f32 = 5.6;
const GRAVITY: f32 = 28.0;
/// Jump take-off speed. Apex height = v0² / (2·g) = 8.4²/56 ≈ 1.26 blocks, so a
/// held jump clears a single full block with margin.
const JUMP_V0: f32 = 8.4;
const TERMINAL: f32 = 30.0;
/// Horizontal friction on the ground — purely a decay rate, applied only when
/// there is no input: the fraction of the player's speed shed in one reference
/// frame (see [`friction_retain`]). Modest, so a body that lands or stops with
/// residual speed skids to a *gradual* halt (~0.7 m, ~0.5 s from walk speed)
/// rather than stopping dead — firmer than the air, but still a slide, not a snap.
const GROUND_FRICTION: f32 = 0.2;
/// Horizontal friction in the air — the decay rate while coasting (no input).
/// Very low, so after a jump the player keeps almost all of its horizontal
/// momentum and drifts a long way before stopping (retains ~99 % per frame, so
/// roughly half the speed survives a full second of free coasting and it bleeds
/// to zero only very gradually). This and the gentle, additive air acceleration
/// are what let a jump carry its momentum.
const AIR_FRICTION: f32 = 0.05;
/// Horizontal acceleration on the ground (m/s²): how fast `move_toward` snaps the
/// velocity to the wish velocity while a direction is held. High, so the ground
/// feels snappy — top speed reached in a few frames, with crisp turns and stops.
/// Independent of friction, so top speed is exactly the walk/sprint speed.
const GROUND_ACCEL: f32 = 60.0;
/// Horizontal acceleration in the air (m/s²). Low, and applied *additively* along
/// the input direction only (never braking), so mid-air input merely nudges the
/// trajectory: you keep the momentum a jump launched you with and gently steer,
/// never snap to a new direction. The air counterpart to [`GROUND_ACCEL`].
const AIR_ACCEL: f32 = 20.0;
/// Reference timestep the friction fractions are calibrated to: at exactly this
/// `dt` the player sheds `friction` of its speed in one frame (ground 10 %, air
/// 1 %). [`friction_retain`] rescales to any other `dt` so the slowdown per
/// second is identical regardless of frame rate or sub-step length. 60 Hz.
const FRICTION_REF_DT: f32 = 1.0 / 60.0;
/// Apex easing band: within this |vel.y| (m/s) of the top of a jump, gravity is
/// scaled toward `APEX_GRAVITY`, rounding the up→down transition rather than
/// snapping through it.
const APEX_VY: f32 = 3.0;
/// Gravity multiplier at the exact apex (vel.y = 0), ramping linearly back to
/// 1.0 by `APEX_VY`. Slightly below 1 so the peak floats a touch; the band is
/// narrow enough that overall jump height barely changes.
const APEX_GRAVITY: f32 = 0.7;
/// Boundary epsilon: the AABB is shrunk by this on every side before its float
/// edges are turned into integer cell indices, so an edge flush on a voxel
/// boundary — or a hair off from float error — is *not* treated as occupying the
/// neighbouring cell. Applied symmetrically (see `lo`/`hi` in `sweep`) for a
/// consistent cell set per axis regardless of approach direction or world
/// position. (Past a few thousand blocks one f32 ULP exceeds EPS, so it stops
/// biting and collisions degrade — phantom blocks and tunnelling return; the
/// inherent limit of an f32 voxel world, reached only far outside normal play.)
const EPS: f32 = 1e-4;

/// Per-frame movement intent, in world space.
#[derive(Copy, Clone, Default)]
pub struct Input {
    /// Horizontal wish direction (unit length, or zero). Y is ignored.
    pub wishdir: Vec3,
    pub jump: bool,
    pub sprint: bool,
}

pub struct Player {
    /// Feet centre (see module docs).
    pub pos: Vec3,
    pub vel: Vec3,
    pub on_ground: bool,
    /// True between a jump take-off and the next blocked vertical sweep (landing
    /// or head-bonk). Gates the apex easing so only a genuine jump arc is
    /// softened — walking off a ledge or bonking a ceiling falls at full gravity.
    jumping: bool,
}

/// Result of a block raycast.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct RaycastHit {
    /// The solid cell the ray entered.
    pub block: IVec3,
    /// Face normal pointing back toward the eye. `block + normal` is the empty
    /// cell to place into. Zero when the eye started inside a solid block.
    pub normal: IVec3,
}

#[derive(Copy, Clone)]
enum Axis {
    X,
    Y,
    Z,
}

impl Player {
    pub fn new(feet: Vec3) -> Self {
        Self {
            pos: feet,
            vel: Vec3::ZERO,
            on_ground: false,
            jumping: false,
        }
    }

    /// Eye position (camera origin).
    #[inline]
    pub fn eye(&self) -> Vec3 {
        Vec3::new(self.pos.x, self.pos.y + EYE, self.pos.z)
    }

    /// AABB min corner.
    #[inline]
    fn aabb_min(&self) -> Vec3 {
        Vec3::new(self.pos.x - HALF_W, self.pos.y, self.pos.z - HALF_W)
    }
    /// AABB max corner.
    #[inline]
    fn aabb_max(&self) -> Vec3 {
        Vec3::new(
            self.pos.x + HALF_W,
            self.pos.y + HEIGHT,
            self.pos.z + HALF_W,
        )
    }

    #[inline]
    fn solid_world(world: &World, x: i32, y: i32, z: i32) -> bool {
        Block::from_id(world.chunk_block(x, y, z)).is_solid()
    }

    /// True if every chunk column the horizontal AABB overlaps is loaded. The
    /// caller gates physics on this (once per frame) so the player can't fall
    /// through terrain that hasn't generated yet (spawn, or running past the
    /// load frontier). Column membership can't change within a frame, so this
    /// need not be re-checked per sub-step.
    pub fn columns_loaded(&self, world: &World) -> bool {
        let cx0 = (self.pos.x - HALF_W).floor() as i32 >> 4;
        let cx1 = (self.pos.x + HALF_W).floor() as i32 >> 4;
        let cz0 = (self.pos.z - HALF_W).floor() as i32 >> 4;
        let cz1 = (self.pos.z + HALF_W).floor() as i32 >> 4;
        for cx in cx0..=cx1 {
            for cz in cz0..=cz1 {
                if !world.chunk_loaded(cx, cz) {
                    return false;
                }
            }
        }
        true
    }

    /// Advance the player by `dt` seconds against the world's solid voxels.
    /// The caller must ensure the overlapped columns are loaded (see
    /// [`Player::columns_loaded`]) before stepping physics.
    pub fn update(&mut self, dt: f32, world: &World, input: Input) {
        let solid = |x: i32, y: i32, z: i32| Self::solid_world(world, x, y, z);
        self.update_core(dt, &solid, input);
    }

    /// Physics integration against an arbitrary solidity predicate, so the feel
    /// can be unit-tested without a `World`. See [`Player::update`].
    fn update_core<F: Fn(i32, i32, i32) -> bool>(&mut self, dt: f32, solid: &F, input: Input) {
        let was_on_ground = self.on_ground;

        // --- Vertical: jump impulse, then gravity (eased near the jump apex). ---
        if input.jump && was_on_ground {
            self.vel.y = JUMP_V0;
            self.jumping = true;
        }
        // Soften the apex of a jump: scale gravity down as the vertical speed
        // approaches zero, easing back to full gravity by |vel.y| = APEX_VY.
        // Velocity stays continuous, so the up→down switch reads as a gentle arc,
        // not a corner. Only while `jumping`, so walk-offs and ceiling bonks
        // (where vel.y is also briefly small) keep full gravity.
        let g = if self.jumping {
            let t = (self.vel.y.abs() / APEX_VY).min(1.0); // 0 at apex → 1 outside
            GRAVITY * (APEX_GRAVITY + (1.0 - APEX_GRAVITY) * t)
        } else {
            GRAVITY
        };
        self.vel.y = (self.vel.y - g * dt).max(-TERMINAL);
        let dy = self.vel.y * dt;
        let blocked_y = self.sweep(Axis::Y, dy, solid);
        if blocked_y {
            // Landed if we were moving down; bonked head if moving up. Either
            // way the jump arc is over, so stop easing gravity.
            self.on_ground = dy < 0.0;
            self.vel.y = 0.0;
            self.jumping = false;
        } else {
            self.on_ground = false;
        }

        // --- Horizontal: input accelerates toward the wish velocity; friction
        // decays it. The two are decoupled — acceleration is how fast you reach
        // and steer toward top speed, friction is purely how fast you slow down
        // once you let go (0 = coast forever, 1 = stop instantly). ---
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
        if wish.length_squared() <= 1e-12 {
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
            // Ground: snap toward the wish velocity at the high ground
            // acceleration — responsive starts, stops, and reversals, with no
            // stray momentum (move_toward redirects the whole velocity vector, so
            // turning leaves no leftover speed on the axis you stopped steering).
            // Friction is not read here: speeding up is fully decoupled from it.
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
            // Air: additive acceleration along the wish direction only — it tops
            // the wish-direction speed up to `speed` but never brakes, so a jump
            // keeps the momentum it launched with. The total horizontal speed is
            // then capped at whatever we already had (or `speed` if slower): input
            // can *redirect* momentum but never *inflate* it. Without that cap,
            // scraping a wall pumps speed without bound — the wall zeroes the
            // into-wall velocity each step, keeping the wish-direction projection
            // low so `add` stays large, while the perpendicular (along-wall) speed
            // climbs every frame. The cap makes steering a constant-speed turn and
            // kills that exploit; friction (above) is the only thing that slows you.
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

        let dx = self.vel.x * dt;
        let dz = self.vel.z * dt;
        if self.sweep(Axis::X, dx, solid) {
            self.vel.x = 0.0;
        }
        if self.sweep(Axis::Z, dz, solid) {
            self.vel.z = 0.0;
        }
    }

    /// Move along one axis by `delta`, stopping at the first solid voxel slice
    /// the AABB would enter. Scans *every* cell slice swept (nearest first), so
    /// it never tunnels regardless of `delta`. Returns true if a block was hit.
    fn sweep<F: Fn(i32, i32, i32) -> bool>(&mut self, axis: Axis, delta: f32, solid: &F) -> bool {
        if delta == 0.0 {
            return false;
        }
        let min = self.aabb_min();
        let max = self.aabb_max();
        // Cell index of a min edge (inclusive) and a max edge (exclusive-ish).
        // Both shrink the box by EPS so an edge sitting on a voxel boundary — or
        // a hair off it from float error (e.g. 1.3 - 0.3 = 0.99999994) — yields a
        // consistent cell set: a flush min edge does not pull in the cell below,
        // and a flush max edge does not pull in the cell above.
        let lo = |a: f32| (a + EPS).floor() as i32;
        let hi = |b: f32| (b - EPS).floor() as i32;

        match axis {
            Axis::X => {
                let (a0, a1) = (lo(min.y), hi(max.y));
                let (b0, b1) = (lo(min.z), hi(max.z));
                if delta > 0.0 {
                    let to = hi(max.x + delta);
                    for c in (hi(max.x) + 1)..=to {
                        if Self::slice_solid(solid, Axis::X, c, a0, a1, b0, b1) {
                            self.pos.x = c as f32 - HALF_W;
                            return true;
                        }
                    }
                } else {
                    let to = lo(min.x + delta);
                    for c in ((to)..=(lo(min.x) - 1)).rev() {
                        if Self::slice_solid(solid, Axis::X, c, a0, a1, b0, b1) {
                            self.pos.x = (c + 1) as f32 + HALF_W;
                            return true;
                        }
                    }
                }
                self.pos.x += delta;
                false
            }
            Axis::Z => {
                let (a0, a1) = (lo(min.x), hi(max.x));
                let (b0, b1) = (lo(min.y), hi(max.y));
                if delta > 0.0 {
                    let to = hi(max.z + delta);
                    for c in (hi(max.z) + 1)..=to {
                        if Self::slice_solid(solid, Axis::Z, c, a0, a1, b0, b1) {
                            self.pos.z = c as f32 - HALF_W;
                            return true;
                        }
                    }
                } else {
                    let to = lo(min.z + delta);
                    for c in ((to)..=(lo(min.z) - 1)).rev() {
                        if Self::slice_solid(solid, Axis::Z, c, a0, a1, b0, b1) {
                            self.pos.z = (c + 1) as f32 + HALF_W;
                            return true;
                        }
                    }
                }
                self.pos.z += delta;
                false
            }
            Axis::Y => {
                let (a0, a1) = (lo(min.x), hi(max.x));
                let (b0, b1) = (lo(min.z), hi(max.z));
                if delta > 0.0 {
                    let to = hi(max.y + delta);
                    for c in (hi(max.y) + 1)..=to {
                        if Self::slice_solid(solid, Axis::Y, c, a0, a1, b0, b1) {
                            self.pos.y = c as f32 - HEIGHT;
                            return true;
                        }
                    }
                } else {
                    let to = lo(min.y + delta);
                    for c in ((to)..=(lo(min.y) - 1)).rev() {
                        if Self::slice_solid(solid, Axis::Y, c, a0, a1, b0, b1) {
                            self.pos.y = (c + 1) as f32;
                            return true;
                        }
                    }
                }
                self.pos.y += delta;
                false
            }
        }
    }

    /// Is any voxel in the AABB's cross-section at slice `c` (along `axis`)
    /// solid? `(a0,a1)` and `(b0,b1)` are the inclusive cell ranges of the two
    /// fixed axes (order: X→(Y,Z), Z→(X,Y), Y→(X,Z)).
    #[inline]
    fn slice_solid<F: Fn(i32, i32, i32) -> bool>(
        solid: &F,
        axis: Axis,
        c: i32,
        a0: i32,
        a1: i32,
        b0: i32,
        b1: i32,
    ) -> bool {
        for a in a0..=a1 {
            for b in b0..=b1 {
                let hit = match axis {
                    Axis::X => solid(c, a, b),
                    Axis::Z => solid(a, b, c),
                    Axis::Y => solid(a, c, b),
                };
                if hit {
                    return true;
                }
            }
        }
        false
    }

    /// Does the player's AABB overlap the unit cube at integer cell `b`? The box
    /// is shrunk by `EPS` on every side — the same cell set [`Player::sweep`]
    /// resolves against — so a block merely flush against the player (touching a
    /// face, exactly or a hair off from float error) does *not* count. Keeping
    /// this in lock-step with `sweep` matters: it gates block placement, so you
    /// can place a block in exactly the cells the collision sweep lets you stand
    /// beside (no "can't place where I clearly fit", no "placed inside myself").
    pub fn intersects_block(&self, b: IVec3) -> bool {
        let min = self.aabb_min();
        let max = self.aabb_max();
        let (bx, by, bz) = (b.x as f32, b.y as f32, b.z as f32);
        min.x + EPS < bx + 1.0
            && max.x - EPS > bx
            && min.y + EPS < by + 1.0
            && max.y - EPS > by
            && min.z + EPS < bz + 1.0
            && max.z - EPS > bz
    }

    /// Cast a ray from `eye` along (assumed-normalised) `dir`, returning the
    /// first solid block within `REACH`. Voxel DDA (Amanatides & Woo).
    pub fn raycast(eye: Vec3, dir: Vec3, world: &World) -> Option<RaycastHit> {
        Self::raycast_core(eye, dir, &|x, y, z| Self::solid_world(world, x, y, z))
    }

    fn raycast_core<F: Fn(i32, i32, i32) -> bool>(
        eye: Vec3,
        dir: Vec3,
        solid: &F,
    ) -> Option<RaycastHit> {
        let mut ix = eye.x.floor() as i32;
        let mut iy = eye.y.floor() as i32;
        let mut iz = eye.z.floor() as i32;

        // Pre-guard: DDA is undefined when the origin is already inside a voxel.
        if solid(ix, iy, iz) {
            return Some(RaycastHit {
                block: IVec3::new(ix, iy, iz),
                normal: IVec3::ZERO,
            });
        }

        let step = IVec3::new(sign(dir.x), sign(dir.y), sign(dir.z));
        let t_delta = Vec3::new(inv_abs(dir.x), inv_abs(dir.y), inv_abs(dir.z));
        let mut t_max = Vec3::new(
            boundary_t(eye.x, dir.x),
            boundary_t(eye.y, dir.y),
            boundary_t(eye.z, dir.z),
        );

        loop {
            // Advance across the nearest voxel boundary.
            let (axis, t) = if t_max.x <= t_max.y && t_max.x <= t_max.z {
                (0, t_max.x)
            } else if t_max.y <= t_max.z {
                (1, t_max.y)
            } else {
                (2, t_max.z)
            };
            if t > REACH {
                return None;
            }
            let mut normal = IVec3::ZERO;
            match axis {
                0 => {
                    ix += step.x;
                    t_max.x += t_delta.x;
                    normal.x = -step.x;
                }
                1 => {
                    iy += step.y;
                    t_max.y += t_delta.y;
                    normal.y = -step.y;
                }
                _ => {
                    iz += step.z;
                    t_max.z += t_delta.z;
                    normal.z = -step.z;
                }
            }
            if solid(ix, iy, iz) {
                return Some(RaycastHit {
                    block: IVec3::new(ix, iy, iz),
                    normal,
                });
            }
        }
    }
}

/// Fraction of horizontal speed *retained* after one timestep `dt` of `friction`.
/// `friction` is the fraction shed in one [`FRICTION_REF_DT`] frame; raising the
/// retained fraction `1 - friction` to `dt / FRICTION_REF_DT` makes the decay
/// compose to the same amount per second at any frame rate or sub-step length.
/// Endpoints hold at every `dt`: friction 0 → retain 1 (velocity untouched —
/// momentum kept forever), friction 1 → retain 0 (an instant stop).
#[inline]
fn friction_retain(friction: f32, dt: f32) -> f32 {
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

#[inline]
fn sign(v: f32) -> i32 {
    if v > 0.0 {
        1
    } else if v < 0.0 {
        -1
    } else {
        0
    }
}

/// 1/|v|, or +∞ when v is zero (that axis is never crossed).
#[inline]
fn inv_abs(v: f32) -> f32 {
    if v == 0.0 {
        f32::INFINITY
    } else {
        (1.0 / v).abs()
    }
}

/// Distance along the ray from `p` to the first voxel boundary in direction `d`.
#[inline]
fn boundary_t(p: f32, d: f32) -> f32 {
    if d == 0.0 {
        return f32::INFINITY;
    }
    let cell = p.floor();
    if d > 0.0 {
        (cell + 1.0 - p) / d
    } else {
        (p - cell) / -d
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(feet: Vec3) -> Player {
        Player::new(feet)
    }

    #[test]
    fn falls_and_lands_on_floor() {
        // Solid everywhere y < 64 (a thick floor), air above.
        let solid = |_x: i32, y: i32, _z: i32| y < 64;
        let mut pl = p(Vec3::new(0.0, 70.0, 0.0));
        // Large downward sweep: must clamp feet to the top of cell 63 (y=64).
        let blocked = pl.sweep(Axis::Y, -20.0, &solid);
        assert!(blocked);
        assert_eq!(pl.pos.y, 64.0);
    }

    #[test]
    fn does_not_tunnel_through_one_block_floor() {
        // Only y == 0 is solid (a 1-block-thick platform).
        let solid = |_x: i32, y: i32, _z: i32| y == 0;
        let mut pl = p(Vec3::new(0.0, 5.0, 0.0));
        let blocked = pl.sweep(Axis::Y, -20.0, &solid);
        assert!(blocked, "must not fall through a 1-thick floor");
        assert_eq!(pl.pos.y, 1.0, "feet rest on top of cell 0");
    }

    #[test]
    fn stops_at_wall_moving_positive_x() {
        // Wall at x >= 5.
        let solid = |x: i32, _y: i32, _z: i32| x >= 5;
        let mut pl = p(Vec3::new(4.0, 64.0, 0.0)); // max.x = 4.3
        let blocked = pl.sweep(Axis::X, 2.0, &solid);
        assert!(blocked);
        // max.x clamped to 5.0 => centre at 4.7.
        assert!((pl.pos.x - 4.7).abs() < 1e-5, "pos.x = {}", pl.pos.x);
    }

    #[test]
    fn stops_at_wall_moving_negative_x() {
        // Wall at x <= 1 (cells 1 and below solid).
        let solid = |x: i32, _y: i32, _z: i32| x <= 1;
        let mut pl = p(Vec3::new(4.0, 64.0, 0.0)); // min.x = 3.7
        let blocked = pl.sweep(Axis::X, -3.0, &solid);
        assert!(blocked);
        // min.x clamped to 2.0 (top of cell 1) => centre at 2.3.
        assert!((pl.pos.x - 2.3).abs() < 1e-5, "pos.x = {}", pl.pos.x);
    }

    #[test]
    fn moves_freely_in_open_air() {
        let solid = |_x: i32, _y: i32, _z: i32| false;
        let mut pl = p(Vec3::new(0.0, 64.0, 0.0));
        assert!(!pl.sweep(Axis::Z, 3.0, &solid));
        assert_eq!(pl.pos.z, 3.0);
    }

    #[test]
    fn air_decays_slower_than_ground() {
        // No input: both decay gradually toward zero, but air friction is far
        // weaker than ground friction, so in a single frame the airborne body
        // sheds only a sliver of its speed while the grounded body sheds a larger
        // share — both a slide, ground just firmer.
        let dt = FRICTION_REF_DT; // at the reference frame, retain == 1 - friction
        let open = |_x: i32, _y: i32, _z: i32| false;
        let mut air = p(Vec3::new(0.0, 128.0, 0.0));
        air.vel = Vec3::new(WALK, 5.0, 0.0); // gliding +x, rising
        air.on_ground = false;
        air.update_core(dt, &open, Input::default());

        let floor = |_x: i32, y: i32, _z: i32| y < 64;
        let mut gnd = p(Vec3::new(0.0, 64.0, 0.0));
        gnd.vel = Vec3::new(WALK, 0.0, 0.0);
        gnd.on_ground = true;
        gnd.update_core(dt, &floor, Input::default());

        // Air retains 1 - AIR_FRICTION of its speed; ground retains less per frame.
        assert!(
            (air.vel.x - WALK * (1.0 - AIR_FRICTION)).abs() < 1e-5,
            "air vx = {}",
            air.vel.x
        );
        assert!(
            (gnd.vel.x - WALK * (1.0 - GROUND_FRICTION)).abs() < 1e-5,
            "gnd vx = {}",
            gnd.vel.x
        );
        assert!(
            air.vel.x > gnd.vel.x,
            "air should keep more momentum than ground"
        );
        assert!(air.vel.y < 5.0, "gravity should bleed upward speed");
    }

    #[test]
    fn ground_accelerates_faster_than_air() {
        let input = Input {
            wishdir: Vec3::new(1.0, 0.0, 0.0),
            jump: false,
            sprint: false,
        };
        let dt = FRICTION_REF_DT;

        // On the ground from rest, one step ramps toward walk speed at the high
        // ground acceleration (GROUND_ACCEL·dt, still well below WALK so it is not
        // yet clamped) — a few frames to top speed, so the ground feels snappy.
        let floor = |_x: i32, y: i32, _z: i32| y < 64;
        let mut g = p(Vec3::new(0.0, 64.0, 0.0));
        g.on_ground = true;
        g.update_core(dt, &floor, input);
        assert!(
            (g.vel.x - GROUND_ACCEL * dt).abs() < 1e-5,
            "ground vx = {}",
            g.vel.x
        );

        // In the air from rest, the same input ramps far more slowly — gentle
        // steering, not a snap to speed.
        let open = |_x: i32, _y: i32, _z: i32| false;
        let mut a = p(Vec3::new(0.0, 128.0, 0.0));
        a.on_ground = false;
        a.update_core(dt, &open, input);
        assert!(
            (a.vel.x - AIR_ACCEL * dt).abs() < 1e-5,
            "air vx = {}",
            a.vel.x
        );
        assert!(
            g.vel.x > a.vel.x * 2.0,
            "ground acceleration much stronger than air"
        );
    }

    #[test]
    fn air_input_does_not_brake_momentum() {
        // Airborne at sprint speed, then holding plain forward (a *slower* walk
        // wish). Air acceleration is additive — it only adds toward the wish
        // direction, never brakes — so the launched momentum is kept, not bled
        // down to walk speed. (Releasing input instead lets air friction coast it
        // down very gradually; that path is covered elsewhere.)
        let open = |_x: i32, _y: i32, _z: i32| false;
        let input = Input {
            wishdir: Vec3::new(1.0, 0.0, 0.0),
            jump: false,
            sprint: false,
        };
        let mut a = p(Vec3::new(0.0, 128.0, 0.0));
        a.on_ground = false;
        a.vel = Vec3::new(SPRINT, 0.0, 0.0); // gliding +x faster than WALK
        a.update_core(FRICTION_REF_DT, &open, input);
        assert!(
            (a.vel.x - SPRINT).abs() < 1e-5,
            "air input must not brake momentum, vx = {}",
            a.vel.x
        );
    }

    #[test]
    fn air_steering_redirects_without_inflating_speed() {
        // Airborne moving +x at walk speed; steering +z rotates the velocity
        // toward +z at constant total speed — momentum is redirected, not pumped
        // (forward bleeds a hair as lateral is added). This speed cap is what stops
        // wall-scraping from building crazy sideways speed.
        let open = |_x: i32, _y: i32, _z: i32| false;
        let input = Input {
            wishdir: Vec3::new(0.0, 0.0, 1.0),
            jump: false,
            sprint: false,
        };
        let mut a = p(Vec3::new(0.0, 128.0, 0.0));
        a.on_ground = false;
        a.vel = Vec3::new(WALK, 0.0, 0.0);
        a.update_core(FRICTION_REF_DT, &open, input);
        let speed = (a.vel.x * a.vel.x + a.vel.z * a.vel.z).sqrt();
        assert!(
            (speed - WALK).abs() < 1e-4,
            "total speed preserved, not inflated, got {speed}"
        );
        assert!(a.vel.z > 0.0, "lateral input adds +z, vz = {}", a.vel.z);
        assert!(
            a.vel.x < WALK,
            "forward bleeds slightly as speed redirects, vx = {}",
            a.vel.x
        );
    }

    #[test]
    fn jumping_into_wall_does_not_pump_sideways_speed() {
        // Wall just ahead in +x; hold a wish mostly *into* the wall, slightly along
        // it. The into-wall velocity is killed by the wall every step, which used to
        // let the perpendicular (+z) speed climb without bound. With the air speed
        // cap, total horizontal speed stays bounded by walk speed no matter how
        // long you scrape the wall.
        let wall_x = 6;
        let solid = move |x: i32, _y: i32, _z: i32| x >= wall_x;
        let mut a = p(Vec3::new(wall_x as f32 - 1.0, 128.0, 0.0));
        a.on_ground = false; // open below: stays airborne the whole run
        let wishdir = Vec3::new(0.98, 0.0, 0.2).normalize();
        let input = Input {
            wishdir,
            jump: false,
            sprint: false,
        };
        for _ in 0..600 {
            a.update_core(0.02, &solid, input);
        }
        let speed = (a.vel.x * a.vel.x + a.vel.z * a.vel.z).sqrt();
        assert!(
            speed <= WALK + 1e-3,
            "wall-scrape pumped speed to {speed} (cap is WALK = {WALK})"
        );
    }

    #[test]
    fn air_out_coasts_ground() {
        // No input: both decay by friction alone. Air friction is far weaker than
        // ground friction, so after the same coast the airborne body retains
        // strictly — and, with the tuned values, far — more speed. Expectations are
        // derived from the constants, so this survives retuning either friction (it
        // only assumes the design invariant AIR_FRICTION < GROUND_FRICTION).
        let open = |_x: i32, _y: i32, _z: i32| false;
        let floor = |_x: i32, y: i32, _z: i32| y < 64;
        let mut air = p(Vec3::new(0.0, 1024.0, 0.0)); // open below: airborne the whole run
        air.on_ground = false;
        air.vel = Vec3::new(WALK, 0.0, 0.0);
        let mut gnd = p(Vec3::new(0.0, 64.0, 0.0));
        gnd.on_ground = true;
        gnd.vel = Vec3::new(WALK, 0.0, 0.0);
        let steps = 30; // ~half a second at the reference step
        for _ in 0..steps {
            air.update_core(FRICTION_REF_DT, &open, Input::default());
            gnd.update_core(FRICTION_REF_DT, &floor, Input::default());
        }
        // Pure-decay speeds implied by the friction constants (one ref step retains
        // exactly 1 - friction).
        let air_expected = WALK * (1.0 - AIR_FRICTION).powi(steps);
        let gnd_expected = WALK * (1.0 - GROUND_FRICTION).powi(steps);
        assert!(
            (air.vel.x - air_expected).abs() < 1e-3,
            "air vx = {} (want {air_expected})",
            air.vel.x
        );
        assert!(
            (gnd.vel.x - gnd_expected).abs() < 1e-3,
            "gnd vx = {} (want {gnd_expected})",
            gnd.vel.x
        );
        assert!(
            air.vel.x > gnd.vel.x,
            "air must out-coast ground: air {} vs gnd {}",
            air.vel.x,
            gnd.vel.x
        );
    }

    #[test]
    fn friction_endpoints_hold_at_any_dt() {
        for &dt in &[0.005f32, FRICTION_REF_DT, 0.05] {
            // friction 0: nothing shed, motion continues indefinitely.
            assert_eq!(
                friction_retain(0.0, dt),
                1.0,
                "friction 0 must not decay (dt={dt})"
            );
            // friction 1: everything shed, an immediate stop.
            assert_eq!(
                friction_retain(1.0, dt),
                0.0,
                "friction 1 must snap to a stop (dt={dt})"
            );
        }
        // At the reference frame the retained fraction is exactly 1 - friction.
        assert!(
            (friction_retain(GROUND_FRICTION, FRICTION_REF_DT) - (1.0 - GROUND_FRICTION)).abs()
                < 1e-6
        );
        assert!(
            (friction_retain(AIR_FRICTION, FRICTION_REF_DT) - (1.0 - AIR_FRICTION)).abs() < 1e-6
        );
    }

    #[test]
    fn friction_is_framerate_independent() {
        // One big decay step must retain the same fraction as several small steps
        // spanning the same wall-clock time (the property the sub-step loop relies on).
        let total = 0.05f32;
        let one = friction_retain(GROUND_FRICTION, total);
        let n = 5;
        let many = friction_retain(GROUND_FRICTION, total / n as f32).powi(n);
        assert!(
            (one - many).abs() < 1e-6,
            "retained {one} (1 step) vs {many} ({n} steps)"
        );
    }

    #[test]
    fn gravity_eases_near_apex() {
        let open = |_x: i32, _y: i32, _z: i32| false;
        // In a jump, inside the apex band: reduced gravity loses less speed.
        let mut near = p(Vec3::new(0.0, 128.0, 0.0));
        near.vel = Vec3::new(0.0, 1.0, 0.0);
        near.on_ground = false;
        near.jumping = true;
        near.update_core(0.05, &open, Input::default());
        let near_drop = 1.0 - near.vel.y;

        // In a jump, outside the band: full gravity.
        let mut fast = p(Vec3::new(0.0, 128.0, 0.0));
        fast.vel = Vec3::new(0.0, 20.0, 0.0);
        fast.on_ground = false;
        fast.jumping = true;
        fast.update_core(0.05, &open, Input::default());
        let fast_drop = 20.0 - fast.vel.y;

        assert!(
            near_drop < fast_drop,
            "apex should ease gravity: {near_drop} vs {fast_drop}"
        );
        assert!(
            (fast_drop - GRAVITY * 0.05).abs() < 1e-5,
            "outside band is full gravity"
        );
    }

    #[test]
    fn no_apex_easing_when_not_jumping() {
        // Walking off a ledge / stepping down (jumping == false) must fall at
        // full gravity even though vel.y is briefly inside the apex band — the
        // easing is reserved for real jump arcs, so the world never feels floaty.
        let open = |_x: i32, _y: i32, _z: i32| false;
        let mut pl = p(Vec3::new(0.0, 128.0, 0.0));
        pl.vel = Vec3::new(0.0, 1.0, 0.0); // small downward-bound speed, no jump
        pl.on_ground = false;
        pl.jumping = false;
        pl.update_core(0.05, &open, Input::default());
        let drop = 1.0 - pl.vel.y;
        assert!(
            (drop - GRAVITY * 0.05).abs() < 1e-5,
            "not jumping → full gravity, got {drop}"
        );
    }

    /// Trusted, slow reference: does the player AABB centred at `pos` overlap any
    /// solid cell? Shrinks the box by a symmetric tol on every side (so it is
    /// direction-agnostic by construction — any asymmetry in `sweep` shows up as
    /// a disagreement with this).
    fn ref_overlaps<F: Fn(i32, i32, i32) -> bool>(pos: Vec3, solid: &F) -> bool {
        let t = 1e-4;
        let x0 = (pos.x - HALF_W + t).floor() as i32;
        let x1 = (pos.x + HALF_W - t).floor() as i32;
        let y0 = (pos.y + t).floor() as i32;
        let y1 = (pos.y + HEIGHT - t).floor() as i32;
        let z0 = (pos.z - HALF_W + t).floor() as i32;
        let z1 = (pos.z + HALF_W - t).floor() as i32;
        for x in x0..=x1 {
            for y in y0..=y1 {
                for z in z0..=z1 {
                    if solid(x, y, z) {
                        return true;
                    }
                }
            }
        }
        false
    }

    /// Reference separated-axis move (X then Z, like `sweep`) advancing in
    /// ~0.5 mm micro-steps and stopping before the first overlap. Moves *exactly*
    /// `disp` in open space (the final sub-step takes up the remainder, so there
    /// is no rounding drift). Obviously correct; the slow oracle for `sweep`.
    fn ref_move<F: Fn(i32, i32, i32) -> bool>(mut pos: Vec3, disp: Vec3, solid: &F) -> Vec3 {
        let step = 5e-4f32;
        for axis in [0, 1] {
            let d = if axis == 0 { disp.x } else { disp.z };
            let mut moved = 0.0f32;
            while moved < d.abs() {
                let this = step.min(d.abs() - moved) * d.signum();
                let mut next = pos;
                if axis == 0 {
                    next.x += this;
                } else {
                    next.z += this;
                }
                if ref_overlaps(next, solid) {
                    break;
                }
                pos = next;
                moved += this.abs();
            }
        }
        pos
    }

    #[test]
    fn sweep_matches_reference_from_all_directions() {
        let configs: [(&str, &[IVec3]); 6] = [
            ("single", &[IVec3::new(10, 64, 10)]),
            (
                "wall_x",
                &[
                    IVec3::new(10, 64, 8),
                    IVec3::new(10, 64, 9),
                    IVec3::new(10, 64, 10),
                    IVec3::new(10, 64, 11),
                    IVec3::new(10, 64, 12),
                ],
            ),
            (
                "wall_z",
                &[
                    IVec3::new(8, 64, 10),
                    IVec3::new(9, 64, 10),
                    IVec3::new(10, 64, 10),
                    IVec3::new(11, 64, 10),
                    IVec3::new(12, 64, 10),
                ],
            ),
            ("pillar2", &[IVec3::new(10, 64, 10), IVec3::new(10, 65, 10)]),
            ("head", &[IVec3::new(10, 65, 10)]),
            (
                "Lcorner",
                &[
                    IVec3::new(10, 64, 10),
                    IVec3::new(11, 64, 10),
                    IVec3::new(10, 64, 11),
                ],
            ),
        ];
        let dirs: [(f32, f32, &str); 8] = [
            (1.0, 0.0, "+X"),
            (-1.0, 0.0, "-X"),
            (0.0, 1.0, "+Z"),
            (0.0, -1.0, "-Z"),
            (1.0, 1.0, "+X+Z"),
            (1.0, -1.0, "+X-Z"),
            (-1.0, 1.0, "-X+Z"),
            (-1.0, -1.0, "-X-Z"),
        ];
        // Translate the whole scene to probe positive, origin-crossing, and
        // negative coordinates (floor()/cast/>>4 behave differently around 0).
        let bases: [(i32, i32, &str); 3] = [(0, 0, "pos"), (-10, -10, "origin"), (-21, -21, "neg")];
        let mut failures = Vec::new();
        for (bx, bz, bname) in bases {
            for (cname0, cells0) in configs {
                let cells_v: Vec<IVec3> = cells0
                    .iter()
                    .map(|c| IVec3::new(c.x + bx, c.y, c.z + bz))
                    .collect();
                let cname = format!("{bname}/{cname0}");
                let solid = {
                    let cells_v = cells_v.clone();
                    move |x: i32, y: i32, z: i32| {
                        y < 64 || cells_v.iter().any(|c| c.x == x && c.y == y && c.z == z)
                    }
                };
                let centre = Vec3::new(10.5 + bx as f32, 64.0, 10.5 + bz as f32);
                for (dx, dz, name) in dirs {
                    let len = (dx * dx + dz * dz).sqrt();
                    let wishdir = Vec3::new(dx / len, 0.0, dz / len);
                    let lateral = Vec3::new(-wishdir.z, 0.0, wishdir.x);
                    for k in -19..=19 {
                        let off = k as f32 * 0.05;
                        let start = centre - wishdir * 3.5 + lateral * off;
                        let (dt, speed) = (0.02f32, WALK);
                        // sweep path. Start at full walk speed so the friction
                        // ramp-up doesn't lag the reference mover (which moves at
                        // exactly speed·dt from step one); this test probes the
                        // collision sweep, not the acceleration curve.
                        let mut pl = p(start);
                        pl.on_ground = true;
                        pl.vel = wishdir * WALK;
                        let input = Input {
                            wishdir,
                            jump: false,
                            sprint: false,
                        };
                        // reference path (kept at floor height, like the grounded body)
                        let mut rpos = start;
                        for _ in 0..150 {
                            pl.update_core(dt, &solid, input);
                            rpos = ref_move(rpos, wishdir * (speed * dt), &solid);
                        }
                        let d = ((pl.pos.x - rpos.x).powi(2) + (pl.pos.z - rpos.z).powi(2)).sqrt();
                        // Cardinals must track the reference tightly (the property the
                        // float-boundary bug broke: phantom/pass-through collisions).
                        // Diagonals slide along walls, where the two integrators round
                        // a corner up to one sub-step apart — allow that discretisation.
                        let tol = if dx == 0.0 || dz == 0.0 { 0.02 } else { 0.12 };
                        if d > tol {
                            failures.push(format!(
                            "{cname} {name} off={off:+.2}: sweep=({:.3},{:.3}) ref=({:.3},{:.3}) d={d:.3}",
                            pl.pos.x, pl.pos.z, rpos.x, rpos.z));
                        }
                    }
                }
            }
        }
        assert!(
            failures.is_empty(),
            "{} mismatches:\n{}",
            failures.len(),
            failures
                .iter()
                .take(40)
                .cloned()
                .collect::<Vec<_>>()
                .join("\n")
        );
    }

    #[test]
    fn raycast_hits_block_ahead_with_back_normal() {
        // Single solid block at (4, 64, 0): entered at x=4.0, i.e. 3.5 from the
        // eye — within REACH (4.0). (A block at x=5 would be 4.5 away → a miss.)
        let solid = |x: i32, y: i32, z: i32| x == 4 && y == 64 && z == 0;
        // Eye centred in cell (0,64,0) looking +x.
        let eye = Vec3::new(0.5, 64.5, 0.5);
        let hit = Player::raycast_core(eye, Vec3::new(1.0, 0.0, 0.0), &solid).unwrap();
        assert_eq!(hit.block, IVec3::new(4, 64, 0));
        assert_eq!(hit.normal, IVec3::new(-1, 0, 0)); // face toward the eye
    }

    #[test]
    fn raycast_out_of_reach_misses() {
        let solid = |x: i32, _y: i32, _z: i32| x == 100;
        let eye = Vec3::new(0.5, 64.5, 0.5);
        assert!(Player::raycast_core(eye, Vec3::new(1.0, 0.0, 0.0), &solid).is_none());
    }

    #[test]
    fn raycast_eye_inside_solid_returns_zero_normal() {
        let solid = |_x: i32, _y: i32, _z: i32| true;
        let eye = Vec3::new(0.5, 64.5, 0.5);
        let hit = Player::raycast_core(eye, Vec3::new(1.0, 0.0, 0.0), &solid).unwrap();
        assert_eq!(hit.normal, IVec3::ZERO);
    }

    #[test]
    fn intersects_block_consistent_with_sweep_when_flush() {
        // Standing flush against a wall on the -X side: a -X resolve leaves the
        // min edge on the integer boundary, which float renders as 0.99999994.
        // `sweep` (lo = floor(min+EPS)) treats the cell beside you as free; the
        // place-gate must agree, or you can't build into a cell you clearly fit
        // next to.
        let pl = p(Vec3::new(1.3, 64.0, 0.5)); // min.x = 1.3 - 0.3 = 0.99999994
        assert!(
            pl.aabb_min().x < 1.0,
            "precondition: float pulls min.x below 1.0"
        );
        assert!(
            !pl.intersects_block(IVec3::new(0, 64, 0)),
            "flush-beside cell must read as free, matching sweep"
        );
        // And the cell the body actually stands in still counts.
        assert!(pl.intersects_block(IVec3::new(1, 64, 0)));
    }

    #[test]
    fn intersects_block_strict_faces() {
        let pl = p(Vec3::new(0.5, 64.0, 0.5));
        // The cell the feet stand in overlaps.
        assert!(pl.intersects_block(IVec3::new(0, 64, 0)));
        // A block flush against +x face (player max.x = 0.8 < 1.0) does not.
        assert!(!pl.intersects_block(IVec3::new(1, 64, 0)));
        // A block at head height overlaps (player spans y in [64, 65.8]).
        assert!(pl.intersects_block(IVec3::new(0, 65, 0)));
        // Above the head does not.
        assert!(!pl.intersects_block(IVec3::new(0, 66, 0)));
    }
}
