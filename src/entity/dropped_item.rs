//! A dropped item-stack bobbing in the world after a block breaks.
//!
//! Physics is intentionally tiny: constant gravity plus axis-resolved collision
//! against solid blocks (so items rest on the ground and slide off into open
//! cells without tunnelling). Spin and age advance for the renderer/pickup; the
//! `entity` module never draws — `App` reads `pos`/`spin`/`stack` directly.

use crate::block::Block;
use crate::item::ItemStack;
use crate::mathh::{IVec3, Vec3};
use crate::world::World;

use super::{hash01, hash_signed};

/// Downward acceleration applied to dropped items, in m/s².
pub const GRAVITY: f32 = -20.0;

/// Half-extent of a dropped item's cube, in metres. Used for both block
/// collision and the player pickup test (a small ~0.3 m cube).
pub const ITEM_HALF_EXTENT: f32 = 0.125;

/// Inner absorb radius (player body-centre to item-centre): once the magnetised
/// item flies this close it is sucked into the inventory and despawned.
pub const ABSORB_RADIUS: f32 = 0.6;

/// body-centre enters the magnet phase and accelerates toward the chest.
pub const ATTRACT_RADIUS: f32 = 1.5;

/// Base inward speed (m/s) at the edge of the attract radius. The magnet pull
/// ramps up as the item nears the player so the suck visibly accelerates.
const MAGNET_BASE_SPEED: f32 = 6.0;
/// Extra inward speed (m/s) added as the item closes on the player (scaled by how
/// far inside the attract radius it is), giving the accelerating "vacuum" feel.
const MAGNET_RAMP_SPEED: f32 = 10.0;

/// Horizontal velocity damping per second while resting on the ground (drops
/// quickly settle instead of sliding forever).
const GROUND_DAMP_PER_SEC: f32 = 6.0;
/// Air drag applied to horizontal velocity each second (mild).
const AIR_DAMP_PER_SEC: f32 = 0.8;
/// Angular speed of the idle spin, in radians/second.
const SPIN_SPEED: f32 = 2.0;

/// A free-floating stack of items in the world.
#[derive(Clone, Debug, PartialEq)]
pub struct DroppedItem {
    /// World-space centre of the item cube.
    pub pos: Vec3,
    pub vel: Vec3,
    pub stack: ItemStack,
    /// 6-bit skylight used by render handoff. The game refreshes this when a
    /// drop crosses voxel cells, avoiding per-frame world-light lookups for
    /// long-lived item piles.
    pub skylight: u8,
    /// Seconds since spawn; drives despawn and bob.
    pub age: f32,
    /// Accumulated Y-rotation in radians for the idle spin.
    pub spin: f32,
}

impl DroppedItem {
    /// Spawn a stack at `pos` with a small deterministic upward+outward "pop".
    ///
    /// `seed` varies the pop direction per spawn (use an incrementing counter or
    /// a hash of the block position): no RNG, fully reproducible. Without a
    /// varying seed every drop from one break would launch identically and stack
    /// into a single column.
    pub fn new(pos: Vec3, stack: ItemStack, seed: u32) -> Self {
        let s = seed as u64;
        // Outward kick in the XZ plane, gentle so drops stay near the block.
        let ang = hash01(s) * std::f32::consts::TAU;
        let speed = 1.0 + hash01(s ^ 0x5151) * 1.5; // 1.0..2.5 m/s
        let up = 2.5 + hash01(s ^ 0xA2A2) * 1.5; // 2.5..4.0 m/s upward pop
        let vel = Vec3::new(ang.cos() * speed, up, ang.sin() * speed);
        DroppedItem {
            pos,
            vel,
            stack,
            skylight: 63,
            age: 0.0,
            // Stagger the starting spin so a pile of drops isn't phase-locked.
            spin: hash_signed(s ^ 0x3C3C) * std::f32::consts::PI,
        }
    }

