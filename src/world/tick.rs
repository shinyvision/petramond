//! Fixed-timestep world simulation: the 20 TPS game tick, neighbour "block
//! updates", scheduled block ticks, and random ticks.
//!
//! The generic loops never name a concrete block. Reactions are dispatched two
//! ways, both keeping those loops block-agnostic:
//! - **random ticks** route through the block's own behaviour (see
//!   [`crate::block::behavior`]): a block carries its reaction as data, so adding
//!   one is "write a behaviour and point its row at it", never a `match` here.
//! - **neighbour / scheduled** reactions still route through a world-side dispatch
//!   ([`World::on_neighbor_update`] / [`World::on_scheduled_tick`]) — today only
//!   water (see [`super::water`]), whose reaction reaches into `World`/`FluidSim`
//!   internals a `Block` must not import. These two hooks are the next to fold
//!   into behaviours (a `Water` behaviour living in `world`), collapsing onto the
//!   single behaviour extension point.
//!
//! Either way a reaction receives `&mut World` and never stores world state on a
//! block.
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
//! - **Random ticks**: each tick, [`RANDOM_TICK_SPEED`] uniformly-random cells in
//!   every loaded column near the player get a probabilistic behaviour callback
//!   (leaf decay today). Air picks are skipped on the spot and columns with
//!   nothing random-tickable are skipped wholesale via a per-chunk counter.

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashSet, VecDeque};

use crate::block::Block;
use crate::chunk::{self, ChunkPos, CHUNK_SX, CHUNK_SZ};
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

const RANDOM_TICK_SPEED: u32 = 48;

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
    /// xorshift64 state for random-tick cell selection (kept non-zero; see
    /// [`TickState::new`]).
    rng: u64,
}

