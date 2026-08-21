//! Fly camera: yaw/pitch orientation, WASD/space/shift translation.

use petramond::world::RENDER_DIST;
use petramond_math::math::{Mat4, Vec3};
use petramond_world::chunk::CHUNK_SX;

/// Far clip plane (world blocks). The far plane only has to sit beyond the point
/// where the world fully fogs out so nothing still visible gets far-plane-culled,
/// and it must comfortably clear the whole loaded world for the rare clear-air
/// long sightline (e.g. across an unfogged gap). We size it off the real cause —
/// the loaded-world extent — rather than a bare magic number.
///
/// `render::uniforms::fog_range` (end = render distance in blocks) is where
/// visibility ends; this far plane is two orders of magnitude past it, so the fog
/// end is never the limiting factor. (Kept out of a direct reference to avoid
/// pointing the low-level camera at the high-level render layer — the invariant
/// to hold is simply `CAMERA_FAR >= fog end` at the maximum render distance.)
///
/// Diameter, in blocks, of the chunks loaded around the camera
/// (`2 * RENDER_DIST` chunks across, `CHUNK_SX` blocks each).
const LOADED_WORLD_DIAMETER: f32 = (2 * RENDER_DIST as usize * CHUNK_SX) as f32;
/// Generous depth-buffer headroom so the far plane clears the loaded world many
/// times over. Larger than necessary on purpose (the value predates this
/// derivation); kept as-is so depth precision is unchanged.
const FAR_HEADROOM: f32 = 102.0;
/// `512 * 102 == 52224` — identical to the previous magic
/// `32.0 * 17.0 * 24.0 * 2.0 * 2.0`.
const CAMERA_FAR: f32 = LOADED_WORLD_DIAMETER * FAR_HEADROOM;

pub use petramond_world::view_volume::{aabb_distance_sq, Frustum, ViewVolume};

#[derive(Clone)]
pub struct Camera {
    pub pos: Vec3,
    // Orientation mirrored from the player's look each frame — `player::Player`
    // owns the authoritative yaw/pitch (and the pitch clamp). Radians.
    pub yaw: f32,   // around +Y
    pub pitch: f32, // up/down
    pub fov_y: f32,
    pub aspect: f32,
    pub near: f32,
    pub far: f32,
}

impl Camera {
    pub fn new(pos: Vec3, aspect: f32) -> Self {
        Self {
            pos,
            // Overwritten each frame by the player's look (see the field docs);
            // this default only applies to a standalone camera, e.g. in tests.
            yaw: 0.0,
            pitch: 0.0,
            fov_y: 70f32.to_radians(),
            aspect,
            near: 0.1,
            far: CAMERA_FAR,
        }
    }

    pub fn forward(&self) -> Vec3 {
        let cp = self.pitch.cos();
        Vec3::new(self.yaw.sin() * cp, self.pitch.sin(), self.yaw.cos() * cp).normalize()
    }

    pub fn right(&self) -> Vec3 {
        // Right-handed: right = U x forward. Pitch contributes no horizontal
        // component, so we derive from yaw-only forward (sin(yaw), 0, cos(yaw)),
        // giving right = (cos(yaw), 0, -sin(yaw)). Inverted because our forward
        // uses +Z at yaw=0 (vs the -Z convention), so negate to keep right =
        // screen-right when facing forward.
        Vec3::new(-self.yaw.cos(), 0.0, self.yaw.sin()).normalize()
    }

    pub fn proj(&self) -> Mat4 {
        Mat4::perspective_rh(self.fov_y, self.aspect, self.near, self.far)
    }

    /// Absolute (non-camera-relative) view matrix; live rendering uses the
    /// camera-relative equivalent in the renderer instead.
    #[cfg(test)]
    pub fn view(&self) -> Mat4 {
        let fwd = self.forward();
        let target = self.pos + fwd;
        Mat4::look_at_rh(self.pos, target, Vec3::Y)
    }

    #[cfg(test)]
    pub fn view_proj(&self) -> Mat4 {
        self.proj() * self.view()
    }
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
        let chunk = |x: f32, z: f32| (Vec3::new(x, 72.0, z), Vec3::new(x + 16.0, 88.0, z + 16.0));
        // Directly ahead (+Z): visible.
        let (mn, mx) = chunk(-8.0, 40.0);
        assert!(f.aabb_visible(mn, mx), "chunk ahead should be visible");
        // Behind the camera (-Z): culled.
        let (mn, mx) = chunk(-8.0, -64.0);
        assert!(!f.aabb_visible(mn, mx), "chunk behind should be culled");
        // Far to the side at camera depth: culled.
        let (mn, mx) = chunk(400.0, -8.0);
        assert!(
            !f.aabb_visible(mn, mx),
            "chunk 90° to the side should be culled"
        );
        // The permissive frustum culls nothing.
        let (mn, mx) = chunk(-8.0, -64.0);
        assert!(Frustum::permissive().aabb_visible(mn, mx));
    }
}