    /// Advance physics by `dt`: gravity, axis-resolved block collision, spin and
    /// age. Items rest on solid ground and have horizontal velocity damped while
    /// grounded.
    ///
    /// `magnet_target` is the player chest (body-centre) the item is sucked into:
    /// once the item is within [`ATTRACT_RADIUS`] of it the normal physics are
    /// bypassed and the item flies straight at the target with an accelerating
    /// pull, ignoring gravity/collision so the vacuum reads cleanly. Pass `None`
    /// (or a far target) to disable magnetism.
    pub fn tick(&mut self, dt: f32, world: &World, magnet_target: Option<Vec3>) {
        self.integrate(dt, magnet_target, &|p| {
            Block::from_id(world.chunk_block(p.x, p.y, p.z)).is_solid()
        });
    }

    /// Pure integration behind [`tick`](Self::tick). `solid_at` reports whether a
    /// block cell is solid. Split out so tests can drive the full physics against
    /// a stub world (a real `World` spins up a worker pool).
    fn integrate(
        &mut self,
        dt: f32,
        magnet_target: Option<Vec3>,
        solid_at: &impl Fn(IVec3) -> bool,
    ) {
        self.age += dt;
        self.spin = (self.spin + SPIN_SPEED * dt) % std::f32::consts::TAU;

        // Magnet phase: if a target is within the attract radius, fly straight at
        // it with an accelerating pull (no gravity/collision) so the suck is
        // smooth and unmistakable. Falls back to free physics otherwise.
        if let Some(target) = magnet_target {
            let to = target - self.pos;
            let dist = to.length();
            if dist <= ATTRACT_RADIUS {
                if dist > 1e-4 {
                    // Speed ramps up as the item closes in (1.0 at the edge of the
                    // attract radius, ~0 right at the target).
                    let closeness = 1.0 - (dist / ATTRACT_RADIUS);
                    let speed = MAGNET_BASE_SPEED + MAGNET_RAMP_SPEED * closeness;
                    self.vel = (to / dist) * speed;
                    // Don't overshoot the target in a single step.
                    let step = (speed * dt).min(dist);
                    self.pos += (to / dist) * step;
                }
                return;
            }
        }

        // Gravity.
        self.vel.y += GRAVITY * dt;

        // Axis-resolved movement: move + resolve each axis independently so a
        // wall on one axis never blocks sliding along the others.
        let grounded = self.move_axis_resolved(dt, solid_at);

        // Damping: strong on the ground (settle), mild in the air.
        let damp = if grounded {
            GROUND_DAMP_PER_SEC
        } else {
            AIR_DAMP_PER_SEC
        };
        let k = (1.0 - damp * dt).clamp(0.0, 1.0);
        self.vel.x *= k;
        self.vel.z *= k;
        if grounded && self.vel.y < 0.0 {
            self.vel.y = 0.0;
        }
    }

    /// Move along each axis in turn, resolving against solid cells. Returns
    /// `true` if the item is resting on solid ground after the move.
    fn move_axis_resolved(&mut self, dt: f32, solid_at: &impl Fn(IVec3) -> bool) -> bool {
        let mut grounded = false;

        // Y first so we land cleanly before horizontal slide.
        let dy = self.vel.y * dt;
        self.pos.y += dy;
        if self.collides(solid_at) {
            self.pos.y -= dy;
            if self.vel.y < 0.0 {
                grounded = true;
            }
            self.vel.y = 0.0;
        }

        let dx = self.vel.x * dt;
        self.pos.x += dx;
        if self.collides(solid_at) {
            self.pos.x -= dx;
            self.vel.x = 0.0;
        }

        let dz = self.vel.z * dt;
        self.pos.z += dz;
        if self.collides(solid_at) {
            self.pos.z -= dz;
            self.vel.z = 0.0;
        }

        grounded
    }

    /// Does the item's AABB overlap any solid block cell?
    fn collides(&self, solid_at: &impl Fn(IVec3) -> bool) -> bool {
        let h = ITEM_HALF_EXTENT;
        let min = self.pos - Vec3::splat(h);
        let max = self.pos + Vec3::splat(h);
        let (x0, x1) = (min.x.floor() as i32, max.x.floor() as i32);
        let (y0, y1) = (min.y.floor() as i32, max.y.floor() as i32);
        let (z0, z1) = (min.z.floor() as i32, max.z.floor() as i32);
        for x in x0..=x1 {
            for y in y0..=y1 {
                for z in z0..=z1 {
                    if solid_at(IVec3::new(x, y, z)) {
                        return true;
                    }
                }
            }
        }
        false
    }

