//! Instance body kinematics: the per-tick locomotion integration (knockback
//! stagger > mod drive > brain wish precedence, water current, buoyancy,
//! gravity, shared swept-AABB collision), the solid-peer motion commit, the
//! soft entity push, and fall/splash bookkeeping.

use std::f32::consts::{PI, TAU};

use crate::mathh::{voxel_at, IVec3, Vec3};

use super::instance::Instance;
use super::MobDef;

/// Downward acceleration (m/s²) applied to airborne mobs.
const GRAVITY: f32 = -22.0;
/// Per-tick decay of the horizontal knockback velocity during the stagger.
const KNOCKBACK_DAMP: f32 = 0.75;
/// Upward swim speed a mob eases toward whenever its body is under water. A mob has no
/// jump key, so it always swims up — exactly like a player holding jump in water.
/// Mirrors the player's `SWIM_RISE`: the mob rises, breaches the surface (the probe
/// clears the water), gravity then pulls it back, it re-enters and rises again —
/// bobbing through the waterline.
const SWIM_RISE: f32 = 3.0;
/// How fast vertical velocity eases toward the swim target (m/s²) — a soft approach
/// (mirrors the player's `SWIM_VACCEL`) so falling into water decelerates smoothly and
/// the bob rocks instead of snapping.
const SWIM_VACCEL: f32 = 14.0;
/// Fraction of body height at which the "submerged enough to swim" probe sits (≈ the
/// player's thigh-height probe). The mob keeps swimming up until this point clears the
/// water, so its body breaks the surface before gravity takes back over.
const SWIM_PROBE_FRAC: f32 = 1.0 / 3.0;
/// Firm upward boost (m/s) a swimming mob gets when steering toward a 1-block ledge it
/// can climb onto — enough to crest the waterline and land on the block instead of
/// hugging its base forever. Mirrors the player's `SWIM_CLIMB`.
const SWIM_CLIMB: f32 = 4.5;
/// Highest ledge top (metres above current feet) that the swim-climb boost treats as
/// reachable. A ledge much above the current waterline is a wall until the mob swims up.
const SWIM_CLIMB_MAX_LEDGE_DELTA: f32 = 1.25;
/// Target horizontal drift speed (m/s) imparted by flowing water — matched to the
/// player's so a mob and the player ride the same current at the same pace. Below walk
/// speed, so a current carries an idle mob but never overpowers a mob that's swimming.
const WATER_CURRENT_SPEED: f32 = 0.75;
/// How far below the waterline a surface-floating body's feet settle
/// (`Buoyancy::Surface`) — a hull rides with its keel wetted, not skimming.
const SURFACE_DRAFT: f32 = 0.1;
/// First-order approach rate (per second) toward the float line for
/// `Buoyancy::Surface`: velocity proportional to the depth error (capped at
/// [`SWIM_RISE`]), so a hull settles level with no overshoot and NO bob.
const SURFACE_FLOAT_RATE: f32 = 6.0;

/// One tick's mod-issued locomotion: a horizontal velocity (m/s, world space)
/// and optionally an absolute facing. See [`Instance::set_drive`].
#[derive(Copy, Clone)]
pub(super) struct DriveIntent {
    pub vel_x: f32,
    pub vel_z: f32,
    pub yaw: Option<f32>,
}

impl Instance {
    pub(super) fn take_fall_distance(&mut self) -> Option<f32> {
        let distance = std::mem::replace(&mut self.fall_distance, 0.0);
        (distance > 0.0).then_some(distance)
    }

    pub(super) fn take_splash_drop(&mut self) -> Option<f32> {
        let drop = std::mem::replace(&mut self.splash_drop, 0.0);
        (drop > 0.0).then_some(drop)
    }

