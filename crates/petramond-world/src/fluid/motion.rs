use super::{Buoyancy, FluidCurrent, FluidMotion, Immersion};
use crate::mathh::Vec3;

const FRICTION_REF_DT: f32 = 1.0 / 60.0;
const SURFACE_DRAFT: f32 = 0.1;
const SURFACE_FLOAT_RATE: f32 = 6.0;

impl FluidMotion {
    /// Apply this fluid to the horizontal intent of any body controller.
    pub fn horizontal_velocity(self, current: Vec3, desired: Vec3, dt: f32) -> Vec3 {
        let target = Vec3::new(desired.x, 0.0, desired.z) * self.speed_scale;
        let mut velocity = Vec3::new(current.x, 0.0, current.z);
        let speed = velocity.length();
        let target_speed = target.length();
        if self.entry_friction > 0.0 && speed > target_speed {
            let slowed = target_speed + (speed - target_speed) * retain(self.entry_friction, dt);
            velocity *= slowed / speed;
        }
        if target_speed <= 1e-6 {
            velocity *= retain(self.friction, dt);
        } else {
            let delta = target - velocity;
            velocity += delta.clamp_length_max(self.accel * dt);
        }
        Vec3::new(velocity.x, current.y, velocity.z)
    }

    /// Reduce incoming vertical momentum before buoyancy or a swim stroke acts.
    pub fn brake_vertical(self, current: f32, dt: f32) -> f32 {
        if self.entry_friction <= 0.0 {
            return current;
        }
        let bounded = current.clamp(-self.sink, self.rise);
        bounded + (current - bounded) * retain(self.entry_friction, dt)
    }

    /// Body-scaled immersion depth; small and tall creatures use the same rule.
    pub fn probe_height(self, body_height: f32) -> f32 {
        (body_height * self.probe_fraction + self.probe_offset).min(body_height)
    }
}

impl Immersion {
    /// Shared swim, passive sink, neutral drift and surface-float response.
    pub fn vertical_velocity(
        self,
        current: f32,
        feet_y: f32,
        buoyancy: Buoyancy,
        ascending: bool,
        dt: f32,
    ) -> f32 {
        let motion = self.fluid.motion;
        match buoyancy {
            Buoyancy::Swim => {
                let current = motion.brake_vertical(current, dt);
                let target = if ascending { motion.rise } else { -motion.sink };
                current
                    + (target - current)
                        .clamp(-motion.vertical_accel * dt, motion.vertical_accel * dt)
            }
            Buoyancy::Neutral => motion.brake_vertical(current, dt),
            Buoyancy::Surface => {
                let error = self.surface_y - SURFACE_DRAFT - feet_y;
                (error * SURFACE_FLOAT_RATE.min(1.0 / dt.max(1e-6)))
                    .clamp(-motion.sink, motion.rise)
            }
        }
    }
}

impl FluidCurrent {
    /// Add a bounded push without braking a body already moving faster downstream.
    pub fn apply(self, velocity: Vec3, dt: f32) -> Vec3 {
        let speed = self.velocity.length();
        if speed <= 1e-6 || self.accel <= 0.0 {
            return velocity;
        }
        let direction = self.velocity / speed;
        let add = (speed - velocity.dot(direction)).clamp(0.0, self.accel * dt);
        velocity + direction * add
    }
}

fn retain(friction: f32, dt: f32) -> f32 {
    if friction >= 1.0 {
        0.0
    } else {
        (1.0 - friction).powf(dt / FRICTION_REF_DT)
    }
}

#[cfg(test)]
mod tests;
