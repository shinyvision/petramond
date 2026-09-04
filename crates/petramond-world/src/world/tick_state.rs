//! Fixed-timestep simulation STATE: the tick counter, block-update queue,
//! scheduled ticks, random-tick RNG, and the announced-change feed. The tick DRIVER
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
    /// Block positions announced changed since each consumer last looked —
    /// see [`ChangeFeed`].
    pub changes: ChangeFeed,
    /// Bumped by every announced nav-relevant change (see
    /// `World::nav_revision`).
    pub nav_revision: u64,
}

/// Cap on the announced-change buffer (see [`ChangeFeed`]).
pub const CHANGE_FEED_CAP: usize = 256;

/// Who reads the announced block changes. Each reader keeps its own cursor
/// into the ONE buffer, so the two can never disagree about what was
/// announced between their drains.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ChangeReader {
    /// Confinement-cache invalidation (`mob::confined::RegionCache`),
    /// drained by `tick_mobs`.
    Mobs = 0,
    /// The item store: a lodged item watches its anchor block through it,
    /// so a wall of lodged items costs nothing until a wall block goes.
    /// Drained by `tick_item_physics`.
    Items = 1,
}

const CHANGE_READERS: usize = 2;

/// Block positions announced changed, buffered once for every
/// [`ChangeReader`]: a sliding window of the last [`CHANGE_FEED_CAP`]
/// announcements, numbered from the first ever pushed, with one sequence
/// cursor per reader. A reader whose cursor is still inside the window gets
/// exactly the positions it has not seen; one that fell behind the window
/// is told "everything may have changed" instead, once. So the buffer is
/// bounded whoever drains (a pure client never does), and a reader that
/// lags never costs a current one its positions.
#[derive(Default)]
pub struct ChangeFeed {
    window: VecDeque<IVec3>,
    /// Sequence number of the window's front entry.
    base: u64,
    /// Per reader: the sequence number of the next entry it has not seen.
    cursors: [u64; CHANGE_READERS],
}

impl ChangeFeed {
    /// Record one announced position.
    pub fn push(&mut self, pos: IVec3) {
        if self.window.len() >= CHANGE_FEED_CAP {
            self.window.pop_front();
            self.base += 1;
        }
        self.window.push_back(pos);
    }

    /// Everything announced since `reader` last drained, plus whether the
    /// window slid past unseen positions in between (the reader must then
    /// treat every cell as possibly changed). Entries every reader has seen
    /// are released.
    pub fn drain(&mut self, reader: ChangeReader) -> (Vec<IVec3>, bool) {
        let end = self.base + self.window.len() as u64;
        let cursor = self.cursors[reader as usize];
        let overflow = cursor < self.base;
        let start = if overflow {
            0
        } else {
            (cursor - self.base) as usize
        };
        let out = self.window.range(start..).copied().collect();
        self.cursors[reader as usize] = end;
        let seen = self.cursors.iter().copied().min().unwrap_or(end);
        let release = (seen.saturating_sub(self.base) as usize).min(self.window.len());
        self.window.drain(..release);
        self.base += release as u64;
        (out, overflow)
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
