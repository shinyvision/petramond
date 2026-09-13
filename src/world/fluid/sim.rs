use crate::world::store::World;
use crate::world::WorldData;
use petramond_math::math::IVec3;
use petramond_world::block::Block;
use petramond_world::chunk::WORLD_MIN_Y;

use super::{
    amount, block_at, contact, fill_with_fluid, fillable, flowing, fluid_of, is_source, meta_at,
    opposite, CARDINALS, DOWN, FALLING, SLOPE_FIND_DIST, UP,
};
use petramond_world::fluid::FluidDef;

impl World {
    /// Whether the cell holds a STILL SOURCE of `fluid` (level 0, not falling) —
    /// the only fluid a bucket can scoop. Flowing/falling cells are an effect of
    /// their source, not a unit of fluid: they drain on their own once cut off.
    pub fn is_fluid_source_world(&self, pos: IVec3, fluid: Block) -> bool {
        block_at(self, pos) == fluid && is_source(meta_at(self, pos))
    }

    /// Set a fluid cell at world coords: write the cell, remesh its chunk (plus
    /// the neighbour across a shared border, whose culled faces change), and
    /// announce the change to neighbours. A fluid is itself transparent and a
    /// fluid cell's emission is seeded by the light gather (never by this write),
    /// but the announce still schedules the 3×3 relight — fluid can move INTO a
    /// cell that held a torch or other emitter, washing it away (see
    /// [`fill_with_fluid`]), so the block light there may have changed; the
    /// relight rides with the announce (see [`notify_block_and_neighbors`]).
    /// Returns false if the target chunk is not loaded.
    ///
    /// [`notify_block_and_neighbors`]: World::notify_block_and_neighbors
    pub(in crate::world) fn set_fluid_world(&mut self, pos: IVec3, block: Block, meta: u8) -> bool {
        debug_assert!(
            block == Block::Air || fluid_of(block).is_some(),
            "set_fluid_world writes fluid cells or dries them to air"
        );
        let Some((cpos, lx, ly, lz)) = WorldData::split_world(pos.x, pos.y, pos.z) else {
            return false;
        };
        // Streaming-finality guard: never mutate a section whose gen result or saved
        // overlay is still in flight (see `world::sim_guard`).
        if !self.stream_writable(cpos) {
            return false;
        }
        if !self.sections.contains_key(&cpos) {
            // Fluid spilling into open air below/around a cliff materializes the
            // section it flows into; a dry-up (setting air) into nothing is a no-op.
            if block == Block::Air || !self.materialize_section(cpos) {
                return false;
            }
            // The flow chose this cell reading the ABSENT section as air; the
            // materialized base is authoritative and may hold terrain (or its own
            // generated fluid) there. Only genuinely open cells accept the flow.
            if block != Block::Air && self.chunk_block(pos.x, pos.y, pos.z) != Block::Air.id() {
                return false;
            }
        }
        {
            let Some(s) = self.section_mut(cpos) else {
                return false;
            };
            s.set_fluid(lx, ly, lz, block, meta);
            s.modified = true;
        }
        self.refresh_particle_emitter_index(cpos);
        if let Some(change) = self.update_column_heights_after_set(pos.x, pos.y, pos.z, block) {
            self.mark_sky_cover_edited_at(pos.x, pos.z, change);
        }
        // A border cell changes neighbour sections' culled faces: re-mesh
        // every section whose pad samples this cell.
        self.queue_dirty_meshes_sampling_cell(pos.x, pos.y, pos.z);
        self.notify_block_and_neighbors(pos.x, pos.y, pos.z);
        true
    }
}

/// The flowing-fluid simulation: re-levelling, source conversion, and the
/// down-then-sideways spread with its slope search, factored out of `World`.
/// Every fluid runs the SAME machine on its own descriptor. Downward quench
/// contact reacts when the flow enters the receiving cell.
///
/// `FluidSim` is **stateless with respect to `World`**: it holds no borrow of a
/// world and no per-cell scratch that must outlive a call. Each method takes the
/// `&World`/`&mut World` it operates on as a parameter, so the tick driver can
/// construct a `FluidSim` at the call site and hand it the world (sequential
/// reborrows) without ever storing a `&mut World` — see [`crate::world::tick`].
/// Reads use [`block_at`]/[`meta_at`]; fluid and contact writes use the
/// world's ordinary mutation paths.
pub(super) struct FluidSim;

