//! The client's easing of every animated block's open fraction — one map and
//! one rule for a chest's lid, a door's or trapdoor's swing, a pack's own.
//!
//! A block rests at its logical open state (its cell's pose, or whoever is
//! looking inside a container), so only a block mid-swing holds an entry: a
//! change of logical state seeds one at the OLD resting pose, the per-frame
//! advance eases it toward the new one at its model's `open_speed`, and it is
//! dropped on arrival. Client-side presentation only, never persisted.

use std::collections::HashMap;

use petramond_math::math::IVec3;

#[derive(Default)]
pub(super) struct BlockAnimations {
    /// Linear open progress (`0.0` closed .. `1.0` open) of each block
    /// mid-swing, keyed by its anchor cell (a door's lower half).
    swings: HashMap<IVec3, f32>,
}

impl BlockAnimations {
    /// The block at `anchor` changed its logical open state away from
    /// `was_open`: ease it from that resting pose. A block already mid-swing
    /// keeps its progress and simply turns around.
    pub(super) fn begin(&mut self, anchor: IVec3, was_open: bool) {
        self.swings
            .entry(anchor)
            .or_insert(if was_open { 1.0 } else { 0.0 });
    }

    /// The linear open progress of the block at `anchor` whose logical state
    /// is `open`: its eased value mid-swing, else its resting pose.
    pub(super) fn progress(&self, anchor: IVec3, open: bool) -> f32 {
        self.swings
            .get(&anchor)
            .copied()
            .unwrap_or(if open { 1.0 } else { 0.0 })
    }

    /// Ease every swinging block toward its logical state by `dt` seconds.
    /// `target` answers a block's `(open, open_speed)`, or `None` once it is
    /// gone (broken mid-swing); gone and arrived blocks are dropped.
    pub(super) fn advance(&mut self, dt: f32, target: impl Fn(IVec3) -> Option<(bool, f32)>) {
        self.swings.retain(|&anchor, progress| {
            let Some((open, speed)) = target(anchor) else {
                return false;
            };
            let goal = if open { 1.0 } else { 0.0 };
            let step = (dt * speed).clamp(0.0, 1.0);
            if *progress < goal {
                *progress = (*progress + step).min(goal);
            } else {
                *progress = (*progress - step).max(goal);
            }
            (*progress - goal).abs() > f32::EPSILON
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const AT: IVec3 = IVec3::new(1, 2, 3);

    #[test]
    fn a_resting_block_reads_its_logical_state() {
        let anims = BlockAnimations::default();
        assert_eq!(anims.progress(AT, true), 1.0);
        assert_eq!(anims.progress(AT, false), 0.0);
    }

    #[test]
    fn a_swing_eases_from_the_old_pose_and_settles() {
        let mut anims = BlockAnimations::default();
        anims.begin(AT, false);
        assert_eq!(anims.progress(AT, true), 0.0, "starts from closed");
        anims.advance(0.1, |_| Some((true, 4.0)));
        assert!((anims.progress(AT, true) - 0.4).abs() < 1e-5, "eases at its speed");
        for _ in 0..10 {
            anims.advance(0.1, |_| Some((true, 4.0)));
        }
        assert!(anims.swings.is_empty(), "an arrived block rests");
        assert_eq!(anims.progress(AT, true), 1.0);
    }

    #[test]
    fn a_reversal_mid_swing_turns_around_and_a_gone_block_is_dropped() {
        let mut anims = BlockAnimations::default();
        anims.begin(AT, false);
        anims.advance(0.1, |_| Some((true, 5.0)));
        anims.begin(AT, true);
        assert!((anims.progress(AT, false) - 0.5).abs() < 1e-5, "keeps its progress");
        anims.advance(0.05, |_| Some((false, 5.0)));
        assert!((anims.progress(AT, false) - 0.25).abs() < 1e-5, "and eases back");
        anims.advance(0.1, |_| None);
        assert!(anims.swings.is_empty(), "a broken block stops animating");
    }
}
