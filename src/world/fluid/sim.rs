use crate::world::WorldData;
use crate::world::{ServerWorld, World, WorldSide};
use petramond_math::math::IVec3;
use petramond_world::block::Block;
use petramond_world::chunk::WORLD_MIN_Y;

use super::{
    amount, block_at, contact, fill_with_fluid, fillable, flowing, fluid_of, is_source, opposite,
    FluidAnnounce, FluidReads, CARDINALS, DOWN, FALLING, SLOPE_FIND_DIST, UP,
};
use petramond_world::fluid::FluidDef;

impl<S: WorldSide> World<S> {
    #[cfg(test)]
    pub(in crate::world) fn set_fluid_world(&mut self, pos: IVec3, block: Block, meta: u8) -> bool {
        let mut announce = FluidAnnounce::default();
        let written = self.write_fluid_cell(pos, block, meta, &mut announce);
        announce.flush(self);
        written
    }

    pub(in crate::world) fn write_fluid_cell(
        &mut self,
        pos: IVec3,
        block: Block,
        meta: u8,
        announce: &mut FluidAnnounce,
    ) -> bool {
        debug_assert!(
            block == Block::Air || fluid_of(block).is_some(),
            "write_fluid_cell writes fluid cells or dries them to air"
        );
        let Some((cpos, lx, ly, lz)) = WorldData::split_world(pos.x, pos.y, pos.z) else {
            return false;
        };
        if !self.data.stream_writable(cpos) {
            return false;
        }
        if !self.data.sections.contains_key(&cpos) {
            if block == Block::Air || !self.materialize_section(cpos) {
                return false;
            }
            if block != Block::Air && self.data.chunk_block(pos.x, pos.y, pos.z) != Block::Air.id()
            {
                return false;
            }
        }
        {
            let Some(s) = self.data.section_mut(cpos) else {
                return false;
            };
            s.set_fluid(lx, ly, lz, block, meta);
            s.modified = true;
        }
        if let Some(change) = self.update_column_heights_after_set(pos.x, pos.y, pos.z, block) {
            announce.note_sky_cover(pos.x, pos.z, change);
        }
        if announce.note_write(pos, cpos) {
            self.queue_cell_and_neighbor_updates(pos);
        }
        true
    }
}

/// The fluid sim, pulled out of `World`: re-levelling, source conversion, and spreading down then
/// sideways with a slope search. Same code for every fluid, just a different descriptor.
///
/// It holds no world borrow or scratch. Methods take the `ServerWorld` they work on, so the tick
/// driver makes one at the call site and never stores a `&mut ServerWorld` (see
/// [`crate::world::tick`]).
///
/// Reads use one [`FluidReads`] cursor; fluid writes go through [`World::write_fluid_cell`] into
/// the caller's [`FluidAnnounce`] batch.
pub(super) struct FluidSim;

impl FluidSim {
    pub(super) fn flow_check(
        &self,
        world: &mut ServerWorld,
        pos: IVec3,
        announce: &mut FluidAnnounce,
    ) {
        let Some(fluid) = fluid_of(block_at(world, pos)) else {
            return;
        };
        contact::react(world, pos, fluid);
        let (current, mut meta, relevel) = {
            let reads = FluidReads::new(world);
            let current = reads.block(pos);
            let meta = reads.meta(pos);
            let relevel = (current == fluid.block && !is_source(meta))
                .then(|| self.recompute(&reads, pos, fluid));
            (current, meta, relevel)
        };
        if current != fluid.block {
            return;
        }
        match relevel {
            Some(None) => {
                world.write_fluid_cell(pos, Block::Air, 0, announce);
                return;
            }
            Some(Some(new_meta)) if new_meta != meta => {
                world.write_fluid_cell(pos, fluid.block, new_meta, announce);
                meta = new_meta;
            }
            _ => {}
        }

        self.spread(world, pos, meta, fluid, announce);
    }

    /// What this cell's fluid should be, going only by its neighbours. `None` means it dries up.
    ///
    /// In priority order:
    /// 1. Renewable fluid with 2+ source neighbours, over solid floor or another source: it becomes
    ///    a source. Falling cells don't count toward the two. Perched over air or moving fluid it
    ///    never converts, or a flooding cave would turn to sources everywhere.
    /// 2. Any fluid directly above: a full falling cell, whatever the sides say.
    /// 3. Otherwise the strongest horizontal neighbour's amount minus one step, dead at zero. So
    ///    flow always leans on a real source or falling column and can't sustain itself.
    fn recompute(
        &self,
        reads: &FluidReads<'_>,
        pos: IVec3,
        fluid: &'static FluidDef,
    ) -> Option<u8> {
        let fluid_block = fluid.block;
        let mut max_amount = 0u8;
        let mut sources = 0;
        for d in CARDINALS {
            let np = pos + d;
            if reads.block(np) != fluid_block {
                continue;
            }
            let nm = reads.meta(np);
            if is_source(nm) {
                sources += 1;
            }
            max_amount = max_amount.max(amount(nm));
        }

        if fluid.renewable && sources >= 2 {
            let below = pos + DOWN;
            let below_block = reads.block(below);
            let solid_below = below_block != fluid_block && !fillable(below_block);
            let source_below = below_block == fluid_block && is_source(reads.meta(below));
            if solid_below || source_below {
                return Some(0);
            }
        }

        if reads.block(pos + UP) == fluid_block {
            return Some(FALLING);
        }

        let amt = max_amount.saturating_sub(fluid.drop_off);
        if amt == 0 {
            None
        } else {
            Some(flowing(8 - amt))
        }
    }

