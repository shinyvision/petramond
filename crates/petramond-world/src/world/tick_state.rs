//! Fixed-timestep simulation STATE: the tick counter, block-update queue,
//! scheduled ticks, random-tick RNG, and the announced-change log. The tick DRIVER
//! (dispatching updates against the world) lives in the engine crate.

use std::cmp::Reverse;
use std::collections::{BinaryHeap, VecDeque};

use rustc_hash::FxHashSet;

use crate::mathh::IVec3;

/// One pending scheduled tick, min-heap ordered: `(due tick, schedule order, x, y, z)`.
type ScheduledTick = Reverse<(u64, u64, i32, i32, i32)>;

/// Per-world tick/update/schedule bookkeeping.
#[derive(Default)]
pub struct TickState {
    /// Monotonic game-tick counter (20 per second).
    pub tick: u64,
    /// Cells whose neighbourhood changed since the last tick, awaiting dispatch.
    pub update_queue: VecDeque<IVec3>,
    pub update_set: FxHashSet<IVec3>,
    /// Pending scheduled ticks ordered by due tick, then by scheduling order
    /// (min-heap via `Reverse`); the position rides along in the entry.
    pub scheduled: BinaryHeap<ScheduledTick>,
    /// Monotonic counter that timestamps each schedule, so ticks due on the same
    /// game tick execute in the order they were scheduled.
    pub scheduled_seq: u64,
    /// Positions with a scheduled tick already pending, for dedup.
    pub scheduled_set: FxHashSet<IVec3>,
    /// Blocks the simulation itself destroyed this tick (a fragile block losing its
    /// support, or one washed away by water), each as `(pos, block)`. Purely a
    /// hand-off to the presentation layer: `Game` drains it right after the tick (see
    /// `World::take_natural_breaks`) to play the break burst + roll the drops, so the
    /// visual effect lives in `Game` while the world stays the authority on the change.
    pub pending_breaks: Vec<(IVec3, crate::block::Block)>,
    /// xorshift64 state for random-tick cell selection (kept non-zero; see
    /// [`TickState::new`]).
    pub rng: u64,
    /// Reused per-phase batch buffer (scheduled dues, update drain, random-tick
    /// cells). The phases run strictly in sequence, so one buffer serves all
    /// three without a fresh allocation every tick.
    pub batch_scratch: Vec<IVec3>,
    /// Every announced change, for readers that keep their own place — see
    /// [`ChangeLog`].
    pub change_log: ChangeLog,
}

/// Cap on the announced-change log (see [`ChangeLog`]).
pub const CHANGE_LOG_CAP: usize = 4096;

/// One announced change: the cell, and whether it could alter what a body
/// walks on or through (a decoration appearing cannot).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Change {
    pub pos: IVec3,
    pub nav: bool,
}

/// Every announced change — a block, a fluid, a door's swing — numbered
/// from the first ever pushed, the last [`CHANGE_LOG_CAP`] of them kept.
/// The READER holds its place (the number of the next entry it has not
/// seen), so any number of readers the world knows nothing about follow it
/// at their own pace, the log stays bounded whoever reads, and a reader
/// that lags never costs a current one its positions.
#[derive(Default)]
pub struct ChangeLog {
    window: VecDeque<Change>,
    /// Sequence number of the window's front entry.
    base: u64,
    nav_revision: u64,
}

impl ChangeLog {
    pub fn push(&mut self, pos: IVec3, nav: bool) {
        if self.window.len() >= CHANGE_LOG_CAP {
            self.window.pop_front();
            self.base += 1;
        }
        self.window.push_back(Change { pos, nav });
        if nav {
            self.nav_revision = self.nav_revision.wrapping_add(1);
        }
    }

    /// The number the next pushed entry will carry.
    pub fn end(&self) -> u64 {
        self.base + self.window.len() as u64
    }

    /// Moves whenever a nav-relevant change is pushed. Outlives the window:
    /// the positions can be lost, the fact cannot.
    pub fn nav_revision(&self) -> u64 {
        self.nav_revision
    }

    /// Everything pushed from entry `seq` on, and whether some of it is
    /// gone (the log slid past it, or `seq` is from another log's
    /// numbering): the reader must then treat every cell as changed.
    pub fn since(&self, seq: u64) -> (Vec<IVec3>, bool) {
        self.collect_since(seq, |_| true)
    }

    /// [`since`](Self::since), nav-relevant changes only.
    pub fn nav_since(&self, seq: u64) -> (Vec<IVec3>, bool) {
        self.collect_since(seq, |c| c.nav)
    }

    fn collect_since(&self, seq: u64, keep: impl Fn(&Change) -> bool) -> (Vec<IVec3>, bool) {
        if seq < self.base || seq > self.end() {
            return (Vec::new(), true);
        }
        let start = (seq - self.base) as usize;
        let cells = self
            .window
            .range(start..)
            .filter(|c| keep(c))
            .map(|c| c.pos);
        (cells.collect(), false)
    }
}

#[cfg(test)]
mod tests;

impl TickState {
    /// Seed the per-world tick state. Only `rng` needs a non-default value
    /// (xorshift64 is stuck at 0); the world seed is mixed in purely to
    /// decorrelate leaf-decay order between worlds — random ticks are real-time
    /// gameplay RNG, not part of deterministic worldgen.
    pub fn new(seed: u32) -> Self {
        Self {
            rng: (seed as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1,
            ..Default::default()
        }
    }

    /// Next xorshift64 word, for choosing random-tick cells.
    #[inline]
    pub fn next_random(&mut self) -> u64 {
        let mut x = self.rng;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.rng = x;
        x
    }
}
