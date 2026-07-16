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

pub(super) fn route_steering_supported(on_ground: bool, in_water: bool, vertical_velocity: f32) -> bool {
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
mod tests {
    use super::*;
    use crate::block::Block;
    use crate::mob::{def, Mob, MobDamageFeedback};

    fn floor_at_zero(p: IVec3) -> bool {
        p.y < 0
    }

    fn owl_def() -> &'static MobDef {
        def(Mob::Owl)
    }

    fn default_feedback() -> MobDamageFeedback {
        MobDamageFeedback::default()
    }

    fn sheep_def() -> &'static MobDef {
        def(Mob::Sheep)
    }

    #[test]
    fn gravity_settles_the_mob_on_the_floor() {
        let mut owl = Instance::new(Mob::Owl, Vec3::new(0.5, 5.0, 0.5), 0.0, 1);
        for _ in 0..600 {
            owl.integrate(
                1.0 / 60.0,
                owl_def(),
                Vec3::ZERO,
                false,
                &floor_at_zero,
                &|_| false,
            );
        }
        assert!(
            owl.pos.y >= -1e-3,
            "mob fell through the floor: {}",
            owl.pos.y
        );
        assert!(owl.pos.y < 0.05, "mob rests on the floor: {}", owl.pos.y);
        assert!(owl.on_ground());
    }

    #[test]
    fn mob_body_rests_on_an_inset_block_top_not_the_cell_top() {
        // Model-aware body collision: a mob settling onto an INSET block (a chest, top at
        // 14/16) rests its feet on that real top, not the full-cube cell top (y = 1). The
        // mob body now collides through the shared `collision_boxes_at` shape (nav stays
        // cell-based, but that's a separate concern).
        let chest = crate::block::Block::Chest.collision_boxes();
        let chest_top = chest.iter().map(|b| b.max[1]).fold(0.0, f32::max);
        assert!(
            chest_top < 1.0,
            "the chest box must be inset (top {chest_top})"
        );
        let boxes = |_x: i32, y: i32, _z: i32| if y == 0 { chest } else { &[][..] };
        let solid = |c: IVec3| c.y == 0; // nav sees the chest cell as a unit obstacle
        let dry = |_: IVec3| false;
        let still = |_: Vec3| Vec3::ZERO;
        let mut owl = Instance::new(Mob::Owl, Vec3::new(0.5, 5.0, 0.5), 0.0, 1);
        for _ in 0..600 {
            owl.integrate_with_flow(
                1.0 / 60.0,
                owl_def(),
                Vec3::ZERO,
                false,
                true,
                &boxes,
                &[],
                &[],
                &solid,
                &dry,
                &|_| None,
                &still,
            );
        }
        assert!(owl.on_ground(), "mob should be grounded on the chest");
        assert!(
            (owl.pos.y - chest_top).abs() < 0.02,
            "mob feet should rest on the chest top {chest_top}, got {}",
            owl.pos.y
        );
    }

    #[test]
    fn grounded_mob_auto_steps_up_a_half_block() {
        // A grounded mob walking into a 0.5-tall ledge auto-climbs it (same STEP_HEIGHT as
        // the player), without needing a jump.
        let half_step = |x: i32, y: i32, _z: i32| -> &'static [crate::block::Aabb] {
            if y == 0 {
                Block::Stone.collision_boxes()
            } else if y == 1 && x >= 1 {
                &[crate::block::Aabb {
                    min: [0.0, 0.0, 0.0],
                    max: [1.0, 0.5, 1.0],
                }]
            } else {
                &[]
            }
        };
        let solid = |c: IVec3| c.y == 0 || (c.y == 1 && c.x >= 1); // nav obstacle
        let dry = |_: IVec3| false;
        let still = |_: Vec3| Vec3::ZERO;
        let wish = Vec3::new(1.0, 0.0, 0.0);
        let mut owl = Instance::new(Mob::Owl, Vec3::new(0.5, 1.0, 0.5), 0.0, 1);
        for _ in 0..180 {
            owl.integrate_with_flow(
                1.0 / 60.0,
                owl_def(),
                wish,
                false,
                true,
                &half_step,
                &[],
                &[],
                &solid,
                &dry,
                &|_| None,
                &still,
            );
        }
        assert!(owl.pos.x > 1.2, "mob steps onto the ledge: x={}", owl.pos.x);
        assert!(
            owl.pos.y > 1.4,
            "mob rises onto the 0.5 ledge top: y={}",
            owl.pos.y
        );
    }

    #[test]
    fn navigation_jump_keeps_steering_until_it_clears_a_full_block_step() {
        // A one-block navigation jump has an airborne phase where the body is still below
        // the ledge top and colliding with the block side. The mob must keep applying the
        // current route wish while rising, otherwise that side hit zeros horizontal
        // velocity and the jump stalls at the face.
        let solid = |c: IVec3| c.y < 1 || (c.x >= 1 && c.y < 2);
        let dry = |_: IVec3| false;
        let still = |_: Vec3| Vec3::ZERO;
        let wish = Vec3::new(1.0, 0.0, 0.0);
        let mut sheep = Instance::new(Mob::Sheep, Vec3::new(0.5, 1.0, 0.5), 0.0, 1);

        sheep.integrate_with_flow(
            0.05,
            sheep_def(),
            Vec3::ZERO,
            false,
            true,
            &boxes_of(&solid),
            &[],
            &[],
            &solid,
            &dry,
            &|_| None,
            &still,
        );
        assert!(sheep.on_ground(), "test starts from the lower floor");

        let mut left_ground = false;
        for _ in 0..80 {
            let can_steer = route_steering_supported(sheep.on_ground, false, sheep.vel.y);
            let jump = sheep.on_ground && sheep.pos.y < 1.5;
            sheep.integrate_with_flow(
                0.05,
                sheep_def(),
                wish,
                jump,
                can_steer,
                &boxes_of(&solid),
                &[],
                &[],
                &solid,
                &dry,
                &|_| None,
                &still,
            );
            left_ground |= !sheep.on_ground();
            if sheep.on_ground() && sheep.pos.y > 1.9 {
                break;
            }
        }

        assert!(left_ground, "the mob actually performed an airborne jump");
        assert!(
            sheep.on_ground() && sheep.pos.y > 1.9,
            "mob should land on the one-block step, pos {:?}",
            sheep.pos
        );
        assert!(
            sheep.pos.x + sheep_def().size.half_width > 1.0,
            "mob footprint should cross onto the step, pos {:?}",
            sheep.pos
        );
    }

    #[test]
    fn wish_direction_drives_horizontal_motion_and_facing() {
        let mut owl = Instance::new(Mob::Owl, Vec3::new(0.5, 0.0, 0.5), 0.0, 1);
        // Settle on the ground first.
        owl.integrate(
            1.0 / 60.0,
            owl_def(),
            Vec3::ZERO,
            false,
            &floor_at_zero,
            &|_| false,
        );
        let x0 = owl.pos.x;
        for _ in 0..30 {
            owl.integrate(
                1.0 / 60.0,
                owl_def(),
                Vec3::new(1.0, 0.0, 0.0),
                false,
                &floor_at_zero,
                &|_| false,
            );
        }
        assert!(
            owl.pos.x > x0 + 0.3,
            "wish +X should move the mob: {} -> {}",
            x0,
            owl.pos.x
        );
        assert!(owl.moving, "moving flag set while walking");
        // Faces +X: heading_yaw((+,0,0)) = atan2(-1, 0) = -PI/2.
        assert!(
            (wrap_angle(owl.yaw - (-PI / 2.0))).abs() < 0.2,
            "turns to face travel: {}",
            owl.yaw
        );
    }

    #[test]
    fn airborne_sheep_carries_velocity_without_walk_steering() {
        let empty_boxes = |_x: i32, _y: i32, _z: i32| -> &'static [crate::block::Aabb] { &[] };
        let dry = |_: IVec3| false;
        let still = |_: Vec3| Vec3::ZERO;
        let mut sheep = Instance::new(Mob::Sheep, Vec3::new(0.5, 5.0, 0.5), 0.0, 1);
        sheep.vel.x = 1.0;

        sheep.integrate_with_flow(
            1.0 / 60.0,
            sheep_def(),
            Vec3::new(-1.0, 0.0, 0.0),
            false,
            false,
            &empty_boxes,
            &[],
            &[],
            &dry,
            &dry,
            &|_| None,
            &still,
        );

        assert!(
            sheep.pos.x > 0.5,
            "falling should carry prior +X velocity instead of steering left: x {}",
            sheep.pos.x
        );
        assert!(
            sheep.vel.x > 0.0,
            "airborne walk wish must not overwrite carried velocity: vx {}",
            sheep.vel.x
        );
        assert!(
            !sheep.moving,
            "unsupported falling should not play the walk animation"
        );
    }

    #[test]
    fn an_airborne_drive_cannot_replace_carry_or_yaw() {
        let empty_boxes = |_x: i32, _y: i32, _z: i32| -> &'static [crate::block::Aabb] { &[] };
        let dry = |_: IVec3| false;
        let still = |_: Vec3| Vec3::ZERO;
        let mut owl = Instance::new(Mob::Owl, Vec3::new(0.5, 5.0, 0.5), 0.25, 1);
        owl.vel.x = 1.0;
        assert!(owl.set_drive(-5.0, 0.0, Some(1.5)));

        owl.integrate_with_flow(
            1.0 / 20.0,
            owl_def(),
            Vec3::ZERO,
            false,
            false,
            &empty_boxes,
            &[],
            &[],
            &dry,
            &dry,
            &|_| None,
            &still,
        );

        assert!(owl.pos.x > 0.5, "airborne carry wins over driven -X");
        assert_eq!(owl.yaw, 0.25, "airborne drive yaw is ignored too");
        assert!(owl.drive.is_none(), "the rejected intent still expires");
    }

    #[test]
    fn jump_impulse_lifts_a_grounded_mob() {
        let mut owl = Instance::new(Mob::Owl, Vec3::new(0.5, 0.0, 0.5), 0.0, 1);
        owl.integrate(
            1.0 / 60.0,
            owl_def(),
            Vec3::ZERO,
            false,
            &floor_at_zero,
            &|_| false,
        );
        assert!(owl.on_ground());
        owl.integrate(
            1.0 / 60.0,
            owl_def(),
            Vec3::ZERO,
            true,
            &floor_at_zero,
            &|_| false,
        );
        assert!(!owl.on_ground(), "jump leaves the ground");
        assert!(owl.pos.y > 0.0, "jump raises the mob");
    }

    #[test]
    fn idle_mob_is_not_moving() {
        let mut owl = Instance::new(Mob::Owl, Vec3::new(0.5, 0.0, 0.5), 0.0, 1);
        for _ in 0..10 {
            owl.integrate(
                1.0 / 60.0,
                owl_def(),
                Vec3::ZERO,
                false,
                &floor_at_zero,
                &|_| false,
            );
        }
        assert!(
            !owl.moving,
            "a still mob reports not moving (renders the rest pose)"
        );
    }

    #[test]
    fn a_drive_intent_moves_the_mob_for_one_tick_then_expires() {
        // A mod's kinematic drive replaces the wish overwrite for exactly the
        // tick it was issued: the mob moves at the driven velocity with its
        // yaw set, does not read as walking, and — like the brain's wish —
        // the intent must be re-issued or the next tick's overwrite parks it.
        let mut owl = Instance::new(Mob::Owl, Vec3::new(0.5, 0.0, 0.5), 0.0, 1);
        assert!(owl.set_drive(2.0, 0.0, Some(1.0)));
        owl.integrate(
            1.0 / 20.0,
            owl_def(),
            Vec3::ZERO,
            false,
            &floor_at_zero,
            &|_| false,
        );
        assert!(owl.pos.x > 0.5, "the drive velocity moved the mob");
        assert!(
            (owl.yaw - 1.0).abs() < 1e-5,
            "the drive yaw is absolute: {}",
            owl.yaw
        );
        assert!(!owl.moving, "driven is not walking (no walk anim/noise)");

        let x = owl.pos.x;
        owl.integrate(
            1.0 / 20.0,
            owl_def(),
            Vec3::ZERO,
            false,
            &floor_at_zero,
            &|_| false,
        );
        assert_eq!(owl.pos.x, x, "an un-renewed drive expires — the mob parks");
        assert!(
            (owl.yaw - 1.0).abs() < 1e-5,
            "nothing fights the driven yaw while idle: {}",
            owl.yaw
        );
    }

    #[test]
    fn knockback_stagger_overrides_a_drive_intent() {
        // A punched vehicle takes its knockback: the decaying knockback owns
        // horizontal velocity for the stagger, the drive is consumed unused.
        let mut owl = Instance::new(Mob::Owl, Vec3::new(0.5, 0.0, 0.5), 0.0, 1);
        let from = Vec3::new(2.0, 0.0, 0.5); // hit from +X: knockback pushes -X
        owl.damage(1.0, Some(from), true, None, &default_feedback());
        assert!(owl.set_drive(5.0, 0.0, Some(1.0)));
        owl.integrate(
            1.0 / 20.0,
            owl_def(),
            Vec3::ZERO,
            false,
            &floor_at_zero,
            &|_| false,
        );
        assert!(
            owl.pos.x < 0.5,
            "knockback wins over the drive during the stagger: x {}",
            owl.pos.x
        );
        assert_eq!(owl.yaw, 0.0, "stagger rejects the drive yaw as well");
        assert!(owl.drive.is_none(), "the rejected intent still expires");
    }

    #[test]
    fn knockback_pushes_away_and_overrides_the_wish() {
        let mut owl = Instance::new(Mob::Owl, Vec3::new(0.5, 0.0, 0.5), 0.0, 1);
        // Settle on the floor first.
        owl.integrate(0.05, owl_def(), Vec3::ZERO, false, &floor_at_zero, &|_| {
            false
        });
        let x0 = owl.pos.x;
        // Hit from the +X side → knockback toward -X. This is the key invariant: the
        // knockback survives `integrate`'s per-tick wish-velocity overwrite.
        assert!(!owl.damage(
            1.0,
            Some(Vec3::new(5.0, 0.0, 0.5)),
            true,
            None,
            &default_feedback()
        ));
        // Wish toward +X (toward the attacker); the knockback must win during the stagger.
        for _ in 0..4 {
            owl.integrate(
                0.05,
                owl_def(),
                Vec3::new(1.0, 0.0, 0.0),
                false,
                &floor_at_zero,
                &|_| false,
            );
        }
        assert!(
            owl.pos.x < x0 - 0.05,
            "knocked back -X despite wishing +X: {x0} -> {}",
            owl.pos.x
        );
        assert!(!owl.moving, "a staggered mob doesn't read as walking");
    }

    #[test]
    fn a_submerged_mob_swims_up_instead_of_sinking() {
        // Solid bed below y==0, water filling y in 0..=5. Start the mob submerged at
        // y==1: buoyancy should lift it over a few ticks (gravity alone would sink it).
        let solid = |c: IVec3| c.y < 0;
        let water = |c: IVec3| (0..=5).contains(&c.y);
        let mut owl = Instance::new(Mob::Owl, Vec3::new(0.5, 1.0, 0.5), 0.0, 1);
        let y0 = owl.pos.y;
        for _ in 0..20 {
            owl.integrate(1.0 / 60.0, owl_def(), Vec3::ZERO, false, &solid, &water);
        }
        assert!(
            owl.pos.y > y0,
            "a submerged mob rises toward the surface: {y0} -> {}",
            owl.pos.y
        );
    }

    #[test]
    fn surface_buoyancy_converges_from_both_sides_without_overshoot() {
        let surface = 6.0;
        let target = surface - SURFACE_DRAFT;
        for start in [target - 2.0, target + 1.0] {
            let mut y = start;
            for _ in 0..200 {
                let before = target - y;
                let velocity = surface_vertical_velocity(0.0, y, Some(surface), 0.05);
                y += velocity * 0.05;
                let after = target - y;
                assert!(
                    before == 0.0 || before.signum() == after.signum() || after.abs() < 1e-6,
                    "surface float crossed its target: {before} -> {after}"
                );
                assert!(
                    after.abs() <= before.abs() + 1e-6,
                    "surface float must converge monotonically: {before} -> {after}"
                );
            }
            assert!(
                (y - target).abs() < 1e-4,
                "surface float settles at the waterline from {start}: {y}"
            );
        }
    }

    #[test]
    fn a_surface_body_out_of_water_falls_under_gravity() {
        let mut velocity = 0.0;
        for _ in 0..3 {
            let next = surface_vertical_velocity(velocity, 10.0, None, 0.05);
            assert!(next < velocity, "gravity accelerates the dry hull downward");
            velocity = next;
        }
    }

    #[test]
    fn a_mob_bobs_up_and_down_through_the_water_surface_like_the_player() {
        // Water fills y in 0..=5 (surface at y==6) over a solid bed at y<0. The mob
        // swims up, breaks the surface, gravity pulls it back, it re-enters and rises
        // again — a real bob through the waterline (not a dead float, not a wiggle that
        // never re-enters). Run the real 20 TPS step.
        let solid = |c: IVec3| c.y < 0;
        let water = |c: IVec3| (0..=5).contains(&c.y);
        let mut owl = Instance::new(Mob::Owl, Vec3::new(0.5, 1.0, 0.5), 0.0, 1);
        // Let it rise to the surface and get into the bob.
        for _ in 0..100 {
            owl.integrate(0.05, owl_def(), Vec3::ZERO, false, &solid, &water);
        }
        // Over the next couple of seconds it must move both up (swim) and down
        // (gravity), and stay in a sane band around the surface.
        let (mut lo, mut hi) = (f32::MAX, f32::MIN);
        let (mut went_up, mut went_down) = (false, false);
        for _ in 0..120 {
            let before = owl.pos.y;
            owl.integrate(0.05, owl_def(), Vec3::ZERO, false, &solid, &water);
            let dy = owl.pos.y - before;
            went_up |= dy > 0.01;
            went_down |= dy < -0.01;
            lo = lo.min(owl.pos.y);
            hi = hi.max(owl.pos.y);
        }
        assert!(
            went_up && went_down,
            "bobs both up and down (up {went_up}, down {went_down})"
        );
        assert!(hi > 5.5, "rises up to/through the surface: hi {hi}");
        assert!(
            (4.0..=7.0).contains(&lo) && (4.0..=7.0).contains(&hi),
            "stays at the waterline: {lo}..{hi}"
        );
    }

    #[test]
    fn a_swimming_mob_climbs_out_onto_an_adjacent_ledge() {
        // A shore the climb-boost can actually clear: water (cells y in 0..SURFACE) over a
        // bed at y<0, with land at x>=1 whose top is AT the waterline. The swim climb-boost
        // (`SWIM_CLIMB`, fired by `ledge_ahead`) lifts the mob's feet just over the surface
        // so it steps out onto the land instead of hugging the shore forever. How high the
        // boost reaches depends on the (tunable) swim constants, so the land is kept at the
        // waterline and the checks derive from the owl's own size + this geometry — no swim
        // numbers are baked in. (The original test hard-coded a 1-block ledge, which needs
        // a far stronger boost than the tuned `SWIM_CLIMB` and so never passed.)
        const SURFACE: i32 = 4; // top of the water (and of the land it climbs onto)
        const SHORE: f32 = 1.0; // land starts at world x = 1
        let solid = |c: IVec3| c.y < 0 || (c.x >= 1 && c.y < SURFACE);
        let water = |c: IVec3| c.x <= 0 && (0..SURFACE).contains(&c.y);
        let half = owl_def().size.half_width;
        let mut owl = Instance::new(Mob::Owl, Vec3::new(0.5, 1.0, 0.5), 0.0, 1);
        for _ in 0..300 {
            owl.integrate(
                0.05,
                owl_def(),
                Vec3::new(1.0, 0.0, 0.0),
                false,
                &solid,
                &water,
            );
        }
        assert!(
            owl.on_ground(),
            "settled on the land, not still bobbing in the water: y {}",
            owl.pos.y
        );
        assert!(
            owl.pos.y >= SURFACE as f32 - 0.05,
            "rests up at the land surface, out of the water: y {}",
            owl.pos.y
        );
        assert!(
            owl.pos.x + half > SHORE,
            "climbed past the shore onto the land: x {}",
            owl.pos.x
        );
    }

    #[test]
    fn swim_climb_does_not_boost_toward_a_ledge_above_reach() {
        const SURFACE: i32 = 4;
        // Land top is one block above the waterline. From the submerged start pose this
        // is not yet reachable; the mob must swim up first instead of getting a cliff
        // boost from below.
        let solid = |c: IVec3| c.y < 0 || (c.x >= 1 && c.y < SURFACE + 1);
        let water = |c: IVec3| c.x <= 0 && (0..SURFACE).contains(&c.y);
        let mut owl = Instance::new(Mob::Owl, Vec3::new(0.5, SURFACE as f32 - 0.7, 0.5), 0.0, 1);
        assert!(
            !owl.ledge_ahead(Vec3::new(1.0, 0.0, 0.0), owl_def().size.half_width, &solid),
            "ledge top is too far above the mob's current feet"
        );
        let y0 = owl.pos.y;
        owl.integrate(
            0.05,
            owl_def(),
            Vec3::new(1.0, 0.0, 0.0),
            false,
            &solid,
            &water,
        );
        assert!(
            owl.pos.y < y0 + 0.1,
            "uses normal swim rise, not the ledge boost: {y0} -> {}",
            owl.pos.y
        );
    }

    #[test]
    fn a_mob_in_flowing_water_is_carried_downstream() {
        // Water fills y in 0..=5 over a solid bed at y<0, with a current heading +X
        // everywhere. A mob sitting in it with no wish to move must still drift
        // downstream — like the player and dropped items do.
        let solid = |c: IVec3| c.y < 0;
        let water = |c: IVec3| (0..=5).contains(&c.y);
        let flow = |_: Vec3| Vec3::new(1.0, 0.0, 0.0);
        let mut owl = Instance::new(Mob::Owl, Vec3::new(0.5, 1.0, 0.5), 0.0, 1);
        let x0 = owl.pos.x;
        for _ in 0..60 {
            owl.integrate_with_flow(
                1.0 / 60.0,
                owl_def(),
                Vec3::ZERO,
                false,
                true,
                &boxes_of(&solid),
                &[],
                &[],
                &solid,
                &water,
                &|_| None,
                &flow,
            );
        }
        assert!(
            owl.pos.x > x0 + 0.3,
            "the current carries the mob downstream: {x0} -> {}",
            owl.pos.x
        );

        // Still water (no current) leaves an idle mob where it is — proving it's the flow
        // doing the carrying, not stray drift.
        let still = |_: Vec3| Vec3::ZERO;
        let mut calm = Instance::new(Mob::Owl, Vec3::new(0.5, 1.0, 0.5), 0.0, 1);
        for _ in 0..60 {
            calm.integrate_with_flow(
                1.0 / 60.0,
                owl_def(),
                Vec3::ZERO,
                false,
                true,
                &boxes_of(&solid),
                &[],
                &[],
                &solid,
                &water,
                &|_| None,
                &still,
            );
        }
        assert!(
            (calm.pos.x - 0.5).abs() < 1e-3,
            "no current → no horizontal drift: x {}",
            calm.pos.x
        );
    }
}
