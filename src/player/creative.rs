use super::collision::Axis;
use super::movement::Surroundings;
use super::{Input, Player, PlayerMode};
use petramond_math::math::Vec3;

/// Creative flight is paced for building within ordinary interaction reach.
pub fn creative_flight_speed(sprint: bool) -> f32 {
    if sprint {
        10.0
    } else {
        6.0
    }
}

impl Player {
    pub fn toggle_creative(&mut self) {
        self.set_mode(if self.is_creative() {
            PlayerMode::Survival
        } else {
            PlayerMode::Creative
        });
    }

    pub fn toggle_creative_flight(&mut self) {
        if !self.abilities().flight {
            return;
        }
        self.set_mode(if self.is_flying() {
            PlayerMode::Creative
        } else {
            PlayerMode::CreativeFlying
        });
    }

    pub(super) fn update_creative_flight(&mut self, dt: f32, env: &Surroundings<'_>, input: Input) {
        let dt = dt.max(0.0);
        if self.escape_geometry(dt, env) {
            return;
        }
        let wish = Vec3::new(
            input.wishdir.x,
            input.jump as u8 as f32 - input.sneak as u8 as f32,
            input.wishdir.z,
        )
        .normalize_or_zero();
        let target = wish * creative_flight_speed(input.sprint);
        let rate = if wish == Vec3::ZERO { 5.0 } else { 12.0 };
        let retain = (-rate * dt).exp();
        // Integrate the exponential exactly so a low frame rate coasts the
        // same distance as a high one, including the first released frame.
        let travel = target * dt + (self.vel - target) * ((1.0 - retain) / rate);
        self.vel = target + (self.vel - target) * retain;
        for (axis, delta, component) in [
            (Axis::Y, travel.y, 1),
            (Axis::X, travel.x, 0),
            (Axis::Z, travel.z, 2),
        ] {
            if self.sweep_boxes_dyn(axis, delta, &env.boxes, env.obstacles) {
                self.vel[component] = 0.0;
            }
        }
        self.on_ground = false;
    }
}
