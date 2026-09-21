//! Moving the body within its cell: every nudge goes through here.

use mod_sdk::*;

use super::Body;
use crate::geometry::feet_of;

/// Stand still this tick. A falling body keeps the sideways speed it left the
/// ground with, so steering it toward the centre only makes it wiggle down a
/// pillar.
pub(super) fn hold_still(body: &Body) {
    mob_drive(body.id, [0.0, 0.0], None);
}

/// Leave the ground straight up at `speed`. Never with a sideways speed: it
/// carries through the fall, past the centre of the cell left.
pub(super) fn jump(body: &Body, speed: f32) {
    launch(body, [0.0, speed, 0.0]);
}

/// Leave the ground at `velocity`.
pub(super) fn launch(body: &Body, velocity: [f32; 3]) {
    mob_drive_velocity(body.id, velocity, None);
}

/// Steer a body in the air, as a player steers a jump.
pub(super) fn steer(body: &Body, velocity: [f32; 2]) {
    mob_drive(body.id, velocity, None);
}

/// Nudge the body toward the centre of its cell for this tick.
pub(super) fn centre(body: &Body) {
    centre_on(body, body.cell);
}

/// Ease the body toward the feet position `to` within its own cell.
fn ease_to(body: &Body, to: [f64; 3]) {
    let [dx, dz] = [to[0] - body.pos[0], to[2] - body.pos[2]];
    let v = |d: f64| (d * 6.0).clamp(-1.5, 1.5) as f32;
    mob_step(body.id, [v(dx), v(dz)]);
}

/// Ease the body out toward the edge of its cell nearest `toward`.
pub(super) fn lean(body: &Body, toward: [i32; 3]) {
    let centre = feet_of(body.cell);
    let side = |a: usize| f64::from((toward[a] - body.cell[a]).signum()) * 0.7;
    ease_to(body, [centre[0] + side(0), centre[1], centre[2] + side(2)]);
}

/// Nudge the body toward the centre of `cell` for this tick.
pub(super) fn centre_on(body: &Body, cell: [i32; 3]) {
    let target = feet_of(cell);
    let [dx, dz] = [target[0] - body.pos[0], target[2] - body.pos[2]];
    let v = |d: f64| (d * 8.0).clamp(-2.0, 2.0) as f32;
    // Its own legs carry it: a shuffle is a walk, not a slide.
    mob_step(body.id, [v(dx), v(dz)]);
}
