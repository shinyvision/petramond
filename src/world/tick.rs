//! Fixed-timestep world simulation: the 20 TPS game tick, neighbour "block
//! updates", and scheduled block ticks.
//!
//! These three pieces are deliberately generic: the generic update/scheduled
//! loops never name a concrete block. Block reactions are routed through a
//! two-phase, world-side dispatch ([`World::on_neighbor_update`] for the announce
//! phase and [`World::on_scheduled_tick`] for the execute phase). That dispatch
//! is the one extension point for reactive blocks — today only water (see
//! [`super::water`]); future blocks (gravity, growth, …) add a `match` arm there.
//!
//! The dispatch lives on the world side (not on `Block`) on purpose: water's
//! reaction reaches into `World`/`FluidSim` internals, which a `Block` method
//! must not import. The reaction always receives `&mut World` and never stores
//! world state on a block.
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
use crate::crafting::Recipes;
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
    /// (even with no pending work) so cadence is independent of activity. Owns the
    /// whole per-tick sequence so the order lives in one place.
    ///
    /// Order per tick, which must stay exact (reordering reorders the simulation):
    /// 1. run the scheduled block ticks due now (these may set blocks, which
    ///    enqueue fresh block updates),
    /// 2. dispatch every queued block update (which may schedule future ticks;
    ///    dispatch never sets blocks, so the drain terminates within the tick),
    /// 3. advance furnace smelting on the same clock (needs `recipes`, which the
    ///    storage layer is kept ignorant of — see [`World::tick_furnaces`]).
    ///
    /// Item physics is paced per render frame (`Game::tick_entities`) and item
    /// lifetime/pickup per tick by `Game` (it needs the player inventory), so
    /// those stay in `Game`; everything the world owns alone sequences here.
    pub fn game_tick(&mut self, recipes: &Recipes) {
        self.sim.tick = self.sim.tick.wrapping_add(1);
        let now = self.sim.tick;

        // 1. Run scheduled block ticks whose due time has arrived (EXECUTE phase).
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

        // 2. Dispatch the block updates accumulated since the last tick (ANNOUNCE
        //    phase). MUST run after scheduled ticks: collapsing or reordering the
        //    two reorders the simulation.
        if !self.sim.update_queue.is_empty() {
            let updates: Vec<IVec3> = self.sim.update_queue.drain(..).collect();
            self.sim.update_set.clear();
            for pos in updates {
                self.dispatch_block_update(pos);
            }
        }

        // 3. Smelt every loaded furnace one tick (chunk-owned; cheap when none).
        self.tick_furnaces(recipes);
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

    /// Generic ANNOUNCE step: a neighbour of `pos` changed. Read the block there
    /// and route it to the world-side reaction dispatch. Names no concrete block.
    fn dispatch_block_update(&mut self, pos: IVec3) {
        let block = Block::from_id(self.chunk_block(pos.x, pos.y, pos.z));
        self.on_neighbor_update(block, pos);
    }

    /// Generic EXECUTE step: run the scheduled behaviour for the block at `pos`.
    /// Read the block there and route it to the world-side reaction dispatch.
    /// Names no concrete block.
    fn run_scheduled_tick(&mut self, pos: IVec3) {
        let block = Block::from_id(self.chunk_block(pos.x, pos.y, pos.z));
        self.on_scheduled_tick(block, pos);
    }

    /// Reaction dispatch, ANNOUNCE phase: the `block` at `pos` learns a neighbour
    /// changed and may schedule a future scheduled-tick. The single extension
    /// point for reactive blocks (with [`on_scheduled_tick`](Self::on_scheduled_tick));
    /// today only water reacts, so this is one branch — it grows into a `match`
    /// over `block` as gravity/growth/… are added.
    ///
    /// World-side by design: water reaches into `World`/`FluidSim` internals a
    /// `Block` method must not import. The reaction takes `&mut World` and never
    /// stores world state on a block.
    fn on_neighbor_update(&mut self, block: Block, pos: IVec3) {
        // Water schedules its flow check `WATER_FLOW_DELAY` ticks out so a
        // disturbance settles before it re-levels (see `super::water`).
        if block == Block::Water {
            self.schedule_block_tick(pos, super::water::WATER_FLOW_DELAY);
        }
    }

    /// Reaction dispatch, EXECUTE phase: the `block` at `pos` runs its scheduled
    /// behaviour. The single extension point for reactive blocks (paired with
    /// [`on_neighbor_update`](Self::on_neighbor_update)); grows into a `match` over
    /// `block` as more reactive blocks are added.
    fn on_scheduled_tick(&mut self, block: Block, pos: IVec3) {
        // `FluidSim` is stateless w.r.t. the world: construct it here and hand
        // it `&mut self` per call, never storing the borrow (see `super::water`).
        if block == Block::Water {
            super::water::FluidSim.flow_check(self, pos);
        }
    }
}
