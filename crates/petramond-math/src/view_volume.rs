use crate::math::{IVec3, Mat4, Vec3, Vec4};
use crate::world_pos::WorldPos;

#[cfg(test)]
mod tests;

#[derive(Copy, Clone, Debug)]
pub struct Frustum {
    planes: [Vec4; 6],
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Containment {
    Outside,
    Intersect,
    Inside,
}

impl Frustum {
    pub fn from_view_proj(m: Mat4) -> Self {
        let r0 = m.row(0);
        let r1 = m.row(1);
        let r2 = m.row(2);
        let r3 = m.row(3);
        let mut planes = [r3 + r0, r3 - r0, r3 + r1, r3 - r1, r2, r3 - r2];
        for p in &mut planes {
            let len = p.truncate().length();
            if len > 0.0 {
                *p /= len;
            }
        }
        Self { planes }
    }

    pub fn permissive() -> Self {
        Self {
            planes: [Vec4::new(0.0, 0.0, 0.0, 1.0); 6],
        }
    }

    #[inline]
    pub fn aabb_containment(&self, min: Vec3, max: Vec3) -> Containment {
        let c = (min + max) * 0.5;
        let e = (max - min) * 0.5;
        let mut inside = true;
        for p in &self.planes {
            let n = Vec3::new(p.x, p.y, p.z);
            let d = n.dot(c) + p.w;
            let r = n.abs().dot(e);
            if d + r < 0.0 {
                return Containment::Outside;
            }
            inside &= d - r >= 0.0;
        }
        if inside {
            Containment::Inside
        } else {
            Containment::Intersect
        }
    }

    #[inline]
    pub fn aabb_visible(&self, min: Vec3, max: Vec3) -> bool {
        let c = (min + max) * 0.5;
        let e = (max - min) * 0.5;
        for p in &self.planes {
            let n = Vec3::new(p.x, p.y, p.z);
            if n.dot(c) + n.abs().dot(e) + p.w < 0.0 {
                return false;
            }
        }
        true
    }
}

#[inline]
pub fn aabb_distance_sq(p: Vec3, min: Vec3, max: Vec3) -> f32 {
    let dx = if p.x < min.x {
        min.x - p.x
    } else if p.x > max.x {
        p.x - max.x
    } else {
        0.0
    };
    let dy = if p.y < min.y {
        min.y - p.y
    } else if p.y > max.y {
        p.y - max.y
    } else {
        0.0
    };
    let dz = if p.z < min.z {
        min.z - p.z
    } else if p.z > max.z {
        p.z - max.z
    } else {
        0.0
    };
    dx * dx + dy * dy + dz * dz
}

#[derive(Copy, Clone, Debug)]
pub struct ViewVolume {
    frustum: Frustum,
    origin: IVec3,
    eye: Vec3,
    cull_dist_sq: f32,
    pixel_scale: f32,
}

impl ViewVolume {
    pub fn new(
        frustum: Frustum,
        origin: IVec3,
        eye: WorldPos,
        cull_dist: f32,
        pixel_scale: f32,
    ) -> Self {
        Self {
            frustum,
            origin,
            eye: eye.relative_to(origin),
            cull_dist_sq: cull_dist * cull_dist,
            pixel_scale,
        }
    }

    pub fn unbounded() -> Self {
        Self::new(
            Frustum::permissive(),
            IVec3::ZERO,
            WorldPos::ZERO,
            f32::INFINITY,
            f32::MAX,
        )
    }

    #[inline]
    pub fn eye(&self) -> WorldPos {
        WorldPos::block_min(self.origin) + self.eye
    }

    #[inline]
    pub fn aabb_visible(&self, min: WorldPos, max: WorldPos) -> bool {
        let (lo, hi) = (min.relative_to(self.origin), max.relative_to(self.origin));
        self.frustum.aabb_visible(lo, hi) && aabb_distance_sq(self.eye, lo, hi) <= self.cull_dist_sq
    }

    #[inline]
    pub fn covers_a_pixel(&self, min: WorldPos, max: WorldPos, size: f32) -> bool {
        let reach = size * self.pixel_scale;
        let (lo, hi) = (min.relative_to(self.origin), max.relative_to(self.origin));
        aabb_distance_sq(self.eye, lo, hi) <= reach * reach
    }
}
