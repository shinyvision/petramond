use super::state::Player;
use crate::mathh::{IVec3, Vec3};
use crate::world::World;

/// Max block-interaction distance, measured from the eye.
pub const REACH: f32 = 4.0;

/// Result of a block raycast.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct RaycastHit {
    /// The solid cell the ray entered.
    pub block: IVec3,
    /// Face normal pointing back toward the eye. `block + normal` is the empty
    /// cell to place into. Zero when the eye started inside a solid block.
    pub normal: IVec3,
}

impl Player {
    /// Cast a ray from `eye` along (assumed-normalised) `dir`, returning the
    /// first solid block within `REACH`. Voxel DDA (Amanatides & Woo).
    pub fn raycast(eye: Vec3, dir: Vec3, world: &World) -> Option<RaycastHit> {
        Self::raycast_core(eye, dir, &|x, y, z| Self::solid_world(world, x, y, z))
    }

    pub(super) fn raycast_core<F: Fn(i32, i32, i32) -> bool>(
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
