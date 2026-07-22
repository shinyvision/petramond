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

use rustc_hash::FxHashSet;
use std::cmp::Reverse;
use std::collections::{BinaryHeap, VecDeque};

use crate::block::Block;
use crate::chunk::{SectionPos, SECTION_SIZE, SECTION_VOLUME};
use crate::crafting::Recipes;
use crate::mathh::{IVec3, FACE_NEIGHBORS};

use super::sim_guard::{SimReadiness, SIM_RETRY_DELAY};
use super::store::World;

/// One pending scheduled tick, min-heap ordered: `(due tick, schedule order, x, y, z)`.
type ScheduledTick = Reverse<(u64, u64, i32, i32, i32)>;

/// Random-tick draws per loaded 16³ section per tick.
const RANDOM_TICK_SPEED: u32 = 3;

/// Horizontal eligibility radius (chunks) of the random-tick disc around each
/// player, clamped to `render_dist - 2`. Random ticks are ambience near
/// players (grass creep, leaf decay, sapling growth), not world simulation —
/// at the old `render_dist - 2` (30 chunks at the server default) the scan
/// plus behaviour probes over ~2 800 columns per player burned half the
/// server tick thread on an idle world. 8 chunks (128 blocks)
const RANDOM_TICK_CHUNK_RADIUS: i32 = 8;

/// Per-world tick/update/schedule bookkeeping.
#[derive(Default)]
pub(super) struct TickState {
    /// Monotonic game-tick counter (20 per second).
    tick: u64,
    /// Cells whose neighbourhood changed since the last tick, awaiting dispatch.
    update_queue: VecDeque<IVec3>,
    update_set: FxHashSet<IVec3>,
    /// Pending scheduled ticks ordered by due tick, then by scheduling order
    /// (min-heap via `Reverse`); the position rides along in the entry.
    scheduled: BinaryHeap<ScheduledTick>,
    /// Monotonic counter that timestamps each schedule, so ticks due on the same
    /// game tick execute in the order they were scheduled.
    scheduled_seq: u64,
    /// Positions with a scheduled tick already pending, for dedup.
    scheduled_set: FxHashSet<IVec3>,
    /// Blocks the simulation itself destroyed this tick (a fragile block losing its
    /// support, or one washed away by water), each as `(pos, block)`. Purely a
    /// hand-off to the presentation layer: `Game` drains it right after the tick (see
    /// [`World::take_natural_breaks`]) to play the break burst + roll the drops, so the
    /// visual effect lives in `Game` while the world stays the authority on the change.
    pending_breaks: Vec<(IVec3, crate::block::Block)>,
    /// xorshift64 state for random-tick cell selection (kept non-zero; see
    /// [`TickState::new`]).
    rng: u64,
    /// Reused per-phase batch buffer (scheduled dues, update drain, random-tick
    /// cells). The phases run strictly in sequence, so one buffer serves all
    /// three without a fresh allocation every tick.
    batch_scratch: Vec<IVec3>,
    /// Block positions announced changed since the last mob tick — the feed
    /// for confinement-cache invalidation (`mob::confined::RegionCache`),
    /// drained by `tick_mobs`. Bounded: past [`NAV_CHANGE_CAP`] the overflow
    /// flag stands in for the exact positions (invalidate everything), so a
    /// world that never drains (a pure client) cannot grow it unbounded.
    nav_changes: Vec<IVec3>,
    nav_changes_overflow: bool,
}

/// Cap on the per-tick nav-change buffer (see [`TickState::nav_changes`]).
const NAV_CHANGE_CAP: usize = 256;

