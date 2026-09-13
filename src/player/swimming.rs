//! Player swim intent. Fluid resistance and buoyancy are shared world-data
//! behavior; leaving the fluid follows the shared shore-climb rule.

use super::movement::{GRAVITY, JUMP_V0};
use super::state::{Input, Player, HALF_W, HEIGHT};
use crate::entity::shore::{ShoreClimb, Swimmer};
use petramond_world::block::Aabb;
use petramond_world::collision::DynBox;
use petramond_world::fluid::{Buoyancy, Immersion};

impl Player {
    /// The shore climb a held jump asks for this tick. Climbing out is an
    /// explicit action for players; creatures always try.
    pub(super) fn shore_climb(
        &self,
        swim: Immersion,
        input: Input,
        boxes: &impl Fn(i32, i32, i32) -> &'static [Aabb],
        obstacles: &[DynBox],
    ) -> Option<ShoreClimb> {
        if !input.jump {
            return None;
        }
        Swimmer {
            pos: self.pos,
            vel_y: self.vel.y,
            half_width: HALF_W,
            height: HEIGHT,
            gravity: GRAVITY,
            jump_speed: JUMP_V0,
        }
        .shore_climb(input.wishdir, swim, boxes, obstacles)
    }

    pub(super) fn swim_vertical(
        &mut self,
        dt: f32,
        swim: Immersion,
        input: Input,
        shore: Option<ShoreClimb>,
    ) {
        if let Some(ShoreClimb::Launch(speed)) = shore {
            self.vel.y = self.vel.y.max(speed);
            self.jumping = true;
            return;
        }
        self.vel.y = swim.vertical_velocity(self.vel.y, self.pos.y, Buoyancy::Swim, input.jump, dt);
        self.jumping = false;
    }
}
