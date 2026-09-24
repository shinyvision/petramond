//! Frustum and view-volume culling primitives, shared by world gathers
//! (streaming, draws, emitters) and the renderer. Pure math over the loaded
//! world — no camera state, no GPU types.

use crate::mathh::{IVec3, Mat4, Vec3, Vec4};
use petramond_math::world_pos::WorldPos;

#[cfg(test)]
mod tests;

/// View frustum as 6 inward-facing planes, for viewspace (frustum) culling.
/// Each plane is `(a,b,c,d)` with the convention `a·x + b·y + c·z + d >= 0`
/// inside. Extracted from a view-projection matrix (Gribb–Hartmann).
#[derive(Copy, Clone, Debug)]
pub struct Frustum {
    planes: [Vec4; 6],
}

/// Where a box sits relative to a frustum (see [`Frustum::aabb_containment`]).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Containment {
    Outside,
    Intersect,
    Inside,
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
    ///
    /// `#[inline]` because the terrain planner calls this once per loaded
    /// column and once per section of every visible one — thousands of times
    /// per frame, across a crate boundary. Left to a cross-crate call it costs
    /// a real call plus a 96-byte copy of the plane set each time, which
    /// measured as the single largest item in the frame's draw planning.
    /// How the axis-aligned box `[min,max]` sits against the frustum.
    /// [`Containment::Inside`] means every point of the box is in front of
    /// every plane, so nothing the box encloses can be culled — a caller
    /// walking a hierarchy can then stop testing the frustum below it and
    /// keep only whatever range test it has of its own.
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
        // Centre/extent form of the same positive-vertex test: the positive
        // vertex is `c + sign(n) * e`, so its signed distance is
        // `dot(n, c) + dot(|n|, e) + w`. Selecting per component instead costs
        // three data-dependent branches per plane — eighteen per box — which
        // the branch predictor cannot learn, and this is the single most
        // executed operation in a frame (once per loaded column, once per
        // section of every visible one).
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
    /// The frustum's planes are expressed relative to this integer origin —
    /// the renderer keeps view coordinates small for float precision, so
    /// boxes are rebased the same way before testing.
    origin: IVec3,
    /// The camera, relative to `origin`.
    eye: Vec3,
    cull_dist_sq: f32,
    /// Presented pixels one world block spans at one block of distance —
    /// `0.5 * screen_height / tan(fov_y / 2)`. Divided by a distance it gives
    /// the on-screen size of anything out there, which is how a gather of
    /// SMALL things decides what is too far to be seen at all.
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

    /// Admits everything, for callers that have no camera (headless tools,
    /// tests) or none yet.
    pub fn unbounded() -> Self {
        Self::new(
            Frustum::permissive(),
            IVec3::ZERO,
            WorldPos::ZERO,
            f32::INFINITY,
            f32::MAX,
        )
    }

    /// The camera position, for distance ordering by the same callers that cull.
    #[inline]
    pub fn eye(&self) -> WorldPos {
        WorldPos::block_min(self.origin) + self.eye
    }

    /// Is any part of the world-space box `[min,max]` drawn this frame?
    #[inline]
    pub fn aabb_visible(&self, min: WorldPos, max: WorldPos) -> bool {
        let (lo, hi) = (min.relative_to(self.origin), max.relative_to(self.origin));
        self.frustum.aabb_visible(lo, hi) && aabb_distance_sq(self.eye, lo, hi) <= self.cull_dist_sq
    }

    /// Could a detail of world-space size `size` sitting inside `[min,max]`
    /// cover a whole presented pixel? A gather that produces detail far below
    /// a cell in size (particles) rejects on this FIRST: sub-pixel detail is
    /// invisible at any resolution, so building it is pure cost, and testing
    /// the box a whole section's worth of it lives in rejects thousands of
    /// candidates on one distance compare.
    ///
    /// This deliberately counts PRESENTED pixels, not the supersampled scene's
    /// — anti-aliasing exists to smooth what is drawn, not to reveal specks
    /// the display cannot resolve.
    #[inline]
    pub fn covers_a_pixel(&self, min: WorldPos, max: WorldPos, size: f32) -> bool {
        let reach = size * self.pixel_scale;
        let (lo, hi) = (min.relative_to(self.origin), max.relative_to(self.origin));
        aabb_distance_sq(self.eye, lo, hi) <= reach * reach
    }
}
