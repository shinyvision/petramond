//! First-person player: AABB physics with gravity/jump, swept voxel collision,
//! spectator noclip movement, and a block raycast used for break/place.
//!
//! The player is a 0.6 × 1.8 × 0.6 box. `pos` is the *feet centre*: x/z are the
//! horizontal centre of the box and y is its bottom. The camera eye sits `EYE`
//! above the feet. Horizontal movement decouples acceleration from friction.
//! While a direction is held, the velocity ramps toward the wish velocity (input
//! direction × speed). On the ground this is a snappy redirect toward wish×speed
//! (responsive starts, stops, and turns); in the air it is a gentle, *additive*
//! nudge along the input direction that tops you up to walk speed but never
//! brakes, with total air speed capped at what you launched with — so a jump
//! keeps its momentum and input can steer the arc but neither brakes nor pumps it
//! up (no wall-scrape speed exploit). With no input, *friction* alone decays the velocity
//! toward zero: friction is purely how fast you slow down — 0 keeps motion
//! forever, 1 stops it instantly — and it never gates how fast you speed up.
//! Ground friction is high (quick stop), air friction low (a long coast). The
//! decay is frame-rate independent; the ramp's rate is too, though the exact
//! frame it reaches top speed can vary by up to one sub-step. Gravity pulls the
//! player down — eased near the jump apex for a softer arc — and Space jumps.
//! Spectator mode bypasses gravity and collision entirely, moving through the
//! full 3-D wish direction.
//! A grounded player auto-steps up a half-block ledge (a slab / a bbmodel block's low
//! edge) via the shared `collision::step_horizontal` (`STEP_HEIGHT = 0.5`); a full block
//! is still a jump-to-climb wall (`JUMP_V0` clears ~1.26 blocks). Step-up is gated on being
//! grounded, so it never lifts a falling/jumping player.

mod collision;
mod interaction;
mod movement;
mod state;

#[cfg(test)]
mod tests;

pub(crate) use interaction::ray_vs_aabb;
pub use interaction::{RaycastHit, REACH};
pub use state::{Input, Player, PlayerMode, DT_MAX, EYE, HALF_W, HEIGHT};
