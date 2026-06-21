//! Fixed-timestep world simulation: the 20 TPS game tick, neighbour "block
//! updates", and scheduled block ticks.
//!
//! These three pieces are deliberately generic — only the dispatch in
//! [`World::dispatch_block_update`] / [`World::run_scheduled_tick`] knows about
//! water (see [`super::water`]); new reactive blocks (gravity, growth, …) hook
//! in there.
//!
//! Ownership note: the whole simulation runs on the main thread inside
//! [`World::game_tick`], driven by an accumulator in `Game::tick`. It mutates the
//! world directly, which is why it lives here rather than on a worker — the world
//! is single-thread-owned and the background workers only ever read snapshots.
//!
//! - **Block updates**: when a block changes, the cell and its 6 orthogonal
//!   neighbours (across chunk borders) are queued. Each is dispatched once per
//!   tick so a block type can react to a changed neighbour. Deduped per tick.
//! - **Scheduled ticks**: a block can ask to run again `delay` ticks later
//!   (water schedules its flow check 10 ticks after a disturbance). A min-heap
//!   keyed by tick number, deduped by position so repeated disturbances collapse
//!   to one pending check.

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashSet, VecDeque};

use crate::block::Block;
use crate::mathh::IVec3;

use super::store::World;

/// The six orthogonal neighbour offsets, used for block-update propagation.
pub(super) const NEIGHBORS: [IVec3; 6] = [
    IVec3::new(1, 0, 0),
    IVec3::new(-1, 0, 0),
    IVec3::new(0, 1, 0),
    IVec3::new(0, -1, 0),
    IVec3::new(0, 0, 1),
    IVec3::new(0, 0, -1),
];

/// Per-world tick/update/schedule bookkeeping.
#[derive(Default)]
pub(super) struct TickState {
    /// Monotonic game-tick counter (20 per second).
    tick: u64,
    /// Cells whose neighbourhood changed since the last tick, awaiting dispatch.
    update_queue: VecDeque<IVec3>,
    update_set: HashSet<IVec3>,
    /// Pending scheduled ticks ordered by due tick (min-heap via `Reverse`).
    scheduled: BinaryHeap<Reverse<(u64, i32, i32, i32)>>,
    /// Positions with a scheduled tick already pending, for dedup.
    scheduled_set: HashSet<IVec3>,
}

impl World {
    /// Current game-tick number (advances once per [`World::game_tick`]).
    #[inline]
    pub fn current_tick(&self) -> u64 {
        self.sim.tick
    }

    /// Advance the world simulation by one fixed 50 ms step. Runs unconditionally
    /// (even with no pending work) so cadence is independent of activity.
    ///
    /// Order per tick: run the block ticks due now (these may set blocks, which
    /// enqueue fresh block updates), then dispatch every queued block update
    /// (which may schedule future ticks). Dispatch never sets blocks, so the
    /// drain terminates within the tick.
    pub fn game_tick(&mut self) {
        self.sim.tick = self.sim.tick.wrapping_add(1);
        let now = self.sim.tick;

        // 1. Run scheduled block ticks whose due time has arrived.
        let mut due: Vec<IVec3> = Vec::new();
        while let Some(&Reverse((d, x, y, z))) = self.sim.scheduled.peek() {
            if d > now {
                break;
            }
            self.sim.scheduled.pop();
            let pos = IVec3::new(x, y, z);
            self.sim.scheduled_set.remove(&pos);
            due.push(pos);
        }
        for pos in due {
            self.run_scheduled_tick(pos);
        }

        // 2. Dispatch the block updates accumulated since the last tick.
        if !self.sim.update_queue.is_empty() {
            let updates: Vec<IVec3> = self.sim.update_queue.drain(..).collect();
            self.sim.update_set.clear();
            for pos in updates {
                self.dispatch_block_update(pos);
            }
        }
    }

    /// Announce that the block at `(wx, wy, wz)` changed: queue a block update for
    /// the cell itself and each of its 6 orthogonal neighbours (crossing chunk
    /// borders). Deduped within the current tick.
    pub(super) fn notify_block_and_neighbors(&mut self, wx: i32, wy: i32, wz: i32) {
        let p = IVec3::new(wx, wy, wz);
        self.queue_block_update(p);
        for d in NEIGHBORS {
            self.queue_block_update(p + d);
        }
    }

    pub(super) fn queue_block_update(&mut self, pos: IVec3) -> bool {
        if self.sim.update_set.insert(pos) {
            self.sim.update_queue.push_back(pos);
            true
        } else {
            false
        }
    }

    /// Ask for `pos` to run a scheduled tick `delay` ticks from now. No-op if a
    /// tick is already pending for `pos` (first schedule wins).
    pub(super) fn schedule_block_tick(&mut self, pos: IVec3, delay: u64) {
        if self.sim.scheduled_set.insert(pos) {
            let due = self.sim.tick.wrapping_add(delay);
            self.sim
                .scheduled
                .push(Reverse((due, pos.x, pos.y, pos.z)));
        }
    }

    /// A neighbour of `pos` changed: let the block at `pos` react. Today only
    /// water reacts (by scheduling a flow check); future reactive blocks branch
    /// here too.
    fn dispatch_block_update(&mut self, pos: IVec3) {
        let block = Block::from_id(self.chunk_block(pos.x, pos.y, pos.z));
        if block == Block::Water {
            self.schedule_block_tick(pos, super::water::WATER_FLOW_DELAY);
        }
    }

    /// Run the scheduled behaviour for the block at `pos`.
    fn run_scheduled_tick(&mut self, pos: IVec3) {
        let block = Block::from_id(self.chunk_block(pos.x, pos.y, pos.z));
        if block == Block::Water {
            self.water_flow_check(pos);
        }
    }
}
