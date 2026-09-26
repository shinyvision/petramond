//! The local body's motion as the first-person animator reads it.

use std::f32::consts::{PI, TAU};

use super::body_pose::{movement_medium, MovementMedium};
use super::Game;
use crate::animation::{AimTarget, LocalMotion};

/// How fast the look turns, from one frame's look to the next.
#[derive(Default)]
pub(super) struct LookRate {
    last: Option<(f32, f32)>,
    /// Degrees per second, rightward and upward.
    rates: (f32, f32),
}

impl LookRate {
    /// Degrees per second, rightward and upward.
    pub fn rates(&self) -> (f32, f32) {
        self.rates
    }

    pub fn advance(&mut self, dt: f32, yaw: f32, pitch: f32) {
        self.rates = match self.last {
            Some((last_yaw, last_pitch)) if dt > 0.0 => {
                let turn = (yaw - last_yaw + PI).rem_euclid(TAU) - PI;
                // Yaw grows toward the view's left; a rightward turn reads positive.
                (
                    (-turn / dt).to_degrees(),
                    ((pitch - last_pitch) / dt).to_degrees(),
                )
            }
            _ => (0.0, 0.0),
        };
        self.last = Some((yaw, pitch));
    }
}

impl Game {
    /// This frame's motion for the first-person animator; `hurt` is the
    /// seconds of hurt shake left.
    pub fn local_motion(&self, hurt: f32) -> LocalMotion {
        let player = &self.local.player;
        let (yaw, pitch) = (player.yaw, player.pitch);
        let (yaw_rate, pitch_rate) = self.local.camera_rig.look_rates();
        let vel = player.vel;
        let forward = glam::Vec3::new(yaw.sin(), 0.0, yaw.cos());
        let right = glam::Vec3::new(-yaw.cos(), 0.0, yaw.sin());
        let medium = movement_medium(self.replica.world.data(), player.pos);
        let (stride, stride_weight) = self.local.camera_rig.stride();
        let target = if self.local.targeted_mob.is_some() || self.local.targeted_player.is_some() {
            AimTarget::Creature
        } else if self.local.look.is_some() {
            AimTarget::Block
        } else {
            AimTarget::Nothing
        };
        LocalMotion {
            speed: glam::Vec3::new(vel.x, 0.0, vel.z).length(),
            forward: vel.dot(forward),
            strafe: vel.dot(right),
            vertical: vel.y,
            grounded: player.on_ground,
            sneaking: self.local.predicted_input.sneak,
            sprinting: self.local.predicted_input.sprint,
            swimming: medium == MovementMedium::Swimming,
            climbing: medium == MovementMedium::Climbing,
            pitch: pitch.to_degrees(),
            yaw_rate,
            pitch_rate,
            stride,
            stride_weight,
            hurt,
            target,
        }
    }
}
