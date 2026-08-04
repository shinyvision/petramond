//! Frustum and view-volume culling primitives, shared by world gathers
//! (streaming, draws, emitters) and the renderer. Pure math over the loaded
//! world — no camera state, no GPU types.

use crate::mathh::{Mat4, Vec3, Vec4};

/// View frustum as 6 inward-facing planes, for viewspace (frustum) culling.
/// Each plane is `(a,b,c,d)` with the convention `a·x + b·y + c·z + d >= 0`
/// inside. Extracted from a view-projection matrix (Gribb–Hartmann).
#[derive(Copy, Clone, Debug)]
pub struct Frustum {
    planes: [Vec4; 6],
}

impl Frustum {
    /// Build from a `view_proj` matrix. Assumes wgpu/DX/Metal/Vulkan clip space
    /// (NDC z in `[0,1]`, which `glam::Mat4::perspective_rh` produces) — hence the
    /// near plane is `row2`, not `row3 + row2`.
    pub fn from_view_proj(m: Mat4) -> Self {
        let r0 = m.row(0);
        let r1 = m.row(1);
        let r2 = m.row(2);
        let r3 = m.row(3);
        let mut planes = [
            r3 + r0, // left
            r3 - r0, // right
            r3 + r1, // bottom
            r3 - r1, // top
            r2,      // near  (z=0 plane in [0,1] clip)
            r3 - r2, // far
        ];
        for p in &mut planes {
            let len = p.truncate().length();
            if len > 0.0 {
                *p /= len;
            }
        }
        Self { planes }
    }

    /// A frustum that contains everything (used before the first real update).
    pub fn permissive() -> Self {
        // d = +inf-ish so every point is on the inside of every plane.
        Self {
            planes: [Vec4::new(0.0, 0.0, 0.0, 1.0); 6],
        }
    }

    /// True if the axis-aligned box `[min,max]` is at least partially inside the
    /// frustum. Uses the positive-vertex test: if the AABB corner farthest along a
    /// plane's normal is still behind that plane, the whole box is outside.
    pub fn aabb_visible(&self, min: Vec3, max: Vec3) -> bool {
        for p in &self.planes {
            let pv = Vec3::new(
                if p.x >= 0.0 { max.x } else { min.x },
                if p.y >= 0.0 { max.y } else { min.y },
                if p.z >= 0.0 { max.z } else { min.z },
            );
            if p.x * pv.x + p.y * pv.y + p.z * pv.z + p.w < 0.0 {
                return false;
            }
        }
        true
    }
}

/// Squared distance from `p` to the nearest point of the box `[min,max]`
/// (zero inside it).
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

/// What a frame can actually draw: the view frustum plus the distance past
/// which nothing is drawn.
///
/// Per-frame gathers take one of these so their cost tracks what is VISIBLE
/// rather than what is loaded — a loaded-but-off-screen region is rejected by
/// one box test instead of being walked, and the work each survivor causes
/// (light sampling, row building, the copies downstream) is never paid for
/// something that will not be drawn.
#[derive(Copy, Clone, Debug)]
pub struct ViewVolume {
    frustum: Frustum,
    /// The frustum's planes are expressed relative to this origin — the
    /// renderer keeps view coordinates small for float precision, so boxes
    /// have to be rebased the same way before testing.
    origin: Vec3,
    eye: Vec3,
    cull_dist_sq: f32,
}

impl ViewVolume {
    pub fn new(frustum: Frustum, origin: Vec3, eye: Vec3, cull_dist: f32) -> Self {
        Self {
            frustum,
            origin,
            eye,
            cull_dist_sq: cull_dist * cull_dist,
        }
    }

    /// Admits everything, for callers that have no camera (headless tools,
    /// tests) or none yet.
    pub fn unbounded() -> Self {
        Self::new(Frustum::permissive(), Vec3::ZERO, Vec3::ZERO, f32::INFINITY)
    }

    /// The camera position, for distance ordering by the same callers that cull.
    #[inline]
    pub fn eye(&self) -> Vec3 {
        self.eye
    }

    /// Is any part of the world-space box `[min,max]` drawn this frame?
    #[inline]
    pub fn aabb_visible(&self, min: Vec3, max: Vec3) -> bool {
        self.frustum
            .aabb_visible(min - self.origin, max - self.origin)
            && aabb_distance_sq(self.eye, min, max) <= self.cull_dist_sq
    }
}