    fn spread(
        &self,
        world: &mut ServerWorld,
        pos: IVec3,
        meta: u8,
        fluid: &'static FluidDef,
        announce: &mut FluidAnnounce,
    ) {
        let below = pos + DOWN;
        match contact::react_to_downward_flow(world, below, fluid) {
            contact::DownwardContact::None => {}
            contact::DownwardContact::Reacted => return,
            contact::DownwardContact::Refused => {
                world.schedule_fluid_tick(pos, crate::world::sim_guard::SIM_RETRY_DELAY);
                return;
            }
        }
        let (pour, spread_sideways) = {
            let reads = FluidReads::new(world);
            let below_block = reads.block(below);
            if below.y >= WORLD_MIN_Y && fillable(below_block) {
                let poured = self.recompute(&reads, below, fluid).unwrap_or(FALLING);
                (
                    Some(poured),
                    self.source_neighbor_count(&reads, pos, fluid.block) >= 3,
                )
            } else {
                // Down is closed. Spread sideways from solid ground — or, for a
                // source resting on a STILL body of its fluid (a source), across
                // that surface: the bucket poured onto a pond sheets over it. A
                // source over its own FALLING column is the head of a pour, not a
                // surface — it feeds the column and the fan-out belongs where the
                // column lands; letting it creep here turned every fall into a
                // cross of falls. A FLOWING cell over fluid is a column joining
                // the body under it and must not creep either: that would let
                // flow climb over itself and advance where no source pushes it.
                let sideways =
                    below_block != fluid.block || (is_source(meta) && is_source(reads.meta(below)));
                (None, sideways)
            }
        };
        if let Some(poured) = pour {
            fill_with_fluid(world, below, fluid.block, poured, announce);
        }
        if spread_sideways {
            self.spread_to_sides(world, pos, meta, fluid, announce);
        }
    }

    fn spread_to_sides(
        &self,
        world: &mut ServerWorld,
        pos: IVec3,
        meta: u8,
        fluid: &'static FluidDef,
        announce: &mut FluidAnnounce,
    ) {
        if amount(meta).saturating_sub(fluid.drop_off) == 0 {
            return;
        }
        let (dirs, count) = self.spread_directions(&FluidReads::new(world), pos, fluid);
        for &(d, new_meta) in &dirs[..count] {
            let np = pos + d;
            if block_at(world, np) != fluid.block {
                fill_with_fluid(world, np, fluid.block, new_meta, announce);
            }
        }
    }

    fn spread_directions(
        &self,
        reads: &FluidReads<'_>,
        pos: IVec3,
        fluid: &'static FluidDef,
    ) -> ([(IVec3, u8); 4], usize) {
        let mut best = i32::MAX;
        let mut out = [(IVec3::new(0, 0, 0), 0u8); 4];
        let mut count = 0;
        for d in CARDINALS {
            let np = pos + d;
            if !self.passable(reads, np, fluid.block) {
                continue;
            }
            let Some(new_meta) = self.recompute(reads, np, fluid) else {
                continue;
            };
            let dist = if self.drop_below(reads, np, fluid) {
                0
            } else {
                self.slope_distance(reads, np, 1, opposite(d), fluid)
            };
            if dist < best {
                count = 0;
            }
            if dist <= best {
                out[count] = (d, new_meta);
                count += 1;
                best = dist;
            }
        }
        (out, count)
    }

    fn slope_distance(
        &self,
        reads: &FluidReads<'_>,
        pos: IVec3,
        depth: i32,
        came_from: IVec3,
        fluid: &'static FluidDef,
    ) -> i32 {
        let mut best = i32::MAX;
        for d in CARDINALS {
            if d == came_from {
                continue;
            }
            let np = pos + d;
            if !self.passable(reads, np, fluid.block) {
                continue;
            }
            if self.drop_below(reads, np, fluid) {
                return depth;
            }
            if depth < SLOPE_FIND_DIST {
                best = best.min(self.slope_distance(reads, np, depth + 1, opposite(d), fluid));
            }
        }
        best
    }

    fn passable(&self, reads: &FluidReads<'_>, pos: IVec3, fluid: Block) -> bool {
        let b = reads.block(pos);
        fillable(b) || (b == fluid && !is_source(reads.meta(pos)))
    }

    fn drop_below(&self, reads: &FluidReads<'_>, pos: IVec3, fluid: &'static FluidDef) -> bool {
        let below = pos + DOWN;
        if below.y < WORLD_MIN_Y {
            return false;
        }
        let b = reads.block(below);
        b == fluid.block || fillable(b) || fluid.quench.is_some_and(|q| b == q.by)
    }

    fn source_neighbor_count(&self, reads: &FluidReads<'_>, pos: IVec3, fluid: Block) -> usize {
        CARDINALS
            .iter()
            .filter(|&&d| {
                let np = pos + d;
                reads.block(np) == fluid && is_source(reads.meta(np))
            })
            .count()
    }
}