    /// `true` if `player_pos` (player body-centre) is within the inner
    /// [`ABSORB_RADIUS`]: the item should be vacuumed into the inventory and
    /// despawned. Cheap squared-distance test.
    #[inline]
    pub fn within_pickup(&self, player_pos: Vec3) -> bool {
        let d = self.pos - player_pos;
        d.length_squared() <= ABSORB_RADIUS * ABSORB_RADIUS
    }

    /// `true` if `player_pos` (player body-centre) is within the outer
    /// [`ATTRACT_RADIUS`]: the item is in the magnet phase and flying at the
    /// player. Cheap squared-distance test.
    #[inline]
    pub fn within_attract(&self, player_pos: Vec3) -> bool {
        let d = self.pos - player_pos;
        d.length_squared() <= ATTRACT_RADIUS * ATTRACT_RADIUS
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::item::ItemType;

    fn stack() -> ItemStack {
        ItemStack::new(ItemType::Dirt, 1)
    }

    /// A flat floor at y < 0: every cell with y < 0 is solid, everything else air.
    fn floor_at_zero(p: IVec3) -> bool {
        p.y < 0
    }

    /// No solid blocks anywhere.
    fn empty(_p: IVec3) -> bool {
        false
    }

    #[test]
    fn new_pops_upward_and_outward() {
        let d = DroppedItem::new(Vec3::ZERO, stack(), 7);
        assert!(d.vel.y > 0.0, "should pop upward");
        let horiz = (d.vel.x * d.vel.x + d.vel.z * d.vel.z).sqrt();
        assert!(horiz > 0.0, "should have some outward kick");
        assert_eq!(d.age, 0.0);
    }

    #[test]
    fn new_is_deterministic_per_seed_and_varies() {
        let a = DroppedItem::new(Vec3::ZERO, stack(), 3);
        let b = DroppedItem::new(Vec3::ZERO, stack(), 3);
        assert_eq!(a.vel, b.vel, "same seed → same pop");
        let c = DroppedItem::new(Vec3::ZERO, stack(), 4);
        assert_ne!(a.vel, c.vel, "different seed → different pop");
    }

    #[test]
    fn gravity_pulls_down_in_free_fall() {
        let mut d = DroppedItem::new(Vec3::new(0.0, 50.0, 0.0), stack(), 1);
        d.vel = Vec3::ZERO; // isolate gravity from the pop
        let before = d.pos.y;
        d.integrate(0.1, None, &empty);
        assert!(d.pos.y < before, "free fall should lower the item");
        assert!(d.vel.y < 0.0, "downward velocity accrues");
    }

    #[test]
    fn rests_on_a_floor() {
        // Start just above the floor with downward velocity; integrate a while.
        let mut d = DroppedItem::new(Vec3::new(0.5, 2.0, 0.5), stack(), 2);
        d.vel = Vec3::new(0.0, -1.0, 0.0);
        for _ in 0..300 {
            d.integrate(1.0 / 60.0, None, &floor_at_zero);
        }
        // Floor top is y == 0; item half-extent keeps its centre above it.
        assert!(
            d.pos.y >= ITEM_HALF_EXTENT - 1e-3,
            "item sank through floor: {}",
            d.pos.y
        );
        assert!(
            d.pos.y < 0.5,
            "item should be resting near the floor: {}",
            d.pos.y
        );
        // Vertical velocity is killed once grounded.
        assert!(d.vel.y.abs() < 1e-3, "grounded item should stop falling");
    }

    #[test]
    fn horizontal_velocity_damps_on_ground() {
        let mut d = DroppedItem::new(Vec3::new(0.5, ITEM_HALF_EXTENT, 0.5), stack(), 5);
        d.vel = Vec3::new(4.0, 0.0, 0.0);
        for _ in 0..120 {
            d.integrate(1.0 / 60.0, None, &floor_at_zero);
        }
        assert!(
            d.vel.x.abs() < 0.5,
            "ground drag should bleed off horizontal speed: {}",
            d.vel.x
        );
    }

    #[test]
    fn age_accumulates() {
        let mut d = DroppedItem::new(Vec3::ZERO, stack(), 9);
        d.integrate(0.5, None, &empty);
        d.integrate(0.5, None, &empty);
        assert!((d.age - 1.0).abs() < 1e-5);
    }

    #[test]
    fn pickup_range_test() {
        let d = DroppedItem::new(Vec3::new(0.0, 0.0, 0.0), stack(), 0);
        // Inside the inner absorb radius.
        assert!(d.within_pickup(Vec3::new(0.0, 0.0, 0.0)));
        // Just outside the absorb radius but inside attract: attract-only.
        let just_out = Vec3::new(ABSORB_RADIUS + 0.1, 0.0, 0.0);
        assert!(!d.within_pickup(just_out));
        assert!(d.within_attract(just_out));
        // More than a block away must not start the magnet/pickup path.
        assert!(!d.within_attract(Vec3::new(ATTRACT_RADIUS + 0.01, 0.0, 0.0)));
        // Well beyond both radii.
        assert!(!d.within_pickup(Vec3::new(10.0, 0.0, 0.0)));
        assert!(!d.within_attract(Vec3::new(10.0, 0.0, 0.0)));
    }

    #[test]
    fn magnet_pulls_item_toward_target() {
        // A target inside the attract radius (but outside absorb) sucks the item
        // toward it: the distance shrinks each step and it ignores gravity.
        let target = Vec3::new(0.0, 0.0, 0.0);
        let start = Vec3::new(0.0, ATTRACT_RADIUS - 0.2, 0.0);
        let mut d = DroppedItem::new(start, stack(), 11);
        d.vel = Vec3::ZERO;
        let before = (d.pos - target).length();
        d.integrate(1.0 / 60.0, Some(target), &empty);
        let after = (d.pos - target).length();
        assert!(
            after < before,
            "magnet should pull the item toward the target"
        );
    }

    #[test]
    fn magnet_absorbs_within_inner_radius() {
        // Starting inside attract range, a few steps fly the item into the inner
        // absorb radius around the target.
        let target = Vec3::new(0.0, 0.0, 0.0);
        let start = Vec3::new(0.0, ATTRACT_RADIUS - 0.1, 0.0);
        let mut d = DroppedItem::new(start, stack(), 12);
        d.vel = Vec3::ZERO;
        for _ in 0..120 {
            d.integrate(1.0 / 60.0, Some(target), &empty);
            if d.within_pickup(target) {
                break;
            }
        }
        assert!(
            d.within_pickup(target),
            "magnetised item should reach the absorb radius"
        );
    }

    #[test]
    fn magnet_does_not_overshoot_target() {
        // A big dt must not fling the item past the target (clamped to the gap).
        let target = Vec3::new(0.0, 0.0, 0.0);
        let start = Vec3::new(0.0, 1.0, 0.0);
        let mut d = DroppedItem::new(start, stack(), 13);
        d.vel = Vec3::ZERO;
        d.integrate(10.0, Some(target), &empty);
        let dist = (d.pos - target).length();
        assert!(
            dist <= 1.0 + 1e-4,
            "item should not overshoot the target: {dist}"
        );
    }

    #[test]
    fn far_target_leaves_physics_untouched() {
        // A target beyond the attract radius leaves normal gravity physics intact.
        let far = Vec3::new(100.0, 0.0, 0.0);
        let mut d = DroppedItem::new(Vec3::new(0.0, 50.0, 0.0), stack(), 14);
        d.vel = Vec3::ZERO;
        let before = d.pos.y;
        d.integrate(0.1, Some(far), &empty);
        assert!(
            d.pos.y < before,
            "free fall should still apply outside attract range"
        );
        assert!(d.vel.y < 0.0);
    }
}
