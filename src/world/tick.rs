use crate::world::{ServerWorld, World, WorldSide};

use petramond_math::math::{IVec3, FACE_NEIGHBORS};
use petramond_world::block::Block;
use petramond_world::chunk::{SectionPos, SECTION_SIZE, SECTION_VOLUME};
use petramond_world::crafting::Recipes;

use super::sim_guard::{SimReadiness, SIM_RETRY_DELAY};

mod region;

pub const TICK_DT: f32 = 0.05;

const RANDOM_TICK_SPEED: u32 = 3;

const RANDOM_TICK_CHUNK_RADIUS: i32 = 8;

pub(super) fn edit_nav_equivalent(old: Block, new: Block) -> bool {
    if old == new {
        return true;
    }
    let static_shape = |b: Block| b.nav_follows_row() && b.fluid().is_none();
    static_shape(old)
        && static_shape(new)
        && old.has_tag(petramond_world::block::BlockTag::NAV_HAZARD)
            == new.has_tag(petramond_world::block::BlockTag::NAV_HAZARD)
        && old.collision_boxes() == new.collision_boxes()
}

impl<S: WorldSide> World<S> {
    #[inline]
    pub fn current_tick(&self) -> u64 {
        self.data.sim.tick
    }

    pub(super) fn notify_block_and_neighbors(&mut self, wx: i32, wy: i32, wz: i32) {
        self.notify_block_change(wx, wy, wz, Self::LIGHT_REACH, true);
    }

    pub(super) fn notify_light_equivalent_change_nav(
        &mut self,
        wx: i32,
        wy: i32,
        wz: i32,
        nav_relevant: bool,
    ) {
        self.notify_block_change(wx, wy, wz, -1, nav_relevant);
    }

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
        self.record_cell_change(IVec3::new(wx, wy, wz), light_radius, nav);
        self.queue_cell_and_neighbor_updates(IVec3::new(wx, wy, wz));
    }

    pub(super) fn record_cell_change(&mut self, p: IVec3, light_radius: i32, nav: bool) {
        self.record_block_delta(p.x, p.y, p.z);
        self.data.sim.change_log.push(p, nav);
        self.relight_cell(p.x, p.y, p.z, light_radius);
    }

    pub(super) fn queue_cell_and_neighbor_updates(&mut self, p: IVec3) {
        self.queue_block_update(p);
        for d in FACE_NEIGHBORS {
            self.queue_block_update(p + d);
        }
    }

    pub(super) fn push_nav_change(&mut self, pos: IVec3) {
        self.data.sim.change_log.push(pos, true);
    }

    pub fn changes_since(&self, seq: u64) -> (u64, Vec<IVec3>, bool) {
        let (cells, lost) = self.data.sim.change_log.since(seq);
        (self.data.sim.change_log.end(), cells, lost)
    }

    pub fn nav_changes_since(&self, seq: u64) -> (u64, Vec<IVec3>, bool) {
        let (cells, lost) = self.data.sim.change_log.nav_since(seq);
        (self.data.sim.change_log.end(), cells, lost)
    }

    pub fn changes_end(&self) -> u64 {
        self.data.sim.change_log.end()
    }

    #[inline]
    pub fn nav_revision(&self) -> u64 {
        self.data.sim.change_log.nav_revision()
    }

    pub(super) fn queue_block_update(&mut self, pos: IVec3) -> bool {
        if self.data.sim.update_set.insert(pos) {
            self.data.sim.update_queue.push_back(pos);
            true
        } else {
            false
        }
    }

    pub fn schedule_tick(&mut self, pos: IVec3, delay: u64) {
        self.schedule_block_tick(pos, delay);
    }

    pub(super) fn schedule_block_tick(&mut self, pos: IVec3, delay: u64) {
        let due = self.data.sim.tick.wrapping_add(delay);
        self.data.sim.scheduled.schedule(pos, due);
    }

    pub(super) fn schedule_fluid_tick(&mut self, pos: IVec3, delay: u64) {
        let due = self.data.sim.tick.wrapping_add(delay);
        self.data.sim.fluid.schedule(pos, due);
    }

    pub fn note_block_destroyed(&mut self, pos: IVec3, block: Block) {
        self.data.sim.pending_breaks.push((pos, block));
        self.forget_block_entity_records(pos);
    }

    pub fn break_block_naturally(&mut self, pos: IVec3) {
        let block = Block::from_id(self.data.chunk_block(pos.x, pos.y, pos.z));
        if block == Block::Air {
            return;
        }
        self.note_block_destroyed(pos, block);
        let below = Block::from_id(self.data.chunk_block(pos.x, pos.y - 1, pos.z));
        self.set_block_world(pos.x, pos.y, pos.z, block.break_residue(below));
    }
}

