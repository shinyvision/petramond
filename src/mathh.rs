//! Misc math helpers not covered by glam.

pub use glam::{IVec2, IVec3, Vec2, Vec3, Vec4, Mat4, Quat};

pub fn lerp(a: f32, b: f32, t: f32) -> f32 { a + (b - a) * t }

pub fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

pub fn smoothstep01(x: f32) -> f32 { smoothstep(0.0, 1.0, x) }

pub fn clamp(x: f32, lo: f32, hi: f32) -> f32 { x.clamp(lo, hi) }