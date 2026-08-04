//! Fixed-timestep simulation STATE: the tick counter, block-update queue,
//! scheduled ticks, random-tick RNG, and nav-change feed. The tick DRIVER
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
    /// [`World::take_natural_breaks`]) to play the break burst + roll the drops, so the
    /// visual effect lives in `Game` while the world stays the authority on the change.
    pub pending_breaks: Vec<(IVec3, crate::block::Block)>,
    /// xorshift64 state for random-tick cell selection (kept non-zero; see
    /// [`TickState::new`]).
    pub rng: u64,
    /// Reused per-phase batch buffer (scheduled dues, update drain, random-tick
    /// cells). The phases run strictly in sequence, so one buffer serves all
    /// three without a fresh allocation every tick.
    pub batch_scratch: Vec<IVec3>,
    /// Block positions announced changed since the last mob tick — the feed
    /// for confinement-cache invalidation (`mob::confined::RegionCache`),
    /// drained by `tick_mobs`. Bounded: past [`NAV_CHANGE_CAP`] the overflow
    /// flag stands in for the exact positions (invalidate everything), so a
    /// world that never drains (a pure client) cannot grow it unbounded.
    pub nav_changes: Vec<IVec3>,
    pub nav_changes_overflow: bool,
    /// Bumped by every announced nav-relevant change (see
    /// [`World::nav_revision`]).
    pub nav_revision: u64,
}

/// Cap on the per-tick nav-change buffer (see [`TickState::nav_changes`]).
pub const NAV_CHANGE_CAP: usize = 256;
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
