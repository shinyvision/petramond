//! The client's easing of every animated block's open fraction — one map and
//! one rule for a chest's lid, a door's or trapdoor's swing, a pack's own.
//!
//! A block rests at its logical open state (its cell's pose, or whoever is
//! looking inside a container — the replicated open-chest set this type also
//! owns), so only a block mid-swing holds an entry: a
//! change of logical state seeds one at the OLD resting pose, the per-frame
//! advance eases it toward the new one at its model's `open_speed`, and it is
//! dropped on arrival. Client-side presentation only, never persisted.

use std::collections::HashMap;

use petramond_math::math::IVec3;
use rustc_hash::FxHashSet;

#[derive(Default)]
pub(super) struct BlockAnimations {
    /// Linear open progress (`0.0` closed .. `1.0` open) of each block
    /// mid-swing, keyed by its anchor cell (a door's lower half).
    swings: HashMap<IVec3, f32>,
    /// Chests with at least one open screen anywhere (replicated per batch —
    /// the server's viewer key set): open whatever their cell pose says.
    open_chests: FxHashSet<IVec3>,
}

impl BlockAnimations {
    /// Adopt the REPLICATED open-chest set: a chest entering it starts its
    /// lid opening, one leaving starts it closing — ANY player's open screen
    /// lifts the lid on every client.
    pub(super) fn set_open_chests(&mut self, open: FxHashSet<IVec3>) {
        let entered: Vec<IVec3> = open.difference(&self.open_chests).copied().collect();
        let left: Vec<IVec3> = self.open_chests.difference(&open).copied().collect();
        for pos in entered {
            self.begin(pos, false);
        }
        for pos in left {
            self.begin(pos, true);
        }
        self.open_chests = open;
    }

    /// The chests someone is looking inside.
    pub(super) fn open_chests(&self) -> &FxHashSet<IVec3> {
        &self.open_chests
    }

    /// The linear open progress of the block at `anchor` whose cell pose says
    /// `pose_open`: eased mid-swing, else resting at its logical state — the
    /// pose, or anyone looking inside it.
    pub(super) fn open_progress(&self, anchor: IVec3, pose_open: bool) -> f32 {
        self.progress(anchor, pose_open || self.open_chests.contains(&anchor))
    }

    /// [`advance`](Self::advance) where `pose` answers a block's
    /// `(pose_open, open_speed)` from its cell and the open-chest set is
    /// folded in here.
    pub(super) fn advance_poses(&mut self, dt: f32, pose: impl Fn(IVec3) -> Option<(bool, f32)>) {
        let open_chests = std::mem::take(&mut self.open_chests);
        self.advance(dt, |anchor| {
            pose(anchor).map(|(open, speed)| (open || open_chests.contains(&anchor), speed))
        });
        self.open_chests = open_chests;
    }

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

    #[test]
    fn a_viewed_chest_opens_and_closes_with_the_replicated_set() {
        let mut anims = BlockAnimations::default();
        anims.set_open_chests([AT].into_iter().collect());
        assert_eq!(anims.open_progress(AT, false), 0.0, "the lid starts closed");
        for _ in 0..10 {
            anims.advance_poses(0.1, |_| Some((false, 4.0)));
        }
        assert_eq!(anims.open_progress(AT, false), 1.0, "a viewer holds it open");
        anims.set_open_chests(FxHashSet::default());
        assert_eq!(anims.open_progress(AT, false), 1.0, "closing starts from open");
        for _ in 0..10 {
            anims.advance_poses(0.1, |_| Some((false, 4.0)));
        }
        assert_eq!(anims.open_progress(AT, false), 0.0);
        assert!(anims.open_chests().is_empty());
    }
}