    /// Latch a mod's locomotion intent for this tick (see [`DriveIntent`] and
    /// the consumption in [`integrate_with_flow`](Self::integrate_with_flow)).
    /// Refused on a dead mob.
    pub(super) fn set_drive(&mut self, vel_x: f32, vel_z: f32, yaw: Option<f32>) -> bool {
        if self.death.is_dead() {
            return false;
        }
        self.drive = Some(DriveIntent { vel_x, vel_z, yaw });
        true
    }

    /// Discard an unconsumed per-tick drive intent. Frozen/skipped mobs call
    /// this explicitly because they never reach locomotion integration.
    #[inline]
    pub(super) fn clear_drive(&mut self) {
        self.drive = None;
    }

    #[cfg(test)]
    pub(super) fn drive_pending(&self) -> bool {
        self.drive.is_some()
    }

    /// Current velocity (m/s) — read-only; mods steer through
    /// [`set_drive`](Self::set_drive), never by writing velocity directly.
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
    /// feet position. Water breaks falls by re-anchoring the peak while submerged.
    pub(super) fn finish_motion(&mut self, was_on_ground: bool, in_water: bool) {
        if in_water {
            // The un-latched drop at the first wet tick is the fall INTO the
            // water; while swimming the per-tick re-anchor keeps it near zero
            // (the splash threshold filters the bobbing).
            let drop = self.fall_peak_y - self.pos.y;
            if drop > 0.0 {
                self.splash_drop = self.splash_drop.max(drop);
            }
            self.fall_peak_y = self.pos.y;
        } else if self.on_ground {
            if !was_on_ground {
                let dist = self.fall_peak_y - self.pos.y;
                if dist > self.fall_distance {
                    self.fall_distance = dist;
                }
            }
            self.fall_peak_y = self.pos.y;
        } else {
            self.fall_peak_y = self.fall_peak_y.max(self.pos.y);
        }
    }