#[inline]
fn cell_pos(ox: i32, oy: i32, oz: i32, i: usize) -> IVec3 {
    let lx = i & (SECTION_SIZE - 1);
    let lz = (i >> 4) & (SECTION_SIZE - 1);
    let ly = i >> 8;
    IVec3::new(ox + lx as i32, oy + ly as i32, oz + lz as i32)
}

impl ServerWorld {
    pub fn restore_tick(&mut self, tick: u64) {
        self.data.sim.tick = tick;
    }

    /// Fixed 50ms step, runs every tick even if nothing's pending, so cadence stays independent of
    /// activity. All per-tick ordering lives here in one place.
    ///
    /// Order matters, don't reshuffle it:
    /// 1. Due scheduled block ticks first (may set blocks, queuing new updates), then due fluid
    ///    checks up to the fluid budget (`ServerWorld::run_fluid_checks`).
    /// 2. Drain the block update queue. Support-loss reactions (fragile, door, grass) decide right
    ///    at the update, and any resulting write queues fresh updates for next tick's batch, so
    ///    the drain always finishes. Support chains still collapse one cell per tick.
    /// 3. Furnace smelting on the same clock. It needs `recipes`, which storage stays ignorant of
    ///    (see `World::tick_furnaces`).
    /// 4. Random block ticks near the player, e.g. leaf decay. Their order relative to the rest
    ///    doesn't matter.
    ///
    /// Item physics and lifetime/pickup live in `Game` instead (per-frame pacing, needs the player
    /// inventory). Everything the world alone owns gets sequenced here.
    pub fn game_tick(&mut self, recipes: &Recipes) {
        self.data.sim.tick = self.data.sim.tick.wrapping_add(1);
        let now = self.data.sim.tick;

        let mut due = std::mem::take(&mut self.data.sim.batch_scratch);
        due.clear();
        while let Some(pos) = self.data.sim.scheduled.pop_due(now) {
            due.push(pos);
        }
        for pos in due.drain(..) {
            match self.sim_readiness_at(pos) {
                SimReadiness::Ready => self.run_scheduled_tick(pos),
                SimReadiness::Wait => self.schedule_block_tick(pos, SIM_RETRY_DELAY),
                SimReadiness::Drop => {}
            }
        }

        self.run_fluid_checks(now, &mut due);

        if !self.data.sim.update_queue.is_empty() {
            let mut updates = due;
            updates.extend(self.data.sim.update_queue.drain(..));
            self.data.sim.update_set.clear();
            for pos in updates.drain(..) {
                match self.sim_readiness_at(pos) {
                    SimReadiness::Ready => self.dispatch_block_update(pos),
                    SimReadiness::Wait => {
                        self.queue_block_update(pos);
                    }
                    SimReadiness::Drop => {}
                }
            }
            self.data.sim.batch_scratch = updates;
        } else {
            self.data.sim.batch_scratch = due;
        }

        self.tick_furnaces(recipes);

        self.random_tick_sections();
    }

    pub fn take_natural_breaks(&mut self) -> Vec<(IVec3, Block)> {
        std::mem::take(&mut self.data.sim.pending_breaks)
    }

    fn dispatch_block_update(&mut self, pos: IVec3) {
        let block = Block::from_id(self.data.chunk_block(pos.x, pos.y, pos.z));
        let behavior = block.behavior();
        match crate::world::engine_behavior::engine_behavior(behavior.key()) {
            Some(engine) => engine.neighbor_update(self, pos),
            None => behavior.neighbor_update(self, pos),
        }
    }

