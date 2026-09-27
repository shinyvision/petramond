use petramond_math::math::Vec3;
use petramond_math::world_pos::WorldPos;
use petramond_world::block::Aabb;
use petramond_world::collision::DynBox;
use petramond_world::fluid::{FluidClimb, Immersion};

use crate::entity::VELOCITY_SLACK;
use crate::mob::CLIMB_CELLS;

const PROBE_AHEAD: f32 = 0.2;
const CLEARANCE: f32 = 0.1;
const EPS: f32 = 1e-4;

#[derive(Clone, Copy, Debug)]
pub struct Swimmer {
    pub pos: WorldPos,
    pub vel_y: f32,
    pub half_width: f32,
    pub height: f32,
    pub gravity: f32,
    pub jump_speed: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ShoreClimb {
    Launch(f32),
    Step(f32),
}

type Span = (f64, f64);

impl Swimmer {
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
