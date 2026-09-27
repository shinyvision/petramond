use std::ops::{Add, AddAssign, Sub, SubAssign};

use glam::{DVec3, IVec3, Vec3};

#[derive(Copy, Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct WorldPos {
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

impl WorldPos {
    pub const ZERO: WorldPos = WorldPos::new(0.0, 0.0, 0.0);

    pub const fn new(x: f64, y: f64, z: f64) -> WorldPos {
        WorldPos { x, y, z }
    }

    pub const fn from_array([x, y, z]: [f64; 3]) -> WorldPos {
        WorldPos { x, y, z }
    }

    pub const fn to_array(self) -> [f64; 3] {
        [self.x, self.y, self.z]
    }

    pub fn from_dvec3(v: DVec3) -> WorldPos {
        WorldPos::new(v.x, v.y, v.z)
    }

    pub fn as_dvec3(self) -> DVec3 {
        DVec3::new(self.x, self.y, self.z)
    }

    pub fn block_min(b: IVec3) -> WorldPos {
        WorldPos::new(f64::from(b.x), f64::from(b.y), f64::from(b.z))
    }

    pub fn block_center(b: IVec3) -> WorldPos {
        WorldPos::block_min(b) + Vec3::splat(0.5)
    }

    pub fn block(self) -> IVec3 {
        IVec3::new(
            self.x.floor() as i32,
            self.y.floor() as i32,
            self.z.floor() as i32,
        )
    }

    pub fn relative_to(self, anchor: IVec3) -> Vec3 {
        Vec3::new(
            (self.x - f64::from(anchor.x)) as f32,
            (self.y - f64::from(anchor.y)) as f32,
            (self.z - f64::from(anchor.z)) as f32,
        )
    }

    pub fn distance_squared(self, other: WorldPos) -> f64 {
        (self.as_dvec3() - other.as_dvec3()).length_squared()
    }

    pub fn distance(self, other: WorldPos) -> f64 {
        self.distance_squared(other).sqrt()
    }

    pub fn lerp(self, to: WorldPos, t: f32) -> WorldPos {
        let t = f64::from(t);
        WorldPos::new(
            self.x + (to.x - self.x) * t,
            self.y + (to.y - self.y) * t,
            self.z + (to.z - self.z) * t,
        )
    }

    pub fn is_finite(self) -> bool {
        self.x.is_finite() && self.y.is_finite() && self.z.is_finite()
    }

    pub fn with_y(self, y: f64) -> WorldPos {
        WorldPos { y, ..self }
    }
}

impl Add<Vec3> for WorldPos {
    type Output = WorldPos;

    fn add(self, offset: Vec3) -> WorldPos {
        WorldPos::new(
            self.x + f64::from(offset.x),
            self.y + f64::from(offset.y),
            self.z + f64::from(offset.z),
        )
    }
}

impl AddAssign<Vec3> for WorldPos {
    fn add_assign(&mut self, offset: Vec3) {
        *self = *self + offset;
    }
}

impl Sub<Vec3> for WorldPos {
    type Output = WorldPos;

    fn sub(self, offset: Vec3) -> WorldPos {
        self + -offset
    }
}

impl SubAssign<Vec3> for WorldPos {
    fn sub_assign(&mut self, offset: Vec3) {
        *self = *self - offset;
    }
}

impl Sub for WorldPos {
    type Output = Vec3;

    fn sub(self, rhs: WorldPos) -> Vec3 {
        Vec3::new(
            (self.x - rhs.x) as f32,
            (self.y - rhs.y) as f32,
            (self.z - rhs.z) as f32,
        )
    }
}

#[cfg(test)]
mod tests;