    fn run_scheduled_tick(&mut self, pos: IVec3) {
        let block = Block::from_id(self.data.chunk_block(pos.x, pos.y, pos.z));
        let behavior = block.behavior();
        match crate::world::engine_behavior::engine_behavior(behavior.key()) {
            Some(engine) => engine.scheduled_tick(self, pos),
            None => behavior.scheduled_tick(self, pos),
        }
    }

    /// Random ticks: [`RANDOM_TICK_SPEED`] random cells per loaded section with a tickable block.
    /// Sections come from the per-column random-tick index and each draw is answered by the
    /// section's tickable-cell mask, so the scan never touches block storage.
    ///
    /// Only sections within [`RANDOM_TICK_CHUNK_RADIUS`] chunks of a player tick, never past
    /// `render_dist - 2`, because leaf decay looks across chunk borders and needs neighbours
    /// loaded. Unloaded cells count as support. Players go in session order and a column already
    /// covered gets skipped, so overlapping discs only tick once.
    fn random_tick_sections(&mut self) {
        self.repair_random_tick_index();
        if self.data.last_load_target.is_none() {
            return;
        }
        let anchors: Vec<(petramond_world::chunk::ChunkPos, i32)> = self
            .data
            .last_load_target
            .iter()
            .chain(&self.data.extra_load_targets)
            .map(|t| {
                (
                    t.center,
                    RANDOM_TICK_CHUNK_RADIUS.min((t.render_dist - 2).max(0)),
                )
            })
            .collect();

        let mut due = std::mem::take(&mut self.data.sim.batch_scratch);
        due.clear();
        let data = &mut self.data;
        data.random_tick_index.refresh_ids();
        let index = &mut data.random_tick_index;
        for (a, &(center, r)) in anchors.iter().enumerate() {
            for dz in -r..=r {
                for dx in -r..=r {
                    if dx * dx + dz * dz > r * r {
                        continue;
                    }
                    let cx = center.cx + dx;
                    let cz = center.cz + dz;
                    let covered_earlier = anchors[..a].iter().any(|&(c, r)| {
                        let (ex, ez) = (cx - c.cx, cz - c.cz);
                        ex * ex + ez * ez <= r * r
                    });
                    if covered_earlier {
                        continue;
                    }
                    let column = petramond_world::chunk::ChunkPos::new(cx, cz);
                    let Some(entry) = index.columns.get_mut(&column) else {
                        continue;
                    };
                    let mut cys = entry.bits();
                    while cys != 0 {
                        let slot = cys.trailing_zeros() as usize;
                        cys &= cys - 1;
                        let cy = petramond_world::chunk::SECTION_MIN_CY + slot as i32;
                        let pos = SectionPos::new(cx, cy, cz);
                        let (ox, oy, oz) = pos.origin_world();
                        let sections = &data.sections;
                        let Some(mask) =
                            entry.mask(slot, || sections.get(&pos).map(|s| &**s), &index.tickable)
                        else {
                            continue;
                        };
                        for _ in 0..RANDOM_TICK_SPEED {
                            let i = (data.sim.next_random() >> 16) as usize % SECTION_VOLUME;
                            if mask.get(i) {
                                due.push(cell_pos(ox, oy, oz, i));
                            }
                        }
                    }
                }
            }
        }

        for pos in due.drain(..) {
            if self.sim_readiness_at(pos) == SimReadiness::Ready {
                self.run_random_tick(pos);
            }
        }
        self.data.sim.batch_scratch = due;
    }

