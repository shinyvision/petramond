//! Fly camera: yaw/pitch orientation, WASD/space/shift translation.

use crate::mathh::{Mat4, Vec3, Vec4};
use crate::chunk::CHUNK_SY;

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
        Self { planes: [Vec4::new(0.0, 0.0, 0.0, 1.0); 6] }
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

pub struct Camera {
    pub pos: Vec3,
    pub yaw: f32,   // around +Y, radians
    pub pitch: f32, // around +X, radians, clamped ~= [-1.55, 1.55]
    pub fov_y: f32,
    pub aspect: f32,
    pub near: f32,
    pub far: f32,
}

impl Camera {
    pub fn new(pos: Vec3, aspect: f32) -> Self {
        Self {
            pos,
            yaw: 0.0,
            pitch: -0.4,
            fov_y: 70f32.to_radians(),
            aspect,
            near: 0.1,
            far: 32.0 * 17.0 * 24 as f32 * 2.0 * 2f32, // large enough for fog
        }
    }

    pub fn forward(&self) -> Vec3 {
        let cp = self.pitch.cos();
        Vec3::new(
            self.yaw.sin() * cp,
            self.pitch.sin(),
            self.yaw.cos() * cp,
        ).normalize()
    }

    pub fn right(&self) -> Vec3 {
        // Right-handed: right = U x forward. Pitch contributes no horizontal
        // component, so we derive from yaw-only forward (sin(yaw), 0, cos(yaw)),
        // giving right = (cos(yaw), 0, -sin(yaw)). Inverted because our forward
        // uses +Z at yaw=0 (vs the -Z convention), so negate to keep right =
        // screen-right when facing forward.
        Vec3::new(-self.yaw.cos(), 0.0, self.yaw.sin()).normalize()
    }

    pub fn up(&self) -> Vec3 { Vec3::Y }

    pub fn move_by(&mut self, delta: Vec3) {
        self.pos += delta;
        // Soft clamp to world bounds (allow flying above).
        self.pos.y = self.pos.y.clamp(-8.0, (CHUNK_SY as f32) * 1.5);
    }

    pub fn rotate(&mut self, dyaw: f32, dpitch: f32) {
        self.yaw += dyaw;
        self.pitch = (self.pitch + dpitch).clamp(-1.553_343, 1.553_343);
    }

    pub fn view(&self) -> Mat4 {
        let fwd = self.forward();
        let target = self.pos + fwd;
        Mat4::look_at_rh(self.pos, target, Vec3::Y)
    }

    pub fn proj(&self) -> Mat4 {
        Mat4::perspective_rh(self.fov_y, self.aspect, self.near, self.far)
    }

    pub fn view_proj(&self) -> Mat4 { self.proj() * self.view() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frustum_keeps_front_culls_behind_and_sides() {
        // Camera at origin-ish, looking toward +Z (forward at yaw=0,pitch=0).
        let mut cam = Camera::new(Vec3::new(0.0, 80.0, 0.0), 16.0 / 9.0);
        cam.yaw = 0.0;
        cam.pitch = 0.0;
        let f = Frustum::from_view_proj(cam.view_proj());
        let chunk = |x: f32, z: f32| {
            (Vec3::new(x, 72.0, z), Vec3::new(x + 16.0, 88.0, z + 16.0))
        };
        // Directly ahead (+Z): visible.
        let (mn, mx) = chunk(-8.0, 40.0);
        assert!(f.aabb_visible(mn, mx), "chunk ahead should be visible");
        // Behind the camera (-Z): culled.
        let (mn, mx) = chunk(-8.0, -64.0);
        assert!(!f.aabb_visible(mn, mx), "chunk behind should be culled");
        // Far to the side at camera depth: culled.
        let (mn, mx) = chunk(400.0, -8.0);
        assert!(!f.aabb_visible(mn, mx), "chunk 90° to the side should be culled");
        // The permissive frustum culls nothing.
        let (mn, mx) = chunk(-8.0, -64.0);
        assert!(Frustum::permissive().aabb_visible(mn, mx));
    }
}