    /// Integrate one tick's kinematics: jump impulse, horizontal wish-velocity, water
    /// current, gravity, collision, and facing/anim. Takes `solid`/`water`/`water_flow`
    /// closures (not the world) so it's directly unit-testable against a stub. While
    /// unsupported and falling, path steering is suspended and existing horizontal
    /// velocity carries through the fall; the upward phase of a navigation jump keeps
    /// steering so the mob can clear a one-block ledge. The mob faces its **wish**
    /// direction — where it wants to go — so it keeps facing forward even when pressed
    /// against a wall (where its actual velocity would be zero). Returns the mandatory
    /// shallow-foot healing lift separately for the peer-motion proposal.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn integrate_with_flow(
        &mut self,
        dt: f32,
        d: &MobDef,
        wish: Vec3,
        jump: bool,
        can_steer: bool,
        boxes: &impl Fn(i32, i32, i32) -> &'static [crate::block::Aabb],
        obstacles: &[crate::collision::DynBox],
        healing_obstacles: &[crate::collision::DynBox],
        solid: &impl Fn(IVec3) -> bool,
        water: &impl Fn(IVec3) -> bool,
        water_surface: &impl Fn(IVec3) -> Option<f32>,
        water_flow: &impl Fn(Vec3) -> Vec3,
    ) -> f32 {
        if jump && self.on_ground {
            self.vel.y = d.jump_speed;
            self.on_ground = false;
        }
        // During the knockback stagger the decaying knockback drives horizontal motion
        // (so a hit shoves the mob even against where it wants to go); otherwise a
        // mod's drive intent (a vehicle) or the wish velocity drives locomotion.
        // Keeping knockback separate from `vel` is why these overwrites can't wipe it.
        // The drive is consumed even when stagger owns the tick — like the wish, it is
        // a this-tick intent, never a queue.
        let drive = self.drive.take();
        let mut requested_yaw = None;
        if self.stagger_timer > 0.0 {
            self.vel.x = self.knockback.x;
            self.vel.z = self.knockback.z;
            self.knockback *= KNOCKBACK_DAMP;
            self.moving = false;
        } else if let Some(drive) = drive {
            // A driven mob is deliberately not `moving`: drive is not a walk —
            // no walk animation, no footstep noise, no wish-facing. Gated on
            // `can_steer` like the wish so a driven body has no more air or
            // stagger control than a walking one. Long-body yaw is clamped by
            // the same segmented geometry that resolves its translation.
            if can_steer {
                self.vel.x = drive.vel_x;
                self.vel.z = drive.vel_z;
                requested_yaw = drive.yaw;
            }
            self.moving = false;
        } else {
            if can_steer {
                self.vel.x = wish.x * d.walk_speed;
                self.vel.z = wish.z * d.walk_speed;
                self.moving = wish.length_squared() > 1e-6;
                if self.moving {
                    let target = heading_yaw(wish);
                    requested_yaw = Some(turn_toward(self.yaw, target, d.turn_rate * dt));
                }
            } else {
                self.moving = false;
            }
        }
        if let Some(yaw) = requested_yaw {
            self.yaw =
                super::clamp_body_yaw(self.pos, self.yaw, yaw, d.size, boxes, obstacles, self.id);
        }
        let preserve_air_carry = self.stagger_timer <= 0.0 && !can_steer;
        let carried_x = self.vel.x;
        let carried_z = self.vel.z;

        // Soft entity push: a velocity from being jostled by overlapping entities,
        // layered on top of locomotion (or knockback) so a crowded mob drifts apart
        // smoothly. Consumed each tick — the push pass re-derives it from the live
        // overlap — and left out of `moving`, so being shoved doesn't read as walking.
        self.vel.x += self.push.x;
        self.vel.z += self.push.z;
        self.push = Vec3::ZERO;

        // Water current: while standing in or swimming through flowing water, drift with
        // it — capped well below walk speed — so a mob caught in a river is carried
        // downstream instead of ignoring the flow. Unlike the player (whose velocity
        // carries momentum and eases into the current over several ticks), a mob rebuilds
        // its horizontal velocity from `wish` every tick, so the current contributes its
        // full drift in one tick (max step = the target speed) rather than a small accel
        // step that would never accumulate. It still never slows a mob already swimming
        // downstream faster than the current.
        let flow = flow_at_body(self.pos, d.size.height, water_flow);
        self.vel = add_flow_push(self.vel, flow, WATER_CURRENT_SPEED, WATER_CURRENT_SPEED);

        // Vertical, by the species' water behavior (`Buoyancy`):
        //
        // SURFACE (a hull): level off AT the waterline — velocity proportional
        // to the depth error toward `surface − draft`, capped at swim speed.
        // First-order, so it settles flat with NO overshoot and no bob, and a
        // submerged hull rises smoothly. No ledge-climb boost: a hull noses
        // against the shore instead of hopping onto it.
        //
        // SWIM (a creature): always stroke toward the surface (no jump key, so
        // it behaves like a player holding jump): vel eases up to `SWIM_RISE`
        // until the probe — a fraction up the body — clears the water; then
        // it's airborne, gravity pulls it back, it re-enters, and rises again.
        // The result is a bob through the waterline, identical in feel to the
        // player.
        //
        // Out of water either way: gravity.
        let feet = voxel_at(self.pos);
        if d.buoyancy == super::Buoyancy::Surface {
            let surface = water_surface(feet).or_else(|| water_surface(feet - IVec3::Y));
            self.vel.y = surface_vertical_velocity(self.vel.y, self.pos.y, surface, dt);
        } else {
            let probe = voxel_at(self.pos + Vec3::new(0.0, d.size.height * SWIM_PROBE_FRAC, 0.0));
            if water(probe) {
                // Climbing out: when steering toward a 1-block ledge it can get onto (and
                // not already falling back), a firm boost crests the waterline and lands it
                // on the block instead of hugging the shore forever — else the swim bob.
                let climbing_out = self.vel.y >= 0.0
                    && can_steer
                    && wish.length_squared() > 1e-12
                    && self.ledge_ahead(wish, d.size.half_width, solid);
                if climbing_out {
                    self.vel.y = self.vel.y.max(SWIM_CLIMB);
                } else {
                    self.vel.y = approach(self.vel.y, SWIM_RISE, SWIM_VACCEL * dt);
                }
            } else {
                self.vel.y += GRAVITY * dt;
            }
        }
        // Body collision via the shared swept-AABB resolver (the same one the player and
        // dropped items use) against the block's REAL collision shape — so a mob stops at a
        // bbmodel block's legs/top, not its full cube. Navigation (foothold/pathfinding/
        // `ledge_ahead`) stays cell-based (`solid`): that's "is this cell an obstacle", a
        // separate concern from "does my body hit the shape".
        // A grounded mob auto-steps up a half-block ledge (a slab / a model block's low
        // edge) without jumping — same `STEP_HEIGHT` the player uses.
        let (moved, grounded, hit, healed) = super::resolve_body_motion(
            self.pos,
            self.yaw,
            d.size,
            self.vel.to_array(),
            dt,
            crate::collision::STEP_HEIGHT,
            boxes,
            obstacles,
            healing_obstacles,
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
        if preserve_air_carry {
            if !hit[0] {
                self.vel.x = carried_x;
            }
            if !hit[2] {
                self.vel.z = carried_z;
            }
        }
        self.on_ground = grounded;
        if grounded && self.vel.y < 0.0 {
            self.vel.y = 0.0;
        }
        healed
    }

    /// [`integrate_with_flow`](Self::integrate_with_flow) in still water — the unit tests
    /// drive the kinematics against a stub world with no currents.
    #[cfg(test)]
    pub(super) fn integrate(
        &mut self,
        dt: f32,
        d: &MobDef,
        wish: Vec3,
        jump: bool,
        solid: &impl Fn(IVec3) -> bool,
        water: &impl Fn(IVec3) -> bool,
    ) {
        // Surface height derived from the stubbed water cells: topmost
        // contiguous cell + the source top (8/9) — engine species are all
        // swimmers, so only the Surface-buoyancy tests consult it.
        let water_surface = |c: IVec3| {
            if !water(c) {
                return None;
            }
            let mut top = c;
            while water(top + IVec3::Y) {
                top += IVec3::Y;
            }
            Some(top.y as f32 + 8.0 / 9.0)
        };
        self.integrate_with_flow(
            dt,
            d,
            wish,
            jump,
            true,
            &boxes_of(solid),
            &[],
            &[],
            solid,
            water,
            &water_surface,
            &|_| Vec3::ZERO,
        );
    }

    /// Move along each axis in turn, resolving against solid cells; returns whether
    /// the mob is resting on the ground after the move. Mirrors the dropped-item
    /// integrator, sized to the mob's AABB.
    /// Is there a 1-block ledge to climb onto just ahead in `dir` (horizontal)? True
    /// when the cell just beyond the body is solid at the feet (or one above) with open
    /// space directly above it — a single step, not a taller wall (so swimming into a
    /// cliff face won't lift the mob up it). Mirrors the player's climb-out probe.
    fn ledge_ahead(&self, dir: Vec3, half_width: f32, solid: &impl Fn(IVec3) -> bool) -> bool {
        let d = Vec3::new(dir.x, 0.0, dir.z);
        if d.length_squared() <= 1e-12 {
            return false;
        }
        let d = d.normalize_or_zero();
        // A cell just beyond the body's footprint in the move direction.
        let fx = (self.pos.x + d.x * (half_width + 0.2)).floor() as i32;
        let fz = (self.pos.z + d.z * (half_width + 0.2)).floor() as i32;
        let base = self.pos.y.floor() as i32;
        // A step at feet level, or one block above (so the boost engages from ~a block
        // below the ledge top, giving runway to crest it).
        let step_at = |y: i32| {
            let top = (y + 1) as f32;
            top <= self.pos.y + SWIM_CLIMB_MAX_LEDGE_DELTA
                && solid(IVec3::new(fx, y, fz))
                && !solid(IVec3::new(fx, y + 1, fz))
        };
        step_at(base) || step_at(base + 1)
    }

    #[cfg(test)]
    pub(crate) fn on_ground(&self) -> bool {
        self.on_ground
    }
}

