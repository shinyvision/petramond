//! Per-body easing of published bone offsets.
//!
//! The held-item pose is eased by the renderer's hand animator; the bones have
//! no animator of their own and must move at the SAME rate, or the arm snaps
//! to a stance while the thing in its fist glides after it. One rate
//! ([`POSE_EASE_RATE`]) for both.
//!
//! One per drawn body — the local third-person body and each remote. Pure
//! presentation, so a body that stops being drawn just drops its easer.

use petramond_render::{BoneOffset, POSE_EASE_RATE};

/// The eased bone offsets of ONE body.
#[derive(Default)]
pub struct BoneEase {
    current: Vec<BoneOffset>,
}

impl BoneEase {
    /// The eased offsets as of the last [`advance`](Self::advance) — what the
    /// presentation gather copies into this frame's arena.
    pub fn current(&self) -> &[BoneOffset] {
        &self.current
    }

    /// Advance toward `target` by `dt` and return what to draw this frame.
    ///
    /// Offsets are matched by BONE, never by position: an offset arriving or
    /// leaving must not drag its neighbour through an interpolation between two
    /// unrelated joints, which would swing an arm through the body on the way
    /// to a head tilt. A released offset eases back to the rig's own pose
    /// before its bone is dropped, so straightening is as smooth as bending.
    ///
    /// The buffer is reused — no allocation once a body has settled into the
    /// set of bones it wears.
    pub fn advance(&mut self, target: &[BoneOffset], dt: f32) -> &[BoneOffset] {
        let t = 1.0 - (-POSE_EASE_RATE * dt.max(0.0)).exp();
        let ease = |cur: &mut BoneOffset, want: &BoneOffset| {
            for i in 0..3 {
                cur.rotation[i] += (want.rotation[i] - cur.rotation[i]) * t;
                cur.translation[i] += (want.translation[i] - cur.translation[i]) * t;
            }
        };
        self.current.retain_mut(|cur| {
            match target.iter().find(|w| w.bone == cur.bone) {
                // Still published, same kind of offset: chase it.
                Some(want) if want.hold == cur.hold => {
                    ease(cur, want);
                    true
                }
                // A nudge became a STANCE (or the reverse). The two mean
                // different things about the same joint, so there is nothing
                // meaningful between them — restart at the rig's own pose.
                Some(want) => {
                    *cur = neutral(want);
                    true
                }
                // Released: ease back to the rig's pose, then let the bone go.
                None => {
                    let rest = neutral(cur);
                    ease(cur, &rest);
                    !settled(cur)
                }
            }
        });
        // A bone this body is not already easing starts at the rig's own pose,
        // so it eases IN rather than appearing bent on its first frame.
        for want in target {
            if !self.current.iter().any(|c| c.bone == want.bone) {
                self.current.push(neutral(want));
            }
        }
        &self.current
    }
}

/// `offset`'s bone with no offset on it at all.
fn neutral(offset: &BoneOffset) -> BoneOffset {
    BoneOffset {
        rotation: [0.0; 3],
        translation: [0.0; 3],
        ..*offset
    }
}

/// Close enough to the rig's own pose that the offset can be dropped —
/// a tenth of a degree and a hundredth of a pixel are both invisible.
fn settled(offset: &BoneOffset) -> bool {
    offset.rotation.iter().all(|c| c.abs() < 0.1)
        && offset.translation.iter().all(|c| c.abs() < 0.01)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn offsets(entries: &[(usize, f32)]) -> Vec<BoneOffset> {
        entries
            .iter()
            .map(|(bone, deg)| BoneOffset {
                bone: *bone,
                rotation: [*deg, 0.0, 0.0],
                translation: [0.0; 3],
                hold: false,
            })
            .collect()
    }

    /// An offset eases IN from the rig's own pose and back OUT to it, and a
    /// released bone eventually leaves the set — so a mod that stops bending
    /// an arm leaves the body standing normally rather than holding a
    /// vanishingly small bend forever.
    #[test]
    fn an_offset_eases_in_and_releases_when_it_is_dropped() {
        let mut ease = BoneEase::default();
        let target = offsets(&[(3, -22.0)]);

        let bent = ease.advance(&target, 1.0 / 60.0)[0];
        assert_eq!(bent.bone, 3);
        assert_eq!(
            bent.rotation[0], 0.0,
            "it starts at the rig's pose and eases in, never appearing bent"
        );

        for _ in 0..60 {
            ease.advance(&target, 1.0 / 60.0);
        }
        assert!((ease.advance(&target, 1.0 / 60.0)[0].rotation[0] + 22.0).abs() < 0.5);

        for _ in 0..120 {
            ease.advance(&[], 1.0 / 60.0);
        }
        assert!(
            ease.advance(&[], 1.0 / 60.0).is_empty(),
            "a released bone leaves the set instead of holding a residue"
        );
    }

    /// Offsets are matched by BONE, not by position. A set that grows or
    /// shrinks must leave the bones it still holds exactly where they were —
    /// interpolating a settled shoulder bend toward a head tilt because the
    /// list shifted under it would swing the arm through the body.
    #[test]
    fn a_changed_set_leaves_the_bones_it_still_holds_alone() {
        let mut ease = BoneEase::default();
        let shoulder = offsets(&[(3, -40.0)]);
        for _ in 0..60 {
            ease.advance(&shoulder, 1.0 / 60.0);
        }
        let settled = ease.advance(&shoulder, 1.0 / 60.0)[0].rotation[0];
        assert!(settled < -30.0);

        // The head arrives IN FRONT of the shoulder in the published order.
        let both = offsets(&[(7, -40.0), (3, -40.0)]);
        let now = ease.advance(&both, 1.0 / 60.0);
        let head = now.iter().find(|b| b.bone == 7).expect("the new bone");
        let arm = now.iter().find(|b| b.bone == 3).expect("the held bone");
        assert_eq!(head.rotation[0], 0.0, "the new bone starts at its rest");
        assert!(
            (arm.rotation[0] - settled).abs() < 1.0,
            "the bone that did not change must not be dragged: {} vs {settled}",
            arm.rotation[0]
        );
    }

    /// A nudge and a STANCE mean different things about one joint, so a bone
    /// that switches between them restarts rather than interpolating between
    /// two incompatible readings of the same numbers.
    #[test]
    fn a_bone_that_switches_between_composing_and_holding_restarts() {
        let mut ease = BoneEase::default();
        let composed = offsets(&[(3, -40.0)]);
        for _ in 0..60 {
            ease.advance(&composed, 1.0 / 60.0);
        }
        assert!(ease.advance(&composed, 1.0 / 60.0)[0].rotation[0] < -30.0);

        let mut held = composed.clone();
        held[0].hold = true;
        let now = ease.advance(&held, 1.0 / 60.0)[0];
        assert!(now.hold);
        assert_eq!(now.rotation[0], 0.0, "the stance eases in from rest");
    }
}