/// Whether replacing `old` with `new` provably CANNOT change what a mob can
/// walk on or through — the confinement-invalidation filter for
/// `set_block_world` (the one funnel that holds both blocks). Only shapes
/// whose collision is fully determined by the block id qualify (cube,
/// lowered cube, and the non-colliding decorations); anything whose real
/// boxes resolve from per-cell or neighbour state (doors, models, fences,
/// panes, stairs, slabs, ladders) stays conservatively relevant, as does any
/// water involvement (water is navigation FOOTING). This keeps the heavy pen
/// churn out of the feed — grazed grass (`Cross` → air), crop growth stages,
/// farmland hydration swaps (same 15/16 box) — while a broken wall, a placed
/// fence, or a stone→air edit still invalidates. When unsure, answer `false`.
pub(super) fn edit_nav_equivalent(old: Block, new: Block) -> bool {
    if old == new {
        return true;
    }
    let static_shape = |b: Block| {
        matches!(
            b.shape_family(),
            crate::block::ShapeFamily::Cube
                | crate::block::ShapeFamily::LoweredCube
                | crate::block::ShapeFamily::Cross
                | crate::block::ShapeFamily::Crop
                | crate::block::ShapeFamily::Torch
        ) && !b.is_water()
    };
    static_shape(old) && static_shape(new) && old.collision_boxes() == new.collision_boxes()
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

    /// Seed the tick counter from a save (`level.dat` v7), so scheduled ticks
    /// and tick-anchored state (the `petramond:clock` day cycle) continue across
    /// sessions instead of restarting at 0. Call once at session open, BEFORE
    /// mods initialize — init-time `CurrentTick` host calls must already see
    /// the restored value.
    pub fn restore_tick(&mut self, tick: u64) {
        self.sim.tick = tick;
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
        debug_assert!(
            self.role != crate::world::store::WorldRole::ClientReplica,
            "a replica never simulates: the server owns the tick"
        );
        self.sim.tick = self.sim.tick.wrapping_add(1);
        let now = self.sim.tick;

        // 1. Run scheduled block ticks whose due time has arrived (EXECUTE phase).
        let mut due = std::mem::take(&mut self.sim.batch_scratch);
        due.clear();
        while let Some(&Reverse((d, _, x, y, z))) = self.sim.scheduled.peek() {
            if d > now {
                break;
            }
            self.sim.scheduled.pop();
            let pos = IVec3::new(x, y, z);
            self.sim.scheduled_set.remove(&pos);
            due.push(pos);
        }
        for pos in due.drain(..) {
            // Streaming-finality gate (see `world::sim_guard`): a behaviour must not
            // act on reads of sections whose streamed content is still in flight or
            // absent-and-lying. In-flight blockers resolve within ticks — retry;
            // unloaded blockers only resolve on a load event — drop, and let the
            // on-load water kick re-arm the flow when the terrain streams in.
            match self.sim_readiness_at(pos) {
                SimReadiness::Ready => self.run_scheduled_tick(pos),
                SimReadiness::Wait => self.schedule_block_tick(pos, SIM_RETRY_DELAY),
                SimReadiness::Drop => {}
            }
        }

        // 2. Dispatch the block updates accumulated since the last tick (ANNOUNCE
        //    phase). MUST run after scheduled ticks: collapsing or reordering the
        //    two reorders the simulation.
        if !self.sim.update_queue.is_empty() {
            let mut updates = due; // reuse the drained phase-1 buffer
            updates.extend(self.sim.update_queue.drain(..));
            self.sim.update_set.clear();
            for pos in updates.drain(..) {
                // Same streaming-finality gate as scheduled ticks; a Wait re-queues
                // into the NEXT tick's batch (this tick's snapshot is already taken).
                match self.sim_readiness_at(pos) {
                    SimReadiness::Ready => self.dispatch_block_update(pos),
                    SimReadiness::Wait => {
                        self.queue_block_update(pos);
                    }
                    SimReadiness::Drop => {}
                }
            }
            self.sim.batch_scratch = updates;
        } else {
            self.sim.batch_scratch = due;
        }

        // 3. Smelt every loaded furnace one tick (chunk-owned; cheap when none).
        self.tick_furnaces(recipes);

        // 4. Random block ticks: a few random cells per nearby section get a
        //    probabilistic behaviour callback (today: leaf decay). Cheapest of all
        //    when nothing is tickable — empty sections are skipped by their counter.
        self.random_tick_sections();
    }

    /// Announce that the block at `(wx, wy, wz)` changed: schedule the light
    /// rebake for every section the change can influence, then queue a block
    /// update for the cell itself and each of its 6 orthogonal neighbours
    /// (crossing chunk borders). Block updates are deduped within the current
    /// tick.
    ///
    /// The relight is emitted HERE, alongside the block update, so the two can
    /// never drift apart: this is the single "a block changed" choke point that
    /// every editor calls (`set_block_world`, `set_water_world`, the model and
    /// furnace paths), and none has to remember a matching `mark_light_dirty` of
    /// its own — forgetting one was the bug this consolidates away (water washing
    /// a torch away changes the block light, but the water path never relit). Any
    /// announced change may have moved opacity or an emitter, so any announced
    /// change relights — the only exemption is a caller that PROVED the cell's
    /// old and new contents light-identically (`notify_light_equivalent_change_nav`).
    pub(super) fn notify_block_and_neighbors(&mut self, wx: i32, wy: i32, wz: i32) {
        self.notify_block_change(wx, wy, wz, Self::LIGHT_REACH, true);
    }

    /// The announce for a change whose caller compared the old and new block
    /// and proved them light-equivalent (`Block::has_same_light_behavior`),
    /// or proved the edit sits in darkness no light reaches — delta capture
    /// and block updates run, the relight is skipped. Callers that cannot
    /// prove either use [`Self::notify_block_and_neighbors`].
    /// The announce for a change whose caller proved the old and new cell
    /// contents light-equivalent — delta capture and block updates run, the
    /// relight is skipped. `nav_relevant: false` additionally skips the
    /// confinement-invalidation feed; only `set_block_world` (which holds
    /// both blocks — see [`edit_nav_equivalent`]) may prove that, every
    /// other announce passes `true`.
    pub(super) fn notify_light_equivalent_change_nav(
        &mut self,
        wx: i32,
        wy: i32,
        wz: i32,
        nav_relevant: bool,
    ) {
        self.notify_block_change(wx, wy, wz, -1, nav_relevant);
    }

    /// The announce with the relight bounded to `radius` cells — for a caller
    /// that bounded the edit's influence by the light actually present at the
    /// cell (see `World::edit_light_reach`). `nav_relevant` as on
    /// [`Self::notify_light_equivalent_change_nav`].
    pub(super) fn notify_block_change_with_light_radius_nav(
        &mut self,
        wx: i32,
        wy: i32,
        wz: i32,
        radius: i32,
        nav_relevant: bool,
    ) {
        self.notify_block_change(wx, wy, wz, radius, nav_relevant);
    }

    fn notify_block_change(&mut self, wx: i32, wy: i32, wz: i32, light_radius: i32, nav: bool) {
        // Replication rides the same choke point, for the same reason as the
        // relight: every editor announces here, so no block/water change a
        // client could see can miss the delta log (see `record_block_delta`).
        if self.replication_capture {
            self.record_block_delta(wx, wy, wz);
        }
        // Confinement invalidation rides here too, unless the caller PROVED
        // the change navigationally equivalent (`edit_nav_relevant`).
        if nav {
            self.push_nav_change(IVec3::new(wx, wy, wz));
        }
        if light_radius >= 0 {
            // Persist staleness notes ride the mark (see
            // `mark_light_dirty_around_cell_radius`).
            self.mark_light_dirty_around_cell_radius(wx, wy, wz, light_radius);
        }
        let p = IVec3::new(wx, wy, wz);
        self.queue_block_update(p);
        for d in FACE_NEIGHBORS {
            self.queue_block_update(p + d);
        }
    }

    /// Record one changed position for the confinement-invalidation feed
    /// (see [`TickState::nav_changes`]). Also the direct entry for mutation
    /// funnels that never pass the announce choke point — the door toggle
    /// flips collision through the door map with no block write.
    pub(super) fn push_nav_change(&mut self, pos: IVec3) {
        if self.sim.nav_changes_overflow {
            return;
        }
        if self.sim.nav_changes.len() >= NAV_CHANGE_CAP {
            self.sim.nav_changes.clear();
            self.sim.nav_changes_overflow = true;
        } else {
            self.sim.nav_changes.push(pos);
        }
    }

    /// Drain the changed-block buffer feeding confinement-cache invalidation
    /// (see [`TickState::nav_changes`]): every position announced since the
    /// last drain, plus whether the buffer overflowed (positions unknown —
    /// the consumer must invalidate everything).
    pub(super) fn take_nav_changes(&mut self) -> (Vec<IVec3>, bool) {
        let overflow = self.sim.nav_changes_overflow;
        self.sim.nav_changes_overflow = false;
        (std::mem::take(&mut self.sim.nav_changes), overflow)
    }

    pub(super) fn queue_block_update(&mut self, pos: IVec3) -> bool {
        if self.sim.update_set.insert(pos) {
            self.sim.update_queue.push_back(pos);
            true
        } else {
            false
        }
    }

    /// Public [`schedule_block_tick`](Self::schedule_block_tick): the mod
    /// `ScheduleTick` HostCall's entry, same first-schedule-wins semantics.
    pub fn schedule_tick(&mut self, pos: IVec3, delay: u64) {
        self.schedule_block_tick(pos, delay);
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
        // Sweep any block-entity record the block owned (a torch's mount, a
        // chest/furnace front) — the same unconditional sweep the player-break
        // path uses, so no per-block arm is needed here and the block-entity
        // section index stays in sync.
        self.forget_block_entity_records(pos);
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
        // Natural breaks leave the same residue a player break would (air for
        // almost everything; melting ice leaves water — `Block::break_residue`).
        let below = Block::from_id(self.chunk_block(pos.x, pos.y - 1, pos.z));
        self.set_block_world(pos.x, pos.y, pos.z, block.break_residue(below));
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

    /// Random block ticks: for each loaded 16³ section near a player that holds
    /// any random-tickable block, pick [`RANDOM_TICK_SPEED`] cells uniformly from
    /// that section and run each one's behaviour. Air — the vast bulk of many
    /// sections — is skipped on the spot, and a section with nothing tickable is
    /// skipped wholesale via its counter, so the cost is a few RNG draws and array
    /// reads per section.
    ///
    /// Eligibility is a disc of [`RANDOM_TICK_CHUNK_RADIUS`] chunks around EVERY
    /// streaming anchor (each connected player), never wider than
    /// `render_dist - 2` so a ticked section's horizontal neighbours are loaded
    /// (the leaf-decay flood reaches a few blocks across borders). A cell that is
    /// *still* unloaded is treated as support, so nothing decays on missing
    /// information. Anchors iterate in session order and a column covered by an
    /// earlier anchor is skipped, so overlapping discs stay deterministic and
    /// tick once.
    fn random_tick_sections(&mut self) {
        let Some(primary) = self.last_load_target else {
            return;
        };
        // (center, radius) per anchor; radii can differ only via render_dist.
        let mut anchors: Vec<(crate::chunk::ChunkPos, i32)> =
            Vec::with_capacity(1 + self.extra_load_targets.len());
        for t in std::iter::once(&primary).chain(self.extra_load_targets.iter()) {
            anchors.push((
                t.center,
                RANDOM_TICK_CHUNK_RADIUS.min((t.render_dist - 2).max(0)),
            ));
        }

        // Gather phase: choose the cells to tick WITHOUT holding a section-map
        // borrow across the dispatch (which mutates the world). `self.sim` and
        // `self.sections` are disjoint fields, so the RNG draw and the block reads
        // borrow side by side.
        let mut due = std::mem::take(&mut self.sim.batch_scratch);
        due.clear();
        for (i, &(center, r)) in anchors.iter().enumerate() {
            for dz in -r..=r {
                for dx in -r..=r {
                    if dx * dx + dz * dz > r * r {
                        continue;
                    }
                    let cx = center.cx + dx;
                    let cz = center.cz + dz;
                    // Covered by an earlier anchor: already ticked this pass.
                    if anchors[..i].iter().any(|&(c, cr)| {
                        let ddx = cx - c.cx;
                        let ddz = cz - c.cz;
                        ddx * ddx + ddz * ddz <= cr * cr
                    }) {
                        continue;
                    }
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
        }

        // Dispatch phase: the section-map borrow is released, so each behaviour is
        // free to edit the world (a decaying leaf sets air, which relights/remeshes).
        // Random ticks are probabilistic, so the streaming-finality gate simply
        // skips a cell whose neighbourhood is not final — same as not picking it.
        for pos in due.drain(..) {
            if self.sim_readiness_at(pos) == SimReadiness::Ready {
                self.run_random_tick(pos);
            }
        }
        self.sim.batch_scratch = due;
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
    fn only_walkability_changes_feed_the_confinement_invalidation() {
        let mut world = world_with_centered_chunk();
        let p = IVec3::new(8, 70, 8);
        world.set_block_world(p.x, p.y - 1, p.z, Block::Grass);
        let _ = world.take_nav_changes();

        // A decoration appearing/vanishing (a tuft grazed to air, a crop
        // stage) can't change what a mob walks on: filtered out.
        world.set_block_world(p.x, p.y, p.z, Block::ShortGrass);
        world.set_block_world(p.x, p.y, p.z, Block::Air);
        assert_eq!(world.take_nav_changes(), (vec![], false));

        // A wall appearing very much can.
        world.set_block_world(p.x, p.y, p.z, Block::Stone);
        assert_eq!(world.take_nav_changes(), (vec![p], false));

        // A door toggle bypasses the announce choke point entirely and must
        // feed the invalidation on its own.
        world.set_block_world(p.x, p.y, p.z, Block::Air);
        let door = IVec3::new(8, 70, 9);
        assert!(world.place_door(door, Block::OakDoor, crate::facing::Facing::South));
        let _ = world.take_nav_changes();
        assert!(world.toggle_door(door).is_some());
        let (changed, overflow) = world.take_nav_changes();
        assert!(!overflow && changed.contains(&door), "toggle invalidates: {changed:?}");
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

    /// Random ticks must cover EVERY streaming anchor, not just the primary —
    /// on a multi-player server the second player's surroundings used to get
    /// no random ticks at all (no leaf decay, no grass spread) because the
    /// disc was centred only on `last_load_target`.
    #[test]
    fn random_ticks_reach_a_second_anchors_surroundings() {
        let mut world = World::new(1, 4);
        // Primary anchor far away; the leaf lives near the EXTRA anchor only.
        world.insert_empty_column_for_test(ChunkPos::new(40, 0));
        world.last_load_target = Some(LoadTarget::new(0, 4, 0, 4));
        world.extra_load_targets = vec![LoadTarget::new(40, 4, 0, 4)];
        let p = IVec3::new(40 * 16 + 8, 70, 8);
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
        assert!(
            decayed,
            "an isolated leaf near the second anchor never random-ticked into decay"
        );
    }
}
