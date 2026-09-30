use crate::strike::{self, Aim};
use mod_sdk::*;

const HAND_DROP: f32 = 0.1;

const HAND_RIGHT: f32 = 0.35;

const CONVERGE_FAR: f32 = 48.0;

pub fn from_hand(state: &PlayerSnapshot, target: Option<f32>) -> ([f64; 3], [f32; 3]) {
    let aim = Aim::of(state);
    let nock = [
        -aim.across[0] * HAND_RIGHT,
        -HAND_DROP,
        -aim.across[2] * HAND_RIGHT,
    ];
    let reach = target.unwrap_or(CONVERGE_FAR).max(1.0);
    let to = [
        aim.forward[0] * reach - nock[0],
        aim.forward[1] * reach - nock[1],
        aim.forward[2] * reach - nock[2],
    ];
    let from = strike::offset_by(aim.eye, nock);
    let len = (to[0] * to[0] + to[1] * to[1] + to[2] * to[2])
        .sqrt()
        .max(1e-6);
    (from, [to[0] / len, to[1] / len, to[2] / len])
}

pub fn ray_box(from: [f64; 3], dir: [f32; 3], (min, max): strike::Box3) -> Option<f32> {
    let (min, max) = (strike::relative(min, from), strike::relative(max, from));
    let from = [0.0f32; 3];
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

pub(crate) fn crosshair_reach(me: PlayerId, state: &PlayerSnapshot) -> Option<f32> {
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
