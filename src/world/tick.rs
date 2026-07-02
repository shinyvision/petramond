//! Fixed-timestep world simulation: the 20 TPS game tick, neighbour "block
//! updates", scheduled block ticks, and random ticks.
//!
//! The generic loops never name a concrete block: every reaction — random tick,
//! neighbour update, and scheduled tick — routes through the block's
//! [`behavior`](crate::block::behavior). A behaviour needing only World's public
//! api (leaf decay) lives in `block`; one reaching into world internals (water,
//! which drives `FluidSim` and the tick scheduler) lives in `world` and still
//! implements the `block`-defined trait.
//!
//! A reaction receives `&mut World` and never stores world state on a block.
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
//!   (water schedules its flow check `WATER_FLOW_DELAY` ticks after a
//!   disturbance). A min-heap keyed by (due tick, schedule order), deduped by
//!   position so repeated disturbances collapse to one pending check — the
//!   first schedule wins, and ticks due together run in the order they were
//!   scheduled, so simultaneous flows advance as wavefronts instead of in
//!   coordinate order.
//! - **Random ticks**: each tick, [`RANDOM_TICK_SPEED`] uniformly-random cells in
//!   every loaded 16³ section near the player get a probabilistic behaviour
//!   callback. Air picks are skipped on the spot and sections with nothing
//!   random-tickable are skipped wholesale via a per-section counter.

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashSet, VecDeque};

use crate::block::Block;
use crate::chunk::{SectionPos, SECTION_SIZE, SECTION_VOLUME};
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

/// One pending scheduled tick, min-heap ordered: `(due tick, schedule order, x, y, z)`.
type ScheduledTick = Reverse<(u64, u64, i32, i32, i32)>;

/// Random-tick draws per loaded 16³ section per tick.
const RANDOM_TICK_SPEED: u32 = 3;

/// Per-world tick/update/schedule bookkeeping.
#[derive(Default)]
pub(super) struct TickState {
    /// Monotonic game-tick counter (20 per second).
    tick: u64,
    /// Cells whose neighbourhood changed since the last tick, awaiting dispatch.
    update_queue: VecDeque<IVec3>,
    update_set: HashSet<IVec3>,
    /// Pending scheduled ticks ordered by due tick, then by scheduling order
    /// (min-heap via `Reverse`); the position rides along in the entry.
    scheduled: BinaryHeap<ScheduledTick>,
    /// Monotonic counter that timestamps each schedule, so ticks due on the same
    /// game tick execute in the order they were scheduled.
    scheduled_seq: u64,
    /// Positions with a scheduled tick already pending, for dedup.
    scheduled_set: HashSet<IVec3>,
    /// Blocks the simulation itself destroyed this tick (a fragile block losing its
    /// support, or one washed away by water), each as `(pos, block)`. Purely a
    /// hand-off to the presentation layer: `Game` drains it right after the tick (see
    /// [`World::take_natural_breaks`]) to play the break burst + roll the drops, so the
    /// visual effect lives in `Game` while the world stays the authority on the change.
    pending_breaks: Vec<(IVec3, crate::block::Block)>,
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
        while let Some(&Reverse((d, _, x, y, z))) = self.sim.scheduled.peek() {
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

        // 4. Random block ticks: a few random cells per nearby section get a
        //    probabilistic behaviour callback (today: leaf decay). Cheapest of all
        //    when nothing is tickable — empty sections are skipped by their counter.
        self.random_tick_sections();
    }

