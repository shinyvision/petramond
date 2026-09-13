//! Leaving a fluid onto a ledge: the one rule players and mobs share.
//!
//! A swimmer whose body breaks the surface may climb a ledge whose top sits at
//! most [`CLIMB_CELLS`] above its fluid-surface foothold: the rise the
//! pathfinder gives a climb edge. The fluid's `climb` row only decides how
//! that rise is delivered; the rise itself is solved from the ledge and the
//! feet, so no constant encodes a body size.

use petramond_math::math::Vec3;
use petramond_math::world_pos::WorldPos;
use petramond_world::block::Aabb;
use petramond_world::collision::DynBox;
use petramond_world::fluid::{FluidClimb, Immersion};

use crate::entity::VELOCITY_SLACK;
use crate::mob::CLIMB_CELLS;

/// How far beyond the body's leading face the ledge probe looks (m).
const PROBE_AHEAD: f32 = 0.2;
/// Height aimed above the ledge top so the feet land on it instead of grazing the lip.
const CLEARANCE: f32 = 0.1;
const EPS: f32 = 1e-4;

/// A swimming body that wants to reach the shore this tick.
#[derive(Clone, Copy, Debug)]
pub struct Swimmer {
    pub pos: WorldPos,
    pub vel_y: f32,
    pub half_width: f32,
    pub height: f32,
    /// Downward acceleration (m/s², positive) the body falls with outside the fluid.
    pub gravity: f32,
    /// The body's own jump take-off speed (m/s); a launch never exceeds it by
    /// more than [`VELOCITY_SLACK`].
    pub jump_speed: f32,
}

/// How a swimmer leaves the fluid.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ShoreClimb {
    /// Rise at least this fast (m/s): the ballistic speed that clears the ledge.
    Launch(f32),
    /// Resolve this tick's horizontal move with a collision step this tall (m).
    Step(f32),
}

/// A collision box's vertical extent, in world space.
type Span = (f64, f64);

impl Swimmer {
    /// The climb onto a reachable ledge ahead of `wish`, if any. A sinking
    /// body gets none, so a failed attempt must settle before the next one.
    pub fn shore_climb(
        self,
        wish: Vec3,
        immersion: Immersion,
        boxes: &impl Fn(i32, i32, i32) -> &'static [Aabb],
        obstacles: &[DynBox],
    ) -> Option<ShoreClimb> {
        let dir = Vec3::new(wish.x, 0.0, wish.z).normalize_or_zero();
        if self.vel_y < 0.0
            || dir == Vec3::ZERO
            || self.pos.y + f64::from(self.height) <= f64::from(immersion.surface_y)
        {
            return None;
        }
        let reach = (immersion.surface_y - EPS).floor() + 1.0 + CLIMB_CELLS as f32;
        let rise = (self.ledge_top(dir, f64::from(reach), boxes, obstacles)? - self.pos.y) as f32;
        Some(match immersion.fluid.motion.climb {
            FluidClimb::Jump => ShoreClimb::Launch(
                (2.0 * self.gravity * (rise + CLEARANCE))
                    .sqrt()
                    .min(self.jump_speed * VELOCITY_SLACK),
            ),
            FluidClimb::Step => ShoreClimb::Step(rise + CLEARANCE),
        })
    }

    /// The highest collision top above the feet and within `reach` under the
    /// body's footprint nudged ahead, when a body standing on it has
    /// headroom: anything else rising past the ledge within body height is a
    /// wall or a ceiling, not a shore.
    fn ledge_top(
        self,
        dir: Vec3,
        reach: f64,
        boxes: &impl Fn(i32, i32, i32) -> &'static [Aabb],
        obstacles: &[DynBox],
    ) -> Option<f64> {
        const EPS64: f64 = EPS as f64;
        let centre = self.pos + dir * PROBE_AHEAD;
        let hw = f64::from(self.half_width);
        let height = f64::from(self.height);
        let (min_x, max_x) = (centre.x - hw, centre.x + hw);
        let (min_z, max_z) = (centre.z - hw, centre.z + hw);
        let overlaps = |lo: [f64; 3], hi: [f64; 3]| {
            lo[0] < max_x && hi[0] > min_x && lo[2] < max_z && hi[2] > min_z
        };
        let ceiling = reach + height;
        let spans = || {
            let terrain = (self.pos.y.floor() as i32..=ceiling.floor() as i32).flat_map(move |y| {
                (min_x.floor() as i32..=(max_x - EPS64).floor() as i32).flat_map(move |x| {
                    (min_z.floor() as i32..=(max_z - EPS64).floor() as i32).flat_map(move |z| {
                        let at = [x, y, z].map(f64::from);
                        boxes(x, y, z).iter().filter_map(move |b| {
                            let lo: [f64; 3] = std::array::from_fn(|i| at[i] + f64::from(b.min[i]));
                            let hi: [f64; 3] = std::array::from_fn(|i| at[i] + f64::from(b.max[i]));
                            overlaps(lo, hi).then_some((lo[1], hi[1]))
                        })
                    })
                })
            });
            let entities = obstacles
                .iter()
                .filter(move |b| overlaps(b.min, b.max))
                .map(|b| (b.min[1], b.max[1]));
            terrain.chain(entities)
        };
        let top = spans()
            .map(|(_, top): Span| top)
            .filter(|&top| top > self.pos.y + EPS64 && top <= reach + EPS64)
            .reduce(f64::max)?;
        let blocked = spans()
            .any(|(bottom, span_top)| span_top > top + EPS64 && bottom < top + height - EPS64);
        (!blocked).then_some(top)
    }
}

#[cfg(test)]
mod tests;
