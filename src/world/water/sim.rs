use crate::block::Block;
use crate::chunk::WORLD_MIN_Y;
use crate::mathh::{IVec3, Vec3};
use crate::world::store::World;

use super::{
    amount, block_at, fill_with_water, fillable, flowing, fluid_height, is_falling, is_source,
    meta_at, opposite, surface_flow_dir, CARDINALS, DOWN, DROP_OFF, FALLING, SLOPE_FIND_DIST, UP,
};

impl World {
    /// Whether the cell holds a STILL SOURCE of water (level 0, not falling) —
    /// the only water a bucket can scoop. Flowing/falling water is an effect of
    /// its source, not a unit of water: it drains on its own once cut off.
    pub fn is_water_source_world(&self, pos: IVec3) -> bool {
        block_at(self, pos) == Block::Water && is_source(meta_at(self, pos))
    }

    /// Horizontal direction entities should drift in when they overlap this water
    /// cell. This intentionally matches [`surface_flow_dir`], which is also what
    /// the mesher uses to face the flowing-water texture.
    pub fn water_flow_dir_at(&self, wx: i32, wy: i32, wz: i32) -> Vec3 {
        let block_at = |x: i32, y: i32, z: i32| block_at(self, IVec3::new(x, y, z));
        let fluid_at = |x: i32, y: i32, z: i32| -> Option<f32> {
            let p = IVec3::new(x, y, z);
            if block_at(p.x, p.y, p.z) != Block::Water {
                return None;
            }
            Some(fluid_height(meta_at(self, p), block_at(p.x, p.y + 1, p.z)))
        };
        let still_at = |x: i32, y: i32, z: i32| {
            block_at(x, y, z) == Block::Water && is_source(meta_at(self, IVec3::new(x, y, z)))
        };
        surface_flow_dir(wx, wy, wz, &block_at, &fluid_at, &still_at)
    }

    /// Absolute Y of the water surface over `cell`, when `cell` holds water:
    /// the topmost contiguous water cell of the column plus its rendered
    /// surface height (a source top sits at 8/9). `None` for a non-water
    /// cell. The read model behind surface-floating bodies (mob `buoyancy:
    /// "surface"`) — the walk up the column is bounded by water depth and a
    /// floating body starts it at the surface anyway.
    pub fn water_surface_y_world(&self, cell: IVec3) -> Option<f32> {
        if block_at(self, cell) != Block::Water {
            return None;
        }
        let mut top = cell;
        while block_at(self, IVec3::new(top.x, top.y + 1, top.z)) == Block::Water {
            top.y += 1;
        }
        let above = block_at(self, IVec3::new(top.x, top.y + 1, top.z));
        Some(top.y as f32 + fluid_height(meta_at(self, top), above))
    }

    /// The current acting on a body PROBE POINT: the cell's flow direction only
    /// while the point sits below the fluid's actual surface. An uncapped cell
    /// tops out at 8/9, so a probe skimming the cell's top sliver — feet
    /// standing on a 15/16 block beside an irrigation channel — catches no
    /// current; capped/falling cells fill the whole cell and push everywhere
    /// in it. This is the sampler every body integrator (player, mob, dropped
    /// item) drifts by.
    pub fn water_flow_at_point(&self, p: Vec3) -> Vec3 {
        let c = IVec3::new(p.x.floor() as i32, p.y.floor() as i32, p.z.floor() as i32);
        if block_at(self, c) != Block::Water {
            return Vec3::ZERO;
        }
        let above = block_at(self, IVec3::new(c.x, c.y + 1, c.z));
        if p.y - c.y as f32 >= fluid_height(meta_at(self, c), above) {
            return Vec3::ZERO;
        }
        self.water_flow_dir_at(c.x, c.y, c.z)
    }