    fn run_random_tick(&mut self, pos: IVec3) {
        let block = Block::from_id(self.data.chunk_block(pos.x, pos.y, pos.z));
        let behavior = block.behavior();
        match crate::world::engine_behavior::engine_behavior(behavior.key()) {
            Some(engine) => engine.random_tick(self, pos),
            None => behavior.random_tick(self, pos),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use petramond_world::chunk::ChunkPos;

    use super::super::store::LoadTarget;

    fn world_with_centered_chunk() -> ServerWorld {
        let mut world = ServerWorld::new(1, 4);
        world.insert_empty_column_for_test(ChunkPos::new(0, 0));
        world.data.last_load_target = Some(LoadTarget::new(0, 4, 0, 4));
        world
    }

    fn brute_random_tick_index(world: &ServerWorld) -> Vec<(ChunkPos, u32)> {
        let mut out: std::collections::BTreeMap<(i32, i32), u32> = Default::default();
        for (pos, section) in &world.data.sections {
            if section.has_random_tickable() {
                *out.entry((pos.cx, pos.cz)).or_insert(0) |=
                    crate::world::store::column_cy_bit(pos.cy);
            }
        }
        out.into_iter()
            .map(|((cx, cz), bits)| (ChunkPos::new(cx, cz), bits))
            .collect()
    }

    fn live_random_tick_index(world: &ServerWorld) -> Vec<(ChunkPos, u32)> {
        let mut out: Vec<(ChunkPos, u32)> = world
            .data
            .random_tick_index
            .columns
            .iter()
            .map(|(&p, e)| (p, e.bits()))
            .collect();
        out.sort_by_key(|(p, _)| (p.cx, p.cz));
        assert!(out.iter().all(|(_, b)| *b != 0), "empty column entry kept");
        out
    }

    /// The random-tick scan walks a per-column bitset of sections that hold
    /// something tickable; the index is repaired from the edits announced
    /// since the last tick, so a stale bit would silently stop (or resurrect)
    /// leaf decay and crop growth in a whole 16³ section.
    #[test]
    fn random_tickable_index_matches_a_full_rederive_through_edits() {
        let mut world = world_with_centered_chunk();
        world.repair_random_tick_index();
        assert!(live_random_tick_index(&world).is_empty());

        let leaf = IVec3::new(8, 70, 8);
        world.set_block_world(leaf.x, leaf.y, leaf.z, Block::OakLeaves);
        world.repair_random_tick_index();
        assert_eq!(
            live_random_tick_index(&world),
            brute_random_tick_index(&world)
        );
        assert_eq!(live_random_tick_index(&world).len(), 1);

        let leaf2 = IVec3::new(9, 71, 8);
        let far = IVec3::new(8, 40, 8);
        world.set_block_world(leaf2.x, leaf2.y, leaf2.z, Block::OakLeaves);
        world.set_block_world(far.x, far.y, far.z, Block::OakLeaves);
        world.repair_random_tick_index();
        assert_eq!(
            live_random_tick_index(&world),
            brute_random_tick_index(&world)
        );

        world.set_block_world(leaf.x, leaf.y, leaf.z, Block::Air);
        world.repair_random_tick_index();
        assert_eq!(
            live_random_tick_index(&world),
            brute_random_tick_index(&world)
        );
        assert_eq!(live_random_tick_index(&world).len(), 1);

        world.set_block_world(leaf2.x, leaf2.y, leaf2.z, Block::Air);
        world.set_block_world(far.x, far.y, far.z, Block::Air);
        world.repair_random_tick_index();
        assert_eq!(
            live_random_tick_index(&world),
            brute_random_tick_index(&world)
        );
        assert!(live_random_tick_index(&world).is_empty());
    }

    fn assert_masks_match_blocks(world: &mut ServerWorld) {
        world.repair_random_tick_index();
        let data = &mut world.data;
        data.random_tick_index.refresh_ids();
        let index = &mut data.random_tick_index;
        for (&column, entry) in index.columns.iter_mut() {
            let mut bits = entry.bits();
            while bits != 0 {
                let slot = bits.trailing_zeros() as usize;
                bits &= bits - 1;
                let pos = SectionPos::new(
                    column.cx,
                    petramond_world::chunk::SECTION_MIN_CY + slot as i32,
                    column.cz,
                );
                let section = data.sections.get(&pos).expect("indexed section is loaded");
                let mask = entry
                    .mask(slot, || Some(&**section), &index.tickable)
                    .expect("mask");
                for i in 0..SECTION_VOLUME {
                    let id = section.blocks().get(i);
                    assert_eq!(
                        mask.get(i),
                        id != 0 && Block::from_id(id).has_random_tick(),
                        "cell {i} of {pos:?}"
                    );
                }
            }
        }
    }

    /// The scan answers "is this cell tickable" from a cached per-section mask; an edit must
    /// drop it, or a planted sapling or a new leaf would never tick (and a removed one would).
    #[test]
    fn random_tick_masks_follow_block_edits() {
        let mut world = world_with_centered_chunk();
        let leaf = IVec3::new(8, 70, 8);
        world.set_block_world(leaf.x, leaf.y, leaf.z, Block::OakLeaves);
        assert_masks_match_blocks(&mut world);
        world.set_block_world(leaf.x + 1, leaf.y, leaf.z, Block::OakLeaves);
        world.set_block_world(leaf.x, leaf.y + 1, leaf.z, Block::Dirt);
        assert_masks_match_blocks(&mut world);
        world.set_block_world(leaf.x, leaf.y, leaf.z, Block::Stone);
        assert_masks_match_blocks(&mut world);

        let sp = SectionPos::from_world(leaf.x, leaf.y, leaf.z).expect("in range");
        let mut replacement = petramond_world::section::Section::new(sp.cx, sp.cy, sp.cz);
        replacement.set_block(1, 2, 3, Block::OakLeaves);
        world.insert_section_for_test(sp, replacement);
        assert_masks_match_blocks(&mut world);
    }

    #[test]
    fn random_tickable_index_retires_an_unloaded_section() {
        let mut world = world_with_centered_chunk();
        let leaf = IVec3::new(8, 70, 8);
        world.set_block_world(leaf.x, leaf.y, leaf.z, Block::OakLeaves);
        world.repair_random_tick_index();
        assert_eq!(live_random_tick_index(&world).len(), 1);

        world.remove_column(ChunkPos::new(0, 0));
        world.repair_random_tick_index();
        assert_eq!(
            live_random_tick_index(&world),
            brute_random_tick_index(&world)
        );
        assert!(live_random_tick_index(&world).is_empty());
    }

    fn tick_leaf(world: &mut ServerWorld, p: IVec3) {
        Block::OakLeaves.behavior().random_tick(world, p);
    }

    #[test]
    fn only_walkability_changes_feed_the_confinement_invalidation() {
        let mut world = world_with_centered_chunk();
        let p = IVec3::new(8, 70, 8);
        world.set_block_world(p.x, p.y - 1, p.z, Block::Grass);
        let mut seq = world.changes_end();
        let mut take_nav_changes = |world: &ServerWorld| {
            let (next, changed, lost) = world.nav_changes_since(seq);
            seq = next;
            (changed, lost)
        };

        world.set_block_world(p.x, p.y, p.z, Block::ShortGrass);
        world.set_block_world(p.x, p.y, p.z, Block::Air);
        assert_eq!(take_nav_changes(&world), (vec![], false));

        for fluid in [Block::Water, Block::Lava] {
            world.set_block_world(p.x, p.y, p.z, fluid);
            let (changed, overflow) = take_nav_changes(&world);
            assert!(!overflow && changed.contains(&p), "fluid entry invalidates");
            world.set_block_world(p.x, p.y, p.z, Block::Air);
            let (changed, overflow) = take_nav_changes(&world);
            assert!(
                !overflow && changed.contains(&p),
                "fluid removal invalidates"
            );
        }

        world.set_block_world(p.x, p.y, p.z, Block::Stone);
        assert_eq!(take_nav_changes(&world), (vec![p], false));

        world.set_block_world(p.x, p.y, p.z, Block::Air);
        let door = IVec3::new(8, 70, 9);
        assert!(world.place_door(door, Block::OakDoor, petramond_math::facing::Facing::South));
        let _ = take_nav_changes(&world);
        assert!(world.toggle_door(door).is_some());
        let (changed, overflow) = take_nav_changes(&world);
        assert!(
            !overflow && changed.contains(&door),
            "toggle invalidates: {changed:?}"
        );
    }

    #[test]
    fn isolated_leaf_decays() {
        let mut world = world_with_centered_chunk();
        let p = IVec3::new(8, 70, 8);
        world.set_block_world(p.x, p.y, p.z, Block::OakLeaves);
        tick_leaf(&mut world, p);
        assert_eq!(world.data.chunk_block(p.x, p.y, p.z), Block::Air.id());
    }

    #[test]
    fn leaf_next_to_log_survives() {
        let mut world = world_with_centered_chunk();
        let p = IVec3::new(8, 70, 8);
        world.set_block_world(p.x, p.y, p.z, Block::OakLeaves);
        world.set_block_world(p.x + 1, p.y, p.z, Block::OakLog);
        tick_leaf(&mut world, p);
        assert_eq!(world.data.chunk_block(p.x, p.y, p.z), Block::OakLeaves.id());
    }

    #[test]
    fn leaf_touching_only_leaves_decays() {
        let mut world = world_with_centered_chunk();
        let p = IVec3::new(8, 70, 8);
        world.set_block_world(p.x, p.y, p.z, Block::OakLeaves);
        world.set_block_world(p.x, p.y + 1, p.z, Block::OakLeaves);
        tick_leaf(&mut world, p);
        assert_eq!(world.data.chunk_block(p.x, p.y, p.z), Block::Air.id());
    }

    #[test]
    fn leaf_with_unloaded_neighbor_survives() {
        let mut world = world_with_centered_chunk();
        let p = IVec3::new(0, 70, 8);
        world.set_block_world(p.x, p.y, p.z, Block::OakLeaves);
        tick_leaf(&mut world, p);
        assert_eq!(world.data.chunk_block(p.x, p.y, p.z), Block::OakLeaves.id());
    }

    #[test]
    fn section_counter_gates_the_section() {
        let mut world = world_with_centered_chunk();
        let tickable = |w: &ServerWorld| {
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
        let mut world = world_with_centered_chunk();
        let p = IVec3::new(8, 70, 8);
        world.set_block_world(p.x, p.y, p.z, Block::OakLeaves);
        let recipes = Recipes::default();
        let mut decayed = false;
        for _ in 0..1_000_000 {
            world.game_tick(&recipes);
            if world.data.chunk_block(p.x, p.y, p.z) == Block::Air.id() {
                decayed = true;
                break;
            }
        }
        assert!(decayed, "isolated leaf was never random-ticked into decay");
    }

    #[test]
    fn random_ticks_reach_a_second_anchors_surroundings() {
        let mut world = ServerWorld::new(1, 4);
        world.insert_empty_column_for_test(ChunkPos::new(40, 0));
        world.data.last_load_target = Some(LoadTarget::new(0, 4, 0, 4));
        world.data.extra_load_targets = vec![LoadTarget::new(40, 4, 0, 4)];
        let p = IVec3::new(40 * 16 + 8, 70, 8);
        world.set_block_world(p.x, p.y, p.z, Block::OakLeaves);

        let recipes = Recipes::default();
        let mut decayed = false;
        for _ in 0..1_000_000 {
            world.game_tick(&recipes);
            if world.data.chunk_block(p.x, p.y, p.z) == Block::Air.id() {
                decayed = true;
                break;
            }
        }
        assert!(
            decayed,
            "an isolated leaf near the second anchor never random-ticked into decay"
        );
    }

    #[test]
    fn overlapping_anchor_discs_random_tick_each_column_once() {
        let rng_after = |targets: &[LoadTarget]| {
            let mut world = ServerWorld::new(1, 4);
            world.insert_empty_column_for_test(ChunkPos::new(0, 0));
            world.insert_empty_column_for_test(ChunkPos::new(40, 0));
            world.set_block_world(9, 70, 8, Block::OakLeaves);
            world.set_block_world(40 * 16 + 9, 70, 8, Block::OakLeaves);
            world.data.last_load_target = targets.first().copied();
            world.data.extra_load_targets = targets.iter().skip(1).copied().collect();
            world.random_tick_sections();
            world.data.sim.rng
        };
        let near = LoadTarget::new(0, 4, 0, 4);
        let far = LoadTarget::new(40, 4, 0, 4);
        assert_eq!(rng_after(&[near]), rng_after(&[near, near]));
        assert_eq!(
            rng_after(&[near, LoadTarget::new(1, 4, 0, 4)]),
            rng_after(&[near]),
            "a shifted anchor still covers the same single tickable column"
        );
        assert_ne!(rng_after(&[near]), rng_after(&[near, far]));
    }
}
