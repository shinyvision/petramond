use petramond_world::{block::Block, mathh::IVec3};

use crate::{feature::FeatureCtx, rng::FeatureRng};

use super::{rotate, CARDINALS};

pub(super) struct TrunkPosture {
    direction: (i32, i32),
    lean: i32,
    shoulder: i32,
}

impl TrunkPosture {
    pub(super) fn sample(lean: (i32, i32), rng: &mut FeatureRng) -> Self {
        Self {
            direction: CARDINALS[rng.next_i32(0, 3) as usize],
            lean: rng.next_i32(lean.0, lean.1),
            shoulder: rng.next_i32(-1, 1),
        }
    }

    pub(super) fn offset(&self, level: i32, height: i32) -> (i32, i32) {
        if self.lean == 0 {
            return (0, 0);
        }
        // Keep the foot upright, then bend as a whole instead of random-walking
        // each layer. The shoulder returns toward the lean at the crown.
        let t = ((level - 2) as f32 / (height - 3).max(1) as f32).clamp(0.0, 1.0);
        let along = (self.lean as f32 * t * t * (3.0 - 2.0 * t)).round() as i32;
        let across = (self.shoulder as f32 * 4.0 * t * (1.0 - t)).round() as i32;
        let side = rotate(self.direction, true);
        (
            self.direction.0 * along + side.0 * across,
            self.direction.1 * along + side.1 * across,
        )
    }
}

pub(super) fn connected_branch(ctx: &mut FeatureCtx, a: IVec3, b: IVec3, log: Block) {
    connected_line(a, b, |p| ctx.set_branch(p, log));
}

pub(super) fn connected_trunk(ctx: &mut FeatureCtx, a: IVec3, b: IVec3, log: Block) {
    connected_line(a, b, |p| ctx.set_log(p, log));
}

fn connected_line(a: IVec3, b: IVec3, mut emit: impl FnMut(IVec3)) {
    let delta = IVec3::new(b.x - a.x, b.y - a.y, b.z - a.z);
    let steps = delta.x.abs().max(delta.y.abs()).max(delta.z.abs()).max(1);
    let mut previous = a;
    emit(a);
    for i in 1..=steps {
        let t = i as f32 / steps as f32;
        let next = IVec3::new(
            a.x + (delta.x as f32 * t).round() as i32,
            a.y + (delta.y as f32 * t).round() as i32,
            a.z + (delta.z as f32 * t).round() as i32,
        );
        // Bridge every diagonal: forks must be wood-connected, even when a
        // one-block stem changes both height and horizontal cell together.
        for axis in 0..3 {
            let target = match axis {
                0 => next.x,
                1 => next.z,
                _ => next.y,
            };
            let coordinate = match axis {
                0 => &mut previous.x,
                1 => &mut previous.z,
                _ => &mut previous.y,
            };
            *coordinate = target;
            emit(previous);
        }
    }
}

#[cfg(test)]
mod tests;