/// The yaw that faces the horizontal component of `v`. The model faces `-Z` at
/// `yaw = 0` (the renderer applies `rotation_y(yaw)`), so heading `(vx, vz)` maps to
/// `atan2(-vx, -vz)`.
fn heading_yaw(v: Vec3) -> f32 {
    (-v.x).atan2(-v.z)
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

fn surface_vertical_velocity(current: f32, feet_y: f32, surface: Option<f32>, dt: f32) -> f32 {
    match surface {
        Some(surface) => {
            let target = surface - SURFACE_DRAFT;
            ((target - feet_y) * SURFACE_FLOAT_RATE).clamp(-SWIM_RISE, SWIM_RISE)
        }
        None => current + GRAVITY * dt,
    }
}

/// Move `cur` toward `target` by at most `step` (linear, no wrapping).
pub(super) fn approach(cur: f32, target: f32, step: f32) -> f32 {
    cur + (target - cur).clamp(-step, step)
}

pub(super) fn route_steering_supported(
    on_ground: bool,
    in_water: bool,
    vertical_velocity: f32,
) -> bool {
    on_ground || in_water || vertical_velocity > 0.0
}

/// The water-flow direction acting on a mob whose feet are at `pos`: the current at the
/// swim probe (a fraction up the body, where the mob is submerged enough to swim), else
/// the current at the feet (so a mob wading in a shallow flowing film is still nudged),
/// else zero when no water touches it. Probes are POINTS, surface-height aware (see
/// `World::water_flow_at_point`) — feet standing on a lowered block beside a channel,
/// above the fluid's real surface, catch nothing.
fn flow_at_body(pos: Vec3, height: f32, water_flow: &impl Fn(Vec3) -> Vec3) -> Vec3 {
    let f = water_flow(pos + Vec3::new(0.0, height * SWIM_PROBE_FRAC, 0.0));
    if f.length_squared() > 0.0 {
        return f;
    }
    water_flow(pos)
}

/// Add a capped push along the water-flow direction `dir` without slowing a body that
/// already drifts at least `target_speed` along it. Mirrors the player's and dropped
/// item's current handling, so every entity rides a current the same way. Horizontal
/// only — `vel.y` is untouched.
fn add_flow_push(vel: Vec3, dir: Vec3, target_speed: f32, max_delta: f32) -> Vec3 {
    let len_sq = dir.x * dir.x + dir.z * dir.z;
    if len_sq <= 1e-12 || target_speed <= 0.0 || max_delta <= 0.0 {
        return vel;
    }
    let inv_len = len_sq.sqrt().recip();
    let nx = dir.x * inv_len;
    let nz = dir.z * inv_len;
    let along = vel.x * nx + vel.z * nz;
    let add = (target_speed - along).clamp(0.0, max_delta);
    Vec3::new(vel.x + nx * add, vel.y, vel.z + nz * add)
}

/// Bridge a cell-solid bool stub into the shared collision box source (a full cube per
/// solid cell), so the kinematics tests keep driving body physics with a simple `solid`
/// predicate while it routes through the same `collision::resolve_body` as production.
#[cfg(test)]
fn boxes_of(
    solid: &impl Fn(IVec3) -> bool,
) -> impl Fn(i32, i32, i32) -> &'static [crate::block::Aabb] + '_ {
    move |x, y, z| {
        if solid(IVec3::new(x, y, z)) {
            crate::block::Block::Stone.collision_boxes()
        } else {
            &[]
        }
    }
}

#[cfg(test)]
mod tests;