impl FluidSim {
    /// The fluid flow update for the cell at `pos` (a scheduled tick).
    pub(super) fn flow_check(&self, world: &mut World, pos: IVec3) {
        let Some(fluid) = fluid_of(block_at(world, pos)) else {
            return; // no longer fluid
        };
        // Earlier scheduled flow can introduce contact before the neighbor
        // update batch runs. A quenched cell must not pour into another cell.
        contact::react(world, pos, fluid);
        if block_at(world, pos) != fluid.block {
            return;
        }
        let mut meta = meta_at(world, pos);

        // Re-level everything that is not a source. Writing the new state
        // notifies neighbours, whose own checks carry a change onward — this is
        // both how a flow front strengthens and how a cut-off sheet recedes.
        if !is_source(meta) {
            match self.recompute(world, pos, fluid) {
                None => {
                    world.set_fluid_world(pos, Block::Air, 0);
                    return; // nothing left to spread
                }
                Some(new_meta) if new_meta != meta => {
                    world.set_fluid_world(pos, fluid.block, new_meta);
                    meta = new_meta;
                }
                _ => {}
            }
        }

        self.spread(world, pos, meta, fluid);
    }

    /// What this cell's fluid should be, judged purely from its neighbours —
    /// `None` when nothing feeds it and it should dry up. The single
    /// re-evaluation rule (in priority order):
    ///   1. for a renewable fluid, two or more SOURCE neighbours over a solid
    ///      floor or over another source: the cell becomes a source itself.
    ///      Falling cells do not count toward the two (only true sources do),
    ///      and a cell perched over air or moving fluid never converts, which
    ///      is what keeps a flooding cave from turning to sources everywhere.
    ///   2. any fluid directly above: a full falling cell, whatever the sides
    ///      say.
    ///   3. otherwise: the strongest horizontal neighbour's amount minus one
    ///      drop-off step — dead when that reaches zero. Every chain of
    ///      flowing fluid therefore leans on a real source or falling column;
    ///      there is no state in which flow sustains itself.
    fn recompute(&self, world: &World, pos: IVec3, fluid: &'static FluidDef) -> Option<u8> {
        let fluid_block = fluid.block;
        let mut max_amount = 0u8;
        let mut sources = 0;
        for d in CARDINALS {
            let np = pos + d;
            if block_at(world, np) != fluid_block {
                continue;
            }
            let nm = meta_at(world, np);
            if is_source(nm) {
                sources += 1;
            }
            max_amount = max_amount.max(amount(nm));
        }

        if fluid.renewable && sources >= 2 {
            let below = pos + DOWN;
            let below_block = block_at(world, below);
            let solid_below = below_block != fluid_block && !fillable(below_block);
            let source_below = below_block == fluid_block && is_source(meta_at(world, below));
            if solid_below || source_below {
                return Some(0);
            }
        }

        if block_at(world, pos + UP) == fluid_block {
            return Some(FALLING);
        }

        let amt = max_amount.saturating_sub(fluid.drop_off);
        if amt == 0 {
            None
        } else {
            Some(flowing(8 - amt))
        }
    }

    /// Move this cell's fluid outward: down first, sideways when down is closed.
    fn spread(&self, world: &mut World, pos: IVec3, meta: u8, fluid: &'static FluidDef) {
        let below = pos + DOWN;
        match contact::react_to_downward_flow(world, below, fluid) {
            contact::DownwardContact::None => {}
            contact::DownwardContact::Reacted => return,
            // Treating the quencher as a floor would fan the flow out sideways.
            contact::DownwardContact::Refused => {
                world.schedule_block_tick(pos, crate::world::sim_guard::SIM_RETRY_DELAY);
                return;
            }
        }
        let below_block = block_at(world, below);
        if below.y >= WORLD_MIN_Y && fillable(below_block) {
            // Pour down. The poured cell is judged by the same re-evaluation
            // rule — with this cell above it that is a full falling cell,
            // unless the infinite-pool rule fires down there instead.
            let poured = self.recompute(world, below, fluid).unwrap_or(FALLING);
            fill_with_fluid(world, below, fluid.block, poured);
            // A cell flanked by three or more sources keeps feeding sideways
            // even while pouring down — the interior edge of a big pool.
            if self.source_neighbor_count(world, pos, fluid.block) >= 3 {
                self.spread_to_sides(world, pos, meta, fluid);
            }
        } else if below_block != fluid.block
            || (is_source(meta) && is_source(meta_at(world, below)))
        {
            // Down is closed. Spread sideways from solid ground — or, for a
            // source resting on a STILL body of its fluid (a source), across
            // that surface: the bucket poured onto a pond sheets over it. A
            // source over its own FALLING column is the head of a pour, not a
            // surface — it feeds the column and the fan-out belongs where the
            // column lands; letting it creep here turned every fall into a
            // cross of falls. A FLOWING cell over fluid is a column joining
            // the body under it and must not creep either: that would let
            // flow climb over itself and advance where no source pushes it.
            self.spread_to_sides(world, pos, meta, fluid);
        }
    }

