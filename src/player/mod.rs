//! First-person player: AABB physics, gravity/jump, swept voxel collision,
//! spectator noclip, block raycast for break/place.
//!
//! Box is 0.6 x 1.8 x 0.6. `pos` is feet centre: x/z centered, y at bottom.
//! Camera eye sits `EYE` above feet.
//!
//! Accel and friction are separate knobs. Hold a direction, velocity ramps
//! toward wish x speed. Ground: snappy redirect, crisp starts/stops/turns.
//! Air: just tops you up toward walk speed, never brakes, total air speed
//! capped at launch speed. So a jump keeps its momentum - steer the arc but
//! can't brake or pump speed, no wall-scrape trick. Release input and
//! friction alone decays velocity toward zero (0 never stops, 1 stops
//! instantly), doesn't touch accel. Ground friction high, air friction low.
//! Decay and ramp are frame-rate independent, but the frame you hit top
//! speed can drift up to one sub-step.
//!
//! Gravity eases near jump apex, softer arc. Spectator skips gravity and
//! collision, just moves freely in 3D.
//!
//! Grounded players auto-step a half-block ledge via `collision::step_horizontal`
//! (`STEP_HEIGHT = 0.5`). Full block still needs a jump (`JUMP_V0` clears ~1.26
//! blocks). Step-up needs solid or fluid support underneath; falling bodies
//! can't step.

mod abilities;
mod collision;
mod creative;
mod interaction;
pub use crate::world::session::PlayerId;

pub mod animator;
mod body_claims;
pub mod model;
mod movement;
pub mod one_shot;
mod progression;
pub mod rigs;
mod state;
mod swimming;

#[cfg(test)]
mod tests;

pub use abilities::PlayerAbilities;
pub use body_claims::{
    AnimatorClaims, AnimatorClock, AnimatorParam, AnimatorPlay, AnimatorValue, BodyClaims,
    BonePose, DeniedActions, ENGINE_CLAIMANT, MOVE_SCALE_DEFAULT, MOVE_SCALE_MAX,
};
pub use creative::creative_flight_speed;
pub use interaction::block_within_reach;
pub use interaction::ray_vs_aabb;
pub use interaction::{RayFilter, RaycastHit, REACH};
pub use movement::{GRAVITY, JUMP_V0, SPECTATOR_SPRINT, SPRINT, SWIM_SPEED, TERMINAL, WALK};
pub use progression::Progression;
pub use rigs::{Presenter, RigId};
pub use state::UseGesture;
pub use state::{
    BedSpawn, Input, Player, PlayerInputSnapshot, PlayerMode, PlayerRosterSnapshot, DT_MAX, EYE,
    HALF_W, HEIGHT, MAX_HEALTH, PITCH_LIMIT,
};
