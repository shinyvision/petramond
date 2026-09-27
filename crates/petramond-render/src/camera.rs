use petramond::world::RENDER_DIST;
use petramond_math::math::{Mat4, Vec3};
use petramond_world::chunk::CHUNK_SX;

const LOADED_WORLD_DIAMETER: f32 = (2 * RENDER_DIST as usize * CHUNK_SX) as f32;
const FAR_HEADROOM: f32 = 102.0;
const CAMERA_FAR: f32 = LOADED_WORLD_DIAMETER * FAR_HEADROOM;

pub use petramond_math::view_volume::{aabb_distance_sq, Containment, Frustum, ViewVolume};

#[derive(Clone)]
pub struct Camera {
    pub pos: petramond_math::world_pos::WorldPos,
    pub yaw: f32,
    pub pitch: f32,
    pub roll: f32,
    pub fov_y: f32,
    pub aspect: f32,
    pub near: f32,
    pub far: f32,
}

impl Camera {
    pub fn new(pos: petramond_math::world_pos::WorldPos, aspect: f32) -> Self {
        Self {
            pos,
            yaw: 0.0,
            pitch: 0.0,
            roll: 0.0,
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
        Vec3::new(-self.yaw.cos(), 0.0, self.yaw.sin()).normalize()
    }

    pub fn up(&self) -> Vec3 {
        let forward = self.forward();
        let right = self.right();
        let level_up = right.cross(forward).normalize();
        level_up * self.roll.cos() + right * self.roll.sin()
    }

    pub fn proj(&self) -> Mat4 {
        Mat4::perspective_rh(self.fov_y, self.aspect, self.near, self.far)
    }

    #[cfg(test)]
    pub fn view(&self) -> Mat4 {
        let eye = self.pos.relative_to(glam::IVec3::ZERO);
        Mat4::look_at_rh(eye, eye + self.forward(), self.up())
    }

    #[cfg(test)]
    pub fn view_proj(&self) -> Mat4 {
        self.proj() * self.view()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use petramond_math::world_pos::WorldPos;

    #[test]
    fn frustum_keeps_front_culls_behind_and_sides() {
        let mut cam = Camera::new(WorldPos::new(0.0, 80.0, 0.0), 16.0 / 9.0);
        cam.yaw = 0.0;
        cam.pitch = 0.0;
        let f = Frustum::from_view_proj(cam.view_proj());
        let chunk = |x: f32, z: f32| (Vec3::new(x, 72.0, z), Vec3::new(x + 16.0, 88.0, z + 16.0));
        let (mn, mx) = chunk(-8.0, 40.0);
        assert!(f.aabb_visible(mn, mx), "chunk ahead should be visible");
        let (mn, mx) = chunk(-8.0, -64.0);
        assert!(!f.aabb_visible(mn, mx), "chunk behind should be culled");
        let (mn, mx) = chunk(400.0, -8.0);
        assert!(
            !f.aabb_visible(mn, mx),
            "chunk 90° to the side should be culled"
        );
        let (mn, mx) = chunk(-8.0, -64.0);
        assert!(Frustum::permissive().aabb_visible(mn, mx));
    }

    #[test]
    fn a_pitch_swept_past_vertical_never_flips_the_image_in_one_step() {
        let mut cam = Camera::new(WorldPos::new(0.0, 80.0, 0.0), 1.0);
        let screen_right = |cam: &Camera| cam.view().row(0).truncate();
        let mut prev = screen_right(&cam);
        for step in 0..=300 {
            cam.pitch = 1.5 + step as f32 * 0.001;
            let now = screen_right(&cam);
            assert!(now.dot(prev) > 0.99, "flipped at pitch {}", cam.pitch);
            prev = now;
        }
    }
}
