//! Line of sight over WORLD COLLISION: a ray is clear when no collision box lies
//! between the endpoints.
//!
//! Collision (not visual opacity) is the deliberate sight predicate: leaves and
//! any future glass-like block collide and therefore block sight, while
//! non-colliding decorations (torches, flowers) never do. Both the melee strike
//! gate and chase engagement share this test, so "can start hunting" and "can
//! hit" agree on what a mob sees through.

use crate::world::World;
use petramond_math::math::{IVec3, Vec3};
use petramond_math::world_pos::WorldPos;

const LOS_EPS: f32 = 0.001;

/// Whether the straight line `from → to` crosses no world collision box.
pub(super) fn line_clear(world: &World, from: WorldPos, to: WorldPos) -> bool {
    line_clear_skipping(world, from, to, &[])
}

/// [`line_clear`], ignoring the boxes of the `skip` cells — the cells an
/// action is aimed at, which may stand in its own way.
pub(crate) fn line_clear_skipping(
    world: &World,
    from: WorldPos,
    to: WorldPos,
    skip: &[IVec3],
) -> bool {
    let delta = to - from;
    let dist = delta.length();
    if dist <= f32::EPSILON {
        return true;
    }
    !ray_hits_collision(world, from, delta / dist, dist, skip)
}

fn ray_hits_collision(world: &World, eye: WorldPos, dir: Vec3, max_t: f32, skip: &[IVec3]) -> bool {
    let mut ix = eye.x.floor() as i32;
    let mut iy = eye.y.floor() as i32;
    let mut iz = eye.z.floor() as i32;
    let step = IVec3::new(sign(dir.x), sign(dir.y), sign(dir.z));
    let mut t_max = Vec3::new(
        boundary_t(eye.x, dir.x),
        boundary_t(eye.y, dir.y),
        boundary_t(eye.z, dir.z),
    );
    let t_delta = Vec3::new(inv_abs(dir.x), inv_abs(dir.y), inv_abs(dir.z));

    loop {
        let cell = IVec3::new(ix, iy, iz);
        if !skip.contains(&cell) && cell_hits_collision(world, eye, dir, max_t, cell) {
            return true;
        }
        let (axis, t_exit) = if t_max.x <= t_max.y && t_max.x <= t_max.z {
            (0, t_max.x)
        } else if t_max.y <= t_max.z {
            (1, t_max.y)
        } else {
            (2, t_max.z)
        };
        if t_exit > max_t {
            return false;
        }
        match axis {
            0 => {
                ix += step.x;
                t_max.x += t_delta.x;
            }
            1 => {
                iy += step.y;
                t_max.y += t_delta.y;
            }
            _ => {
                iz += step.z;
                t_max.z += t_delta.z;
            }
        }
    }
}

fn cell_hits_collision(world: &World, eye: WorldPos, dir: Vec3, max_t: f32, cell: IVec3) -> bool {
    // In the eye's frame: the cell is within the ray's reach of it.
    let base = WorldPos::block_min(cell) - eye;
    world
        .collision_boxes_at(cell.x, cell.y, cell.z)
        .iter()
        .any(|b| {
            let min = base + Vec3::from(b.min);
            let max = base + Vec3::from(b.max);
            ray_vs_aabb(Vec3::ZERO, dir, min, max)
                .is_some_and(|t| t > LOS_EPS && t < max_t - LOS_EPS)
        })
}

fn ray_vs_aabb(eye: Vec3, dir: Vec3, min: Vec3, max: Vec3) -> Option<f32> {
    let (e, d, lo, hi) = (
        eye.to_array(),
        dir.to_array(),
        min.to_array(),
        max.to_array(),
    );
    let mut t_near = f32::NEG_INFINITY;
    let mut t_far = f32::INFINITY;
    for i in 0..3 {
        if d[i].abs() < LOS_EPS {
            if e[i] < lo[i] - LOS_EPS || e[i] > hi[i] + LOS_EPS {
                return None;
            }
        } else {
            let inv = 1.0 / d[i];
            let mut t1 = (lo[i] - e[i]) * inv;
            let mut t2 = (hi[i] - e[i]) * inv;
            if t1 > t2 {
                std::mem::swap(&mut t1, &mut t2);
            }
            t_near = t_near.max(t1);
            t_far = t_far.min(t2);
            if t_near > t_far {
                return None;
            }
        }
    }
    (t_far >= 0.0).then_some(t_near.max(0.0))
}

fn sign(v: f32) -> i32 {
    if v > 0.0 {
        1
    } else if v < 0.0 {
        -1
    } else {
        0
    }
}

fn inv_abs(v: f32) -> f32 {
    if v == 0.0 {
        f32::INFINITY
    } else {
        1.0 / v.abs()
    }
}

fn boundary_t(coord: f64, dir: f32) -> f32 {
    let frac = (coord - coord.floor()) as f32;
    if dir > 0.0 {
        (1.0 - frac) / dir
    } else if dir < 0.0 {
        frac / -dir
    } else {
        f32::INFINITY
    }
}
