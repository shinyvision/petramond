//! Inertialization: a pose that must jump (a hard state change, a montage
//! cut in at full weight) jumps, and the difference from where the previous
//! pose was heading decays away on a critically damped spring. Unlike a
//! cross-fade it evaluates only the new pose and carries the old motion's
//! velocity, so an interrupted swing flows into the next instead of mushing.

use std::f32::consts::LN_2;

use super::pose::LocalPose;

const MAX_TURN: f32 = 2000.0;
const MAX_TRAVEL: f32 = 200.0;

pub(crate) fn settle_halflife(settle: f32) -> f32 {
    settle.max(0.0) * 2.0 * LN_2 / 4.744
}

#[derive(Default)]
pub(crate) struct Inertia {
    offset: LocalPose,
    velocity: LocalPose,
    last: LocalPose,
    last_velocity: LocalPose,
    halflife: f32,
    pending: Option<f32>,
    active: bool,
    frame: u64,
}

impl Inertia {
    pub fn trigger(&mut self, halflife: f32) {
        if halflife > 0.0 {
            self.pending = Some(halflife);
        }
    }

    pub fn apply(&mut self, pose: &mut LocalPose, dt: f32, frame: u64) {
        let primed = self.frame + 1 == frame && self.last.len() == pose.len();
        if !primed {
            self.active = false;
            self.pending = None;
        } else if let Some(halflife) = self.pending.take() {
            self.offset.copy_from(&self.last);
            self.offset.add_scaled(&self.last_velocity, dt, None);
            self.offset.add_scaled(pose, -1.0, None);
            self.velocity.copy_from(&self.last_velocity);
            self.halflife = halflife;
            self.active = true;
        } else if self.active {
            self.decay(dt);
        }
        if self.active {
            pose.add_scaled(&self.offset, 1.0, None);
        }

        if !primed {
            self.last_velocity.copy_from(pose);
            self.last_velocity.clear();
        } else if dt > 0.0 {
            self.last_velocity.copy_from(pose);
            self.last_velocity.add_scaled(&self.last, -1.0, None);
            let (turn, travel) = self.last_velocity.channels_mut();
            for v in turn {
                *v = (*v / dt).clamp_length_max(MAX_TURN);
            }
            for v in travel {
                *v = (*v / dt).clamp_length_max(MAX_TRAVEL);
            }
        }
        self.last.copy_from(pose);
        self.frame = frame;
    }

    fn decay(&mut self, dt: f32) {
        let y = 2.0 * LN_2 / self.halflife.max(1e-4);
        let e = (-y * dt).exp();
        let (orot, opos) = self.offset.channels_mut();
        let (vrot, vpos) = self.velocity.channels_mut();
        let mut peak = 0.0f32;
        for (x, v) in orot.iter_mut().chain(opos).zip(vrot.iter_mut().chain(vpos)) {
            let j1 = *v + *x * y;
            *x = (*x + j1 * dt) * e;
            *v = (*v - j1 * y * dt) * e;
            peak = peak
                .max(x.abs().max_element())
                .max(v.abs().max_element() * 0.01);
        }
        if peak < 1e-3 {
            self.active = false;
        }
    }
}
