//! The loose: where an arrow leaves from, which way, how fast — and the
//! server's act of spending one from the pack and launching it.

use super::rows::{BowRow, Rows};
use crate::strike::{self, Aim};
use mod_sdk::*;

/// Where the arrow leaves from, below the eye (blocks): a nocked arrow sits
/// at the cheek, not in the pupil, and starting it a touch lower keeps the
/// first frame of flight out of the camera.
const NOCK_DROP: f32 = 0.1;

/// ...and to the RIGHT of the eye (blocks): the arrow sits at the bow in
/// the main hand, not on the nose, and a shot leaving off-centre is one the
/// archer can watch fly instead of losing it behind the crosshair.
const NOCK_RIGHT: f32 = 0.35;

/// How far out the shot converges on the crosshair when nothing is under
/// it (blocks): far enough that the offset nock is a hair's angle.
const CONVERGE_FAR: f32 = 48.0;

/// Where an arrow leaves from and which way: beside the eye, a touch low
/// and to the archer's RIGHT (the negative of the aim's `across` — the
/// world is right-handed with Y up, so facing +Z puts +X on the LEFT; the
/// first cut had it mirrored and shot from the left), aimed at the point
/// the crosshair rests on — `target` blocks along the look, the first body
/// or block there, or [`CONVERGE_FAR`] with nothing under it — so a shot
/// at a zombie a few blocks off lands on the zombie, not beside it.
pub fn nock(state: &PlayerSnapshot, target: Option<f32>) -> ([f32; 3], [f32; 3]) {
    let aim = Aim::of(state);
    let from = [
        aim.eye[0] - aim.across[0] * NOCK_RIGHT,
        aim.eye[1] - NOCK_DROP,
        aim.eye[2] - aim.across[2] * NOCK_RIGHT,
    ];
    let reach = target.unwrap_or(CONVERGE_FAR).max(1.0);
    let at = [
        aim.eye[0] + aim.forward[0] * reach,
        aim.eye[1] + aim.forward[1] * reach,
        aim.eye[2] + aim.forward[2] * reach,
    ];
    let to = [at[0] - from[0], at[1] - from[1], at[2] - from[2]];
    let len = (to[0] * to[0] + to[1] * to[1] + to[2] * to[2])
        .sqrt()
        .max(1e-6);
    (from, [to[0] / len, to[1] / len, to[2] / len])
}

/// Where a ray from `from` along unit `dir` enters the box `(min, max)`:
/// the distance, or `None` for a miss (a start inside counts at 0).
pub fn ray_box(from: [f32; 3], dir: [f32; 3], (min, max): strike::Box3) -> Option<f32> {
    let (mut near, mut far) = (0.0f32, f32::INFINITY);
    for axis in 0..3 {
        if dir[axis].abs() < 1e-9 {
            if from[axis] < min[axis] || from[axis] > max[axis] {
                return None;
            }
            continue;
        }
        let a = (min[axis] - from[axis]) / dir[axis];
        let b = (max[axis] - from[axis]) / dir[axis];
        near = near.max(a.min(b));
        far = far.min(a.max(b));
        if near > far {
            return None;
        }
    }
    Some(near)
}

/// SERVER: how far along the look the crosshair rests — the nearest of
/// the first collidable block and the first body (any mob, any other
/// player) on the ray, `None` for open air. What the shot converges on.
fn crosshair_reach(me: PlayerId, state: &PlayerSnapshot) -> Option<f32> {
    let aim = Aim::of(state);
    let (eye, dir) = (aim.eye, aim.forward);
    let mut best =
        raycast(eye, dir, RAYCAST_MAX_DISTANCE, RayFilter::Collidable).map(|hit| hit.distance);
    let mut consider = |d: f32| {
        if best.is_none_or(|b| d < b) {
            best = Some(d);
        }
    };
    for mob in mobs_in_radius(eye, RAYCAST_MAX_DISTANCE) {
        for b in strike::mob_boxes(&mob) {
            if let Some(d) = ray_box(eye, dir, b) {
                consider(d);
            }
        }
    }
    for entry in players() {
        if entry.id == me || entry.state.spectator {
            continue;
        }
        if let Some(d) = ray_box(eye, dir, strike::player_box(&entry.state)) {
            consider(d);
        }
    }
    best
}

/// SERVER: the draw of `bow` came off at `ticks` — spend the first arrow
/// row the archer carries and launch it from the eye along the look. No
/// arrow, no shot: the draw was still shown (both sides agree on that by
/// construction), it simply had nothing to loose.
pub fn loose(rows: &Rows, bow: &BowRow, me: PlayerId, state: &PlayerSnapshot, ticks: u32) {
    let Some((arrow, stack)) = rows
        .arrows
        .iter()
        .find_map(|arrow| take_item(me, &arrow.name, 1, None).map(|stack| (arrow, stack)))
    else {
        return;
    };
    let (from, dir) = nock(state, crosshair_reach(me, state));
    let speed = bow.draw.launch_speed(ticks);
    // The arrow leaves with the archer's own motion on top of the draw's:
    // a shot loosed on the run is not slower than the runner.
    let vel = [
        dir[0] * speed + state.vel[0],
        dir[1] * speed + state.vel[1],
        dir[2] * speed + state.vel[2],
    ];
    let data: Vec<(&str, &[u8])> = stack
        .data
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_slice()))
        .collect();
    launch_item(&arrow.name, from, vel, Some(EntityRef::Player(me)), &data);
}
