//! How the golem shows what it is doing: where it looks and what it holds.

use mod_sdk::*;

use super::{act, trouble, EYE_HEIGHT, FACE_TAG, GOAL_TAG, HOLD_TAG, LOOK_TAG, TRACE};
use crate::geometry::encode_cell;

/// Where a player's eyes are, for a golem to look at.
pub(super) fn eye_of(player: PlayerId) -> Option<[f64; 3]> {
    const PLAYER_EYE: f64 = 1.62;
    players()
        .into_iter()
        .find(|row| row.id == player)
        .map(|row| {
            [
                row.state.pos[0],
                row.state.pos[1] + PLAYER_EYE,
                row.state.pos[2],
            ]
        })
}

/// What the golem shows of itself, as last written to its tags: only changes
/// are written.
#[derive(Default)]
pub struct Presentation {
    pub(super) goal: Option<[i32; 3]>,
    pub(super) hold: bool,
    pub(super) held: Option<String>,
    pub(super) anim: Option<&'static str>,
    pub(super) look: Option<f32>,
    pub(super) gaze: Option<[f64; 3]>,
    /// When the gaze last swung to a new direction, and the tries an action
    /// has waited for it to land.
    pub(super) gaze_since: u64,
    pub(super) gaze_dir: [f64; 3],
    pub(super) unaimed: u32,
    /// The mark over its head, as last submitted.
    pub(super) mark: Option<trouble::Mark>,
    /// What it was thinking about when a player opened its panel, kept while
    /// the panel is up: standing still to be asked must not wipe the answer.
    pub(super) asked_about: Option<Option<trouble::Trouble>>,
}

impl Presentation {
    /// The golem is gone, and its tags with it: nothing written is remembered.
    pub(super) fn forget_tags(&mut self) {
        self.goal = None;
        self.hold = false;
        self.look = None;
        self.gaze = None;
        self.held = None;
        self.anim = None;
    }

    pub(super) fn set_goal(&mut self, id: u64, goal: Option<[i32; 3]>) {
        if self.goal == goal {
            return;
        }
        self.goal = goal;
        match goal {
            Some(cell) => mob_tag_set(id, GOAL_TAG, MobTagValue::Str(encode_cell(cell))),
            None => mob_tag_delete(id, GOAL_TAG),
        };
    }

    pub(super) fn set_hold(&mut self, id: u64, hold: bool) {
        if self.hold == hold {
            return;
        }
        self.hold = hold;
        if hold {
            mob_tag_set(id, HOLD_TAG, MobTagValue::Bool(true));
        } else {
            mob_tag_delete(id, HOLD_TAG);
        }
    }

    /// Turn toward cell `at` from where the body stands and look at it, or
    /// stop (`None`).
    pub(super) fn face(&mut self, id: u64, at: Option<([f64; 3], [i32; 3])>) {
        match at {
            Some((from, cell)) => self.look_at(id, 0, from, crate::geometry::centre_of(cell)),
            None => {
                if self.look.take().is_some() {
                    mob_tag_delete(id, FACE_TAG);
                }
                if self.gaze.take().is_some() {
                    mob_tag_delete(id, LOOK_TAG);
                }
            }
        }
    }

    /// Turn the body and the head toward `point`. A point straight over or
    /// under the body turns only the head.
    pub(super) fn look_at(&mut self, id: u64, now: u64, from: [f64; 3], point: [f64; 3]) {
        let (dx, dz) = (point[0] - from[0], point[2] - from[2]);
        // Even a point almost underfoot has a bearing, and the body must come
        // round to it: the neck alone does not look behind.
        if dx * dx + dz * dz > 4.0e-4 {
            let yaw = (-dx).atan2(-dz) as f32;
            if self.look.is_none_or(|was| (was - yaw).abs() > 0.01) {
                self.look = Some(yaw);
                mob_tag_set(id, FACE_TAG, MobTagValue::F64(f64::from(yaw)));
            }
        }
        let moved = |was: [f64; 3]| {
            let d = [point[0] - was[0], point[1] - was[1], point[2] - was[2]];
            d[0] * d[0] + d[1] * d[1] + d[2] * d[2]
        };
        if self.gaze.is_none_or(|was| moved(was) > 1.0e-4) {
            // A new direction is a swing of the head; the same one from a
            // body that moved (a jump over its own column) is not.
            let to = [dx, point[1] - (from[1] + EYE_HEIGHT), dz];
            let len = (to[0] * to[0] + to[1] * to[1] + to[2] * to[2])
                .sqrt()
                .max(1.0e-6);
            let dir = to.map(|c| c / len);
            let along: f64 = (0..3).map(|a| dir[a] * self.gaze_dir[a]).sum();
            if self.gaze.is_none() || along < 0.9 {
                self.gaze_since = now;
            }
            self.gaze_dir = dir;
            self.gaze = Some(point);
            mob_tag_set(
                id,
                LOOK_TAG,
                MobTagValue::Str(crate::geometry::encode_point(point)),
            );
        }
    }

    pub(super) fn hold_item(&mut self, id: u64, item: Option<String>) {
        if self.held == item {
            return;
        }
        mob_held_display(id, item.clone(), None);
        self.held = item;
    }

    /// Play the one-shot jab of a hand setting something down.
    pub(super) fn jab(&mut self, id: u64) {
        self.animate(id, None);
        mob_anim_set(id, act::JAB, false);
        if !mob_anim_set(id, act::JAB, true) && TRACE {
            log("TRACE the golem has no jab clip");
        }
    }

    pub(super) fn animate(&mut self, id: u64, anim: Option<&'static str>) {
        if self.anim == anim {
            return;
        }
        if let Some(old) = self.anim {
            mob_anim_set(id, old, false);
        }
        if let Some(new) = anim {
            mob_anim_set(id, new, true);
        }
        self.anim = anim;
    }

    /// Nowhere to walk, nothing to face, nothing in hand — whatever clip is
    /// playing plays on. A step that plays a clip of its own (sinking into
    /// the ground) stands still with this: [`rest`](Self::rest) ends the
    /// clip, and ending it each tick before playing it again restarts it
    /// each tick.
    pub(super) fn still(&mut self, id: u64) {
        self.set_goal(id, None);
        self.face(id, None);
        self.hold_item(id, None);
    }

    pub(super) fn rest(&mut self, id: u64) {
        self.still(id);
        self.animate(id, None);
    }
}