    /// Set a water cell at world coords: write the cell, remesh its chunk (plus
    /// the neighbour across a shared border, whose culled faces change), and
    /// announce the change to neighbours. Water is itself transparent and emits
    /// nothing, but the announce still schedules the 3×3 relight — water can move
    /// INTO a cell that held a torch or other emitter, washing it away (see
    /// [`fill_with_water`]), so the block light there may have changed; the relight
    /// rides with the announce (see [`notify_block_and_neighbors`]). Returns false
    /// if the target chunk is not loaded.
    ///
    /// [`notify_block_and_neighbors`]: World::notify_block_and_neighbors
    pub(in crate::world) fn set_water_world(&mut self, pos: IVec3, block: Block, meta: u8) -> bool {
        let Some((cpos, lx, ly, lz)) = Self::split_world(pos.x, pos.y, pos.z) else {
            return false;
        };
        // Streaming-finality guard: never mutate a section whose gen result or saved
        // overlay is still in flight (see `world::sim_guard`).
        if !self.stream_writable(cpos) {
            return false;
        }
        if !self.sections.contains_key(&cpos) {
            // Water spilling into open air below/around a cliff materializes the section
            // it flows into; a dry-up (setting air) into nothing is a no-op.
            if block == Block::Air || !self.materialize_section(cpos) {
                return false;
            }
            // The flow chose this cell reading the ABSENT section as air; the
            // materialized base is authoritative and may hold terrain (or its own
            // generated water) there. Only genuinely open cells accept the flow.
            if block != Block::Air && self.chunk_block(pos.x, pos.y, pos.z) != Block::Air.id() {
                return false;
            }
        }
        {
            let Some(s) = self.section_mut(cpos) else {
                return false;
            };
            s.set_water(lx, ly, lz, block, meta);
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

/// The flowing-water simulation: re-levelling, source conversion, and the
/// down-then-sideways spread with its slope search, factored out of `World`.
///
/// `FluidSim` is **stateless with respect to `World`**: it holds no borrow of a
/// world and no per-cell scratch that must outlive a call. Each method takes the
/// `&World`/`&mut World` it operates on as a parameter, so the tick driver can
/// construct a `FluidSim` at the call site and hand it the world (sequential
/// reborrows) without ever storing a `&mut World` — see [`crate::world::tick`]. The
/// world is touched only through three accessors: the module-private
/// [`block_at`]/[`meta_at`] reads and [`World::set_water_world`] writes.
pub(super) struct FluidSim;

impl FluidSim {
    /// The water flow update for the cell at `pos` (a scheduled block tick).
    pub(super) fn flow_check(&self, world: &mut World, pos: IVec3) {
        if block_at(world, pos) != Block::Water {
            return; // no longer water
        }
        let mut meta = meta_at(world, pos);

        // Re-level everything that is not a source. Writing the new state
        // notifies neighbours, whose own checks carry a change onward — this is
        // both how a flow front strengthens and how a cut-off sheet recedes.
        if !is_source(meta) {
            match self.recompute(world, pos) {
                None => {
                    world.set_water_world(pos, Block::Air, 0);
                    return; // nothing left to spread
                }
                Some(new_meta) if new_meta != meta => {
                    world.set_water_world(pos, Block::Water, new_meta);
                    meta = new_meta;
                }
                _ => {}
            }
        }

        self.spread(world, pos, meta);
    }

    /// What this cell's water should be, judged purely from its neighbours —
    /// `None` when nothing feeds it and it should dry up. The single
    /// re-evaluation rule (in priority order):
    ///   1. two or more SOURCE neighbours over a solid floor or over another
    ///      source: the cell becomes a source itself — the infinite-pool rule.
    ///      Falling water does not count toward the two (only true sources do),
    ///      and a cell perched over air or moving water never converts, which is
    ///      what keeps a flooding cave from turning to sources everywhere.
    ///   2. any water directly above: a full falling cell, whatever the sides say.
    ///   3. otherwise: the strongest horizontal neighbour's amount minus
    ///      [`DROP_OFF`] — dead when that reaches zero. Every chain of flowing
    ///      water therefore leans on a real source or falling column; there is
    ///      no state in which flow sustains itself.
    fn recompute(&self, world: &World, pos: IVec3) -> Option<u8> {
        let mut max_amount = 0u8;
        let mut sources = 0;
        for d in CARDINALS {
            let np = pos + d;
            if block_at(world, np) != Block::Water {
                continue;
            }
            let nm = meta_at(world, np);
            if is_source(nm) {
                sources += 1;
            }
            max_amount = max_amount.max(amount(nm));
        }

        if sources >= 2 {
            let below = pos + DOWN;
            let below_block = block_at(world, below);
            let solid_below = below_block != Block::Water && !fillable(below_block);
            let source_below = below_block == Block::Water && is_source(meta_at(world, below));
            if solid_below || source_below {
                return Some(0);
            }
        }

        if block_at(world, pos + UP) == Block::Water {
            return Some(FALLING);
        }

        let amt = max_amount.saturating_sub(DROP_OFF);
        if amt == 0 {
            None
        } else {
            Some(flowing(8 - amt))
        }
    }

    /// Move this cell's water outward: down first, sideways when down is closed.
    fn spread(&self, world: &mut World, pos: IVec3, meta: u8) {
        let below = pos + DOWN;
        let below_block = block_at(world, below);
        if below.y >= WORLD_MIN_Y && fillable(below_block) {
            // Pour down. The poured cell is judged by the same re-evaluation
            // rule — with this cell above it that is a full falling cell,
            // unless the infinite-pool rule fires down there instead.
            let poured = self.recompute(world, below).unwrap_or(FALLING);
            fill_with_water(world, below, poured);
            // A cell flanked by three or more sources keeps feeding sideways
            // even while pouring down — the interior edge of a big pool.
            if self.source_neighbor_count(world, pos) >= 3 {
                self.spread_to_sides(world, pos, meta);
            }
        } else if is_source(meta) || below_block != Block::Water {
            // Down is closed. Spread sideways from solid ground — or, for a
            // source only, across the top of the water below it. A FLOWING cell
            // over water is a column joining the body under it, not a surface,
            // and must not creep sideways: that would let flow climb over
            // itself and advance where no source pushes it.
            self.spread_to_sides(world, pos, meta);
        }
    }

    /// Spread one ring sideways: pick the direction(s) via the slope search and
    /// fill each — but never a cell that already holds water (existing water
    /// re-levels itself; overwriting it would double-move the flow in one tick).
    fn spread_to_sides(&self, world: &mut World, pos: IVec3, meta: u8) {
        // A landing falling cell carries full strength — it spreads like a
        // source's outflow. A flowing cell carries its amount minus the
        // per-block loss, and stops spreading entirely at the last level.
        let carried = if is_falling(meta) {
            7
        } else {
            amount(meta) - DROP_OFF
        };
        if carried == 0 {
            return;
        }
        let (dirs, count) = self.spread_directions(world, pos);
        for &(d, new_meta) in &dirs[..count] {
            let np = pos + d;
            if block_at(world, np) != Block::Water {
                fill_with_water(world, np, new_meta);
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
    fn spread_directions(&self, world: &World, pos: IVec3) -> ([(IVec3, u8); 4], usize) {
        let mut best = i32::MAX;
        let mut out = [(IVec3::new(0, 0, 0), 0u8); 4];
        let mut count = 0;
        for d in CARDINALS {
            let np = pos + d;
            if !self.passable(world, np) {
                continue;
            }
            let Some(new_meta) = self.recompute(world, np) else {
                continue;
            };
            let dist = if self.drop_below(world, np) {
                0
            } else {
                self.slope_distance(world, np, 1, opposite(d))
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
    fn slope_distance(&self, world: &World, pos: IVec3, depth: i32, came_from: IVec3) -> i32 {
        let mut best = i32::MAX;
        for d in CARDINALS {
            if d == came_from {
                continue;
            }
            let np = pos + d;
            if !self.passable(world, np) {
                continue;
            }
            if self.drop_below(world, np) {
                return depth;
            }
            if depth < SLOPE_FIND_DIST {
                best = best.min(self.slope_distance(world, np, depth + 1, opposite(d)));
            }
        }
        best
    }

    /// Can sideways flow travel through/into this cell? Open space (air or a
    /// washable fragile block) or water that is not a source. Source cells wall
    /// the search off: flow neither crosses nor competes with a full pool cell.
    fn passable(&self, world: &World, pos: IVec3) -> bool {
        let b = block_at(world, pos);
        fillable(b) || (b == Block::Water && !is_source(meta_at(world, pos)))
    }

    /// Is the cell below `pos` somewhere water could go — open space to fall
    /// into, or existing water to merge with? The slope search steers toward
    /// these. The world floor is treated as solid ground, not a drop.
    fn drop_below(&self, world: &World, pos: IVec3) -> bool {
        let below = pos + DOWN;
        if below.y < WORLD_MIN_Y {
            return false;
        }
        let b = block_at(world, below);
        b == Block::Water || fillable(b)
    }

    /// How many of the four horizontal neighbours are still SOURCES.
    fn source_neighbor_count(&self, world: &World, pos: IVec3) -> usize {
        CARDINALS
            .iter()
            .filter(|&&d| {
                let np = pos + d;
                block_at(world, np) == Block::Water && is_source(meta_at(world, np))
            })
            .count()
    }
}