    /// Announce that the block at `(wx, wy, wz)` changed: schedule the light
    /// rebake for its 3×3 chunk neighbourhood, then queue a block update for the
    /// cell itself and each of its 6 orthogonal neighbours (crossing chunk
    /// borders). Block updates are deduped within the current tick.
    ///
    /// The relight is emitted HERE, alongside the block update, so the two can
    /// never drift apart: this is the single "a block changed" choke point that
    /// every editor calls (`set_block_world`, `set_water_world`, the model and
    /// furnace paths), and none has to remember a matching `mark_light_dirty` of
    /// its own — forgetting one was the bug this consolidates away (water washing
    /// a torch away changes the block light, but the water path never relit). Any
    /// announced change may have moved opacity or an emitter, so any announced
    /// change relights. The 3×3 covers the border flood: a cell's light can spill
    /// one chunk in every direction.
    pub(super) fn notify_block_and_neighbors(&mut self, wx: i32, wy: i32, wz: i32) {
        if let Some(sp) = SectionPos::from_world(wx, wy, wz) {
            self.mark_light_dirty_neighborhood(sp, true);
        }
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
            let seq = self.sim.scheduled_seq;
            self.sim.scheduled_seq += 1;
            self.sim
                .scheduled
                .push(Reverse((due, seq, pos.x, pos.y, pos.z)));
        }
    }

    /// Record that `block` at `pos` was destroyed by the simulation itself — a fragile
    /// block that lost its support, or one a fluid washed away — so the presentation
    /// layer gives it the same break a player's would: the particle burst plus the
    /// block's rolled item drops (drained by `Game` via [`take_natural_breaks`] right
    /// after this tick). Also forgets any block-entity state the block owned (a torch's
    /// recorded orientation). Does NOT clear the cell: the caller writes the new
    /// occupant — air for a support loss, water when a fluid took its place.
    ///
    /// [`take_natural_breaks`]: Self::take_natural_breaks
    pub(crate) fn note_block_destroyed(&mut self, pos: IVec3, block: Block) {
        self.sim.pending_breaks.push((pos, block));
        // A torch keeps its mount direction in the chunk's torch map; clear it so the
        // freed cell carries no stale orientation (mirrors the player-break path).
        if block == Block::Torch {
            self.take_torch(pos);
        }
    }

    /// Take the blocks the simulation destroyed this tick (see [`note_block_destroyed`]),
    /// leaving the queue empty. `Game` calls this immediately after [`game_tick`] to
    /// spawn each one's break burst + drops on the same tick.
    ///
    /// [`note_block_destroyed`]: Self::note_block_destroyed
    /// [`game_tick`]: Self::game_tick
    pub fn take_natural_breaks(&mut self) -> Vec<(IVec3, Block)> {
        std::mem::take(&mut self.sim.pending_breaks)
    }

    /// Destroy the block at `pos` the way the simulation does when it is lost — a
    /// fragile block undermined, or a leaf decaying — by handing it the same break a
    /// player's hand would: record it as a natural break (so `Game` plays the burst
    /// and rolls its drops, e.g. a decayed leaf's 10% sapling) and clear the cell to
    /// air. Reads the current occupant at `pos`; a no-op if that cell is already air.
    pub(crate) fn break_block_naturally(&mut self, pos: IVec3) {
        let block = Block::from_id(self.chunk_block(pos.x, pos.y, pos.z));
        if block == Block::Air {
            return;
        }
        self.note_block_destroyed(pos, block);
        self.set_block_world(pos.x, pos.y, pos.z, Block::Air);
    }

    /// Generic ANNOUNCE step: a neighbour of `pos` changed. Read the block there
    /// and route it to that block's [`behavior`](crate::block::behavior). Names no
    /// concrete block — water (and any future reactor) carries its own reaction.
    fn dispatch_block_update(&mut self, pos: IVec3) {
        let block = Block::from_id(self.chunk_block(pos.x, pos.y, pos.z));
        block.behavior().neighbor_update(self, pos);
    }

    /// Generic EXECUTE step: run the scheduled behaviour for the block at `pos`.
    /// Read the block there and route it to that block's behaviour. Names no
    /// concrete block.
    fn run_scheduled_tick(&mut self, pos: IVec3) {
        let block = Block::from_id(self.chunk_block(pos.x, pos.y, pos.z));
        block.behavior().scheduled_tick(self, pos);
    }

    /// Random block ticks: for each loaded 16³ section near the player that holds
    /// any random-tickable block, pick [`RANDOM_TICK_SPEED`] cells uniformly from
    /// that section and run each one's behaviour. Air — the vast bulk of many
    /// sections — is skipped on the spot, and a section with nothing tickable is
    /// skipped wholesale via its counter, so the cost is a few RNG draws and array
    /// reads per section.
    ///
    /// Eligibility is a disc of `render_dist - 2` chunks around the player, so a
    /// ticked section's horizontal neighbours are loaded (the leaf-decay flood
    /// reaches a few blocks across borders). A cell that is *still* unloaded is
    /// treated as support, so nothing decays on missing information.
    fn random_tick_sections(&mut self) {
        let Some(target) = self.last_load_target else {
            return;
        };
        let center = target.center;
        let r = (target.render_dist - 2).max(0);

        // Gather phase: choose the cells to tick WITHOUT holding a section-map
        // borrow across the dispatch (which mutates the world). `self.sim` and
        // `self.sections` are disjoint fields, so the RNG draw and the block reads
        // borrow side by side.
        let mut due: Vec<IVec3> = Vec::new();
        for dz in -r..=r {
            for dx in -r..=r {
                if dx * dx + dz * dz > r * r {
                    continue;
                }
                let cx = center.cx + dx;
                let cz = center.cz + dz;
                for cy in Self::column_section_range() {
                    let Some(section) = self.sections.get(&SectionPos::new(cx, cy, cz)) else {
                        continue;
                    };
                    if !section.has_random_tickable() {
                        continue;
                    }
                    let (ox, oy, oz) = SectionPos::new(cx, cy, cz).origin_world();
                    let blocks = section.blocks_slice();
                    for _ in 0..RANDOM_TICK_SPEED {
                        let i = (self.sim.next_random() >> 16) as usize % SECTION_VOLUME;
                        let id = blocks[i];
                        if id == 0 {
                            continue; // air — nothing ticks; the overwhelming majority
                        }
                        if !Block::from_id(id).has_random_tick() {
                            continue;
                        }
                        // Decode the flat section index back to world coords (only for a hit).
                        let lx = i & (SECTION_SIZE - 1);
                        let lz = (i >> 4) & (SECTION_SIZE - 1);
                        let ly = i >> 8;
                        due.push(IVec3::new(ox + lx as i32, oy + ly as i32, oz + lz as i32));
                    }
                }
            }
        }

        // Dispatch phase: the section-map borrow is released, so each behaviour is
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
    use crate::chunk::ChunkPos;
    use crate::crafting::Recipes;

    use super::super::store::LoadTarget;

    /// A world with one empty loaded column at (0,0) (every section present, all air)
    /// and the player centred on it, so its sections are eligible for random ticks.
    fn world_with_centered_chunk() -> World {
        let mut world = World::new(1, 4);
        world.insert_empty_column_for_test(ChunkPos::new(0, 0));
        world.last_load_target = Some(LoadTarget::new(0, 4, 0, 4));
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
    fn section_counter_gates_the_section() {
        let mut world = world_with_centered_chunk();
        // The leaf at (8,70,8) lives in section (0,4,0); the counter gates that section.
        let tickable = |w: &World| {
            w.section_at_world_for_test(8, 70, 8)
                .unwrap()
                .has_random_tickable()
        };
        assert!(!tickable(&world));
        world.set_block_world(8, 70, 8, Block::OakLeaves);
        assert!(tickable(&world));
        world.set_block_world(8, 70, 8, Block::Air);
        assert!(!tickable(&world));
    }

    #[test]
    fn game_tick_eventually_decays_isolated_leaf() {
        // End-to-end: the random-tick loop inside game_tick selects and decays an
        // isolated leaf. Deterministic for the fixed seed; the cap sits far above
        // the ~1.4k expected ticks (3 picks of 4096 cells per tick in its section).
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