    /// Spread one ring sideways: pick the direction(s) via the slope search and
    /// fill each — but never a cell that already holds any fluid
    /// (existing fluid re-levels itself; overwriting it would double-move the
    /// flow in one tick).
    fn spread_to_sides(&self, world: &mut World, pos: IVec3, meta: u8, fluid: &'static FluidDef) {
        // What the next ring would hold: a source or landing falling cell
        // carries the full amount, so it spreads at the fluid's own outflow;
        // a flowing cell at the last level pushes nothing further.
        if amount(meta).saturating_sub(fluid.drop_off) == 0 {
            return;
        }
        let (dirs, count) = self.spread_directions(world, pos, fluid);
        for &(d, new_meta) in &dirs[..count] {
            let np = pos + d;
            if block_at(world, np) != fluid.block {
                fill_with_fluid(world, np, fluid.block, new_meta);
            }
        }
    }

    /// The sideways-spread candidates: every passable direction, filtered to the
    /// one(s) whose path reaches a drop soonest. Distance 0 means the adjacent
    /// cell itself sits over a drop; with no drop within [`SLOPE_FIND_DIST`]
    /// steps past the first ring every passable direction ties and all spread.
    /// Each kept direction carries the metadata its cell would re-evaluate to
    /// (computed before any of this ring is written), so merging flows land at
    /// their final level immediately.
    fn spread_directions(
        &self,
        world: &World,
        pos: IVec3,
        fluid: &'static FluidDef,
    ) -> ([(IVec3, u8); 4], usize) {
        let mut best = i32::MAX;
        let mut out = [(IVec3::new(0, 0, 0), 0u8); 4];
        let mut count = 0;
        for d in CARDINALS {
            let np = pos + d;
            if !self.passable(world, np, fluid.block) {
                continue;
            }
            let Some(new_meta) = self.recompute(world, np, fluid) else {
                continue;
            };
            let dist = if self.drop_below(world, np, fluid) {
                0
            } else {
                self.slope_distance(world, np, 1, opposite(d), fluid)
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

    /// Shortest path length (over passable cells at this Y, no immediate
    /// backtracking) from `pos` to a cell with a drop below it, or `i32::MAX`
    /// when none is within [`SLOPE_FIND_DIST`] steps. A bounded depth-first
    /// walk, NOT a shared-frontier flood: each spread direction measures its
    /// own distance independently, so two directions whose paths overlap still
    /// both count the drop and tie.
    fn slope_distance(
        &self,
        world: &World,
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
            if !self.passable(world, np, fluid.block) {
                continue;
            }
            if self.drop_below(world, np, fluid) {
                return depth;
            }
            if depth < SLOPE_FIND_DIST {
                best = best.min(self.slope_distance(world, np, depth + 1, opposite(d), fluid));
            }
        }
        best
    }

    /// Can sideways flow travel through/into this cell? Open space (air or a
    /// washable fragile block) or this fluid that is not a source. Source cells
    /// wall the search off: flow neither crosses nor competes with a full pool
    /// cell.
    fn passable(&self, world: &World, pos: IVec3, fluid: Block) -> bool {
        let b = block_at(world, pos);
        fillable(b) || (b == fluid && !is_source(meta_at(world, pos)))
    }

    /// Is the cell below `pos` somewhere this fluid could go — open space to
    /// fall into, existing fluid of the same kind to merge with, or this fluid's
    /// quencher, which the downward pour enters and solidifies? The world floor
    /// is solid ground, not a drop.
    fn drop_below(&self, world: &World, pos: IVec3, fluid: &'static FluidDef) -> bool {
        let below = pos + DOWN;
        if below.y < WORLD_MIN_Y {
            return false;
        }
        let b = block_at(world, below);
        b == fluid.block || fillable(b) || fluid.quench.is_some_and(|q| b == q.by)
    }

    /// How many of the four horizontal neighbours are still SOURCES of this fluid.
    fn source_neighbor_count(&self, world: &World, pos: IVec3, fluid: Block) -> usize {
        CARDINALS
            .iter()
            .filter(|&&d| {
                let np = pos + d;
                block_at(world, np) == fluid && is_source(meta_at(world, np))
            })
            .count()
    }
}
