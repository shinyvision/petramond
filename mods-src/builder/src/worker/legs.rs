use crate::host::prelude::*;

use super::Body;
use crate::geometry::feet_of;

pub(super) fn hold_still(body: &Body) {
    mob_drive(body.id, [0.0, 0.0], None);
}

pub(super) fn jump(body: &Body, speed: f32) {
    launch(body, [0.0, speed, 0.0]);
}

pub(super) fn launch(body: &Body, velocity: [f32; 3]) {
    mob_drive_velocity(body.id, velocity, None);
}

pub(super) fn steer(body: &Body, velocity: [f32; 2]) {
    mob_drive(body.id, velocity, None);
}

pub(super) fn centre(body: &Body) {
    centre_on(body, body.cell);
}

fn ease_to(body: &Body, to: [f64; 3]) {
    let [dx, dz] = [to[0] - body.pos[0], to[2] - body.pos[2]];
    let v = |d: f64| (d * 6.0).clamp(-1.5, 1.5) as f32;
    mob_step(body.id, [v(dx), v(dz)]);
}

pub(super) fn lean(body: &Body, toward: [i32; 3]) {
    let centre = feet_of(body.cell);
    let side = |a: usize| f64::from((toward[a] - body.cell[a]).signum()) * 0.7;
    ease_to(body, [centre[0] + side(0), centre[1], centre[2] + side(2)]);
}

pub(super) fn centre_on(body: &Body, cell: [i32; 3]) {
    let target = feet_of(cell);
    let [dx, dz] = [target[0] - body.pos[0], target[2] - body.pos[2]];
    let v = |d: f64| (d * 8.0).clamp(-2.0, 2.0) as f32;
    mob_step(body.id, [v(dx), v(dz)]);
}