impl TickState {
    /// Seed the per-world tick state. Only `rng` needs a non-default value
    /// (xorshift64 is stuck at 0); the world seed is mixed in purely to
    /// decorrelate leaf-decay order between worlds — random ticks are real-time
    /// gameplay RNG, not part of deterministic worldgen.
    pub(super) fn new(seed: u32) -> Self {
        Self {
            rng: (seed as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1,
            ..Default::default()
        }
    }

    /// Next xorshift64 word, for choosing random-tick cells.
    #[inline]
    fn next_random(&mut self) -> u64 {
        let mut x = self.rng;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.rng = x;
        x
    }
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
    ///    storage layer is kept ignorant of — see [`World::tick_furnaces`]),
    /// 4. run random block ticks near the player (probabilistic per-block
    ///    behaviour, e.g. leaf decay; order-independent of the above).
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

        // 4. Random block ticks: a few random cells per nearby column get a
        //    probabilistic behaviour callback (today: leaf decay). Cheapest of all
        //    when nothing is tickable — empty columns are skipped by their counter.
        self.random_tick_chunks();
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
            self.sim.scheduled.push(Reverse((due, pos.x, pos.y, pos.z)));
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

    /// Random block ticks: for each loaded column near the player that holds any
    /// random-tickable block, pick [`RANDOM_TICK_SPEED`] cells uniformly from the
    /// whole 16×16×256 column and run each one's behaviour. Air — the vast bulk of
    /// any column — is skipped on the spot, and a column with nothing tickable is
    /// skipped wholesale via its chunk counter, so the cost is a few RNG draws and
    /// array reads per column.
    ///
    /// Eligibility is a disc of `render_dist - 2` chunks around the player, so a
    /// ticked column's neighbours are loaded (the leaf-decay flood reaches a few
    /// blocks across borders). A cell that is *still* unloaded is treated as
    /// support, so nothing decays on missing information.
    fn random_tick_chunks(&mut self) {
        let Some(target) = self.last_load_target else {
            return;
        };
        let center = target.center;
        let r = (target.render_dist - 2).max(0);

        // Gather phase: choose the cells to tick WITHOUT holding a chunk borrow
        // across the dispatch (which mutates the world). `self.sim` and
        // `self.chunks` are disjoint fields, so the RNG draw and the block reads
        // borrow side by side.
        let mut due: Vec<IVec3> = Vec::new();
        for dz in -r..=r {
            for dx in -r..=r {
                if dx * dx + dz * dz > r * r {
                    continue;
                }
                let pos = ChunkPos::new(center.cx + dx, center.cz + dz);
                let Some(chunk) = self.chunks.get(&pos) else {
                    continue;
                };
                if !chunk.has_random_tickable() {
                    continue;
                }
                let (ox, oz) = chunk.chunk_origin_world();
                let blocks = chunk.blocks_slice();
                for _ in 0..RANDOM_TICK_SPEED {
                    let i = (self.sim.next_random() >> 16) as usize % chunk::VOLUME;
                    let id = blocks[i];
                    if id == 0 {
                        continue; // air — nothing ticks; the overwhelming majority
                    }
                    if !Block::from_id(id).has_random_tick() {
                        continue;
                    }
                    // Decode the flat index back to world coords (only for a hit).
                    let lx = i & (CHUNK_SX - 1);
                    let lz = (i >> 4) & (CHUNK_SZ - 1);
                    let ly = i >> 8;
                    due.push(IVec3::new(ox + lx as i32, ly as i32, oz + lz as i32));
                }
            }
        }

        // Dispatch phase: the chunk-map borrow is released, so each behaviour is
        // free to edit the world (a decaying leaf sets air, which relights/remeshes).
        for pos in due {
            self.run_random_tick(pos);
        }
    }

    /// Read the block at `pos` and run its random-tick behaviour. The block is
    /// re-read here (not carried from the gather pass) so an earlier tick in the
    /// same batch that changed this cell is respected — like `run_scheduled_tick`.
    fn run_random_tick(&mut self, pos: IVec3) {
        let block = Block::from_id(self.chunk_block(pos.x, pos.y, pos.z));
        block.behavior().random_tick(self, pos);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chunk::Chunk;
    use crate::crafting::Recipes;

    use super::super::store::LoadTarget;

    /// A world with one empty loaded chunk at (0,0) and the player centred on it,
    /// so its column is eligible for random ticks.
    fn world_with_centered_chunk() -> World {
        let mut world = World::new(1, 4);
        world.chunks.insert(ChunkPos::new(0, 0), Chunk::new(0, 0));
        world.last_load_target = Some(LoadTarget::new(0, 0, 4));
        world
    }

    /// Fire one leaf random tick at `p` through the public behaviour path.
    fn tick_leaf(world: &mut World, p: IVec3) {
        Block::OakLeaves.behavior().random_tick(world, p);
    }

    #[test]
    fn isolated_leaf_decays() {
        let mut world = world_with_centered_chunk();
        let p = IVec3::new(8, 70, 8);
        world.set_block_world(p.x, p.y, p.z, Block::OakLeaves);
        tick_leaf(&mut world, p);
        assert_eq!(world.chunk_block(p.x, p.y, p.z), Block::Air.id());
    }

    #[test]
    fn leaf_next_to_log_survives() {
        let mut world = world_with_centered_chunk();
        let p = IVec3::new(8, 70, 8);
        world.set_block_world(p.x, p.y, p.z, Block::OakLeaves);
        world.set_block_world(p.x + 1, p.y, p.z, Block::OakLog);
        tick_leaf(&mut world, p);
        assert_eq!(world.chunk_block(p.x, p.y, p.z), Block::OakLeaves.id());
    }

    #[test]
    fn leaf_touching_only_leaves_decays() {
        // The old "touches any leaf → survives" rule was wrong: two leaves with no
        // log in reach must both eventually decay. Ticking one decays it.
        let mut world = world_with_centered_chunk();
        let p = IVec3::new(8, 70, 8);
        world.set_block_world(p.x, p.y, p.z, Block::OakLeaves);
        world.set_block_world(p.x, p.y + 1, p.z, Block::OakLeaves);
        tick_leaf(&mut world, p);
        assert_eq!(world.chunk_block(p.x, p.y, p.z), Block::Air.id());
    }

    // The exact step-distance boundary is unit-tested next to the flood itself,
    // in `block::behavior::leaves::tests` (it pins to `MAX_LOG_DISTANCE`, so it
    // survives retuning the reach). These world-level tests cover the qualitative
    // rules and the dispatch path.

    #[test]
    fn leaf_with_unloaded_neighbor_survives() {
        // Leaf at local x=0: its -x neighbour is in chunk (-1,0), which is NOT
        // loaded. An unknown neighbour counts as support, so it must not decay.
        let mut world = world_with_centered_chunk();
        let p = IVec3::new(0, 70, 8);
        world.set_block_world(p.x, p.y, p.z, Block::OakLeaves);
        tick_leaf(&mut world, p);
        assert_eq!(world.chunk_block(p.x, p.y, p.z), Block::OakLeaves.id());
    }

    #[test]
    fn chunk_counter_gates_the_column() {
        let mut world = world_with_centered_chunk();
        let pos = ChunkPos::new(0, 0);
        assert!(!world.chunks.get(&pos).unwrap().has_random_tickable());
        world.set_block_world(8, 70, 8, Block::OakLeaves);
        assert!(world.chunks.get(&pos).unwrap().has_random_tickable());
        world.set_block_world(8, 70, 8, Block::Air);
        assert!(!world.chunks.get(&pos).unwrap().has_random_tickable());
    }

    #[test]
    fn game_tick_eventually_decays_isolated_leaf() {
        // End-to-end: the random-tick loop inside game_tick selects and decays an
        // isolated leaf. Deterministic for the fixed seed; the cap sits far above
        // the ~22k expected ticks (3 picks of 65536 cells per tick).
        let mut world = world_with_centered_chunk();
        let p = IVec3::new(8, 70, 8);
        world.set_block_world(p.x, p.y, p.z, Block::OakLeaves);
        let recipes = Recipes::default();
        let mut decayed = false;
        for _ in 0..1_000_000 {
            world.game_tick(&recipes);
            if world.chunk_block(p.x, p.y, p.z) == Block::Air.id() {
                decayed = true;
                break;
            }
        }
        assert!(decayed, "isolated leaf was never random-ticked into decay");
    }
}
