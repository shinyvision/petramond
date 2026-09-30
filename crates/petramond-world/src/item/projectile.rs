pub const PROJECTILE_DATA_KEY: &str = "petramond:projectile";

pub const DEFAULT_GRAVITY: f32 = 20.0;

#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Projectile {
    pub gravity: f32,
    pub drag: f32,
    pub sticks: bool,
    pub sprite_spin: f32,
    pub sprite_tilt: f32,
}

impl Default for Projectile {
    fn default() -> Self {
        Projectile {
            gravity: DEFAULT_GRAVITY,
            drag: 0.0,
            sticks: false,
            sprite_spin: 0.0,
            sprite_tilt: 0.0,
        }
    }
}

impl Projectile {
    #[inline]
    pub fn drag_factor(self, dt: f32) -> f32 {
        (1.0 - self.drag).max(0.0).powf(dt)
    }
}
