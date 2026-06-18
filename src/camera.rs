//! Fly camera: yaw/pitch orientation, WASD/space/shift translation.

use crate::mathh::{Mat4, Vec3};
use crate::chunk::CHUNK_SY;

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