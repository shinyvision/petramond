//! Flowing-water simulation.
//!
//! State lives in one metadata byte per water cell (see `Chunk::water_meta`):
//!   - bits 0..4 `level`: 0 = a still SOURCE; 1..=7 = flowing water, one level
//!     lost per horizontal block travelled, so a sheet on flat ground reaches 7
//!     cells past its source. A cell's *amount* is `8 - level` (sources and
//!     falling cells hold the full 8) and its rendered height is `amount / 9`.
//!   - bit 7 [`FALLING`]: a vertical stream cell (full amount, renders full).
//!
//! Worldgen oceans/rivers are written as plain `Block::Water` with meta 0 —
//! sources that sit still until disturbed. A neighbouring block change queues a
//! block update, which schedules the cell's flow check [`WATER_FLOW_DELAY`]
//! ticks out. The check, [`FluidSim::flow_check`], does two things:
//!
//!   1. **Re-level** (flowing/falling cells only; a source is never
//!      re-evaluated, only a bucket or block edit removes one): recompute the
//!      cell from its neighbours — two or more SOURCE neighbours over a solid
//!      floor or over another source convert it into a source itself (the
//!      infinite-pool rule); any water directly above forces a full falling
//!      cell; otherwise it takes the strongest horizontal neighbour's amount
//!      minus one, drying up when nothing feeds it. This one rule is also the
//!      decay path: cut the source and every cell re-levels downward, ring by
//!      ring, until the sheet is gone.
//!   2. **Spread**: pour into the cell below when it can accept water (also
//!      spreading sideways then only when flanked by 3+ sources — the interior
//!      of a pool keeps feeding outward while its edge pours down). When it
//!      cannot pour — blocked by solid ground, or by water it has merged with —
//!      it spreads sideways if it is a source or rests on that solid ground; a
//!      flowing cell suspended over other water is part of a column, not a
//!      surface, and never spreads. Sideways flow prefers the direction(s)
//!      whose open path reaches a drop soonest (a bounded slope search,
//!      [`SLOPE_FIND_DIST`] steps past the first ring); with no drop in range
//!      it spreads every open way. A falling cell landing on solid ground
//!      spreads a full-strength ring, like a source's outflow.
//!
//! Spread never overwrites a cell that already holds water: existing water
//! re-levels itself on its own scheduled check. Every write announces itself to
//! its neighbours, so a sheet advances one ring per flow delay and naturally
//! crosses chunk borders.

use crate::block::{Block, BlockBehavior};
use crate::chunk::WORLD_MIN_Y;
use crate::mathh::{IVec3, Vec3};

use super::store::World;

/// Ticks between a water cell being disturbed and its flow check running.
pub(super) const WATER_FLOW_DELAY: u64 = 5;

/// Water's block behaviour. Both hooks delegate to the [`FluidSim`] below, so this
/// is just the wiring that puts water on the generic reaction path. It lives here
/// in `world` (not in `block`) because it drives the world tick scheduler and
/// `FluidSim` — world internals a `block`-side behaviour can't reach — while still
/// implementing the `block`-defined [`BlockBehavior`].
pub struct Water;

impl BlockBehavior for Water {
    fn key(&self) -> &'static str {
        "water"
    }

    fn neighbor_update(&self, world: &mut World, pos: IVec3) {
        // A neighbour changed: schedule the flow check `WATER_FLOW_DELAY` ticks out
        // so the disturbance settles before water re-levels.
        world.schedule_block_tick(pos, WATER_FLOW_DELAY);
    }

    fn scheduled_tick(&self, world: &mut World, pos: IVec3) {
        FluidSim.flow_check(world, pos);
    }
}

/// The water singleton a row points at (`behavior: &behavior::WATER`).
pub static WATER: Water = Water;

/// Amount lost per horizontal block travelled: a source (amount 8) feeds its
/// neighbours at 7, and the flow dies past amount 1 — seven cells out.
const DROP_OFF: u8 = 1;

/// Slope-search depth past the first ring: a drop up to `1 + SLOPE_FIND_DIST`
/// cells away steers the flow toward it.
const SLOPE_FIND_DIST: i32 = 4;

/// `meta` byte layout for a water cell:
///   bit 7     — FALLING: a vertical stream (full amount, renders full).
///   bits 0..4 — `level`: 0 = source, 1..=7 = flowing distance from a source.
const FALLING: u8 = 0x80;
const LEVEL_MASK: u8 = 0x0F;

#[inline]
fn level(meta: u8) -> u8 {
    // Clamped: metadata written before the 7-cell reach (and its retired
    // thickness bits, 0x70) may still be in old saves; the first flow check
    // rewrites such a cell clean.
    (meta & LEVEL_MASK).min(7)
}
#[inline]
fn is_falling(meta: u8) -> bool {
    meta & FALLING != 0
}
/// A full, still source: level 0 and not falling (worldgen water is all this).
#[inline]
fn is_source(meta: u8) -> bool {
    meta & (LEVEL_MASK | FALLING) == 0
}
/// How much water the cell holds, 1..=8: full for sources and falling cells,
/// `8 - level` for flowing ones. Drives spreading strength and surface height.
#[inline]
fn amount(meta: u8) -> u8 {
    if is_source(meta) || is_falling(meta) {
        8
    } else {
        8 - level(meta)
    }
}
/// Encode a flowing cell at the given level (1..=7).
#[inline]
fn flowing(level: u8) -> u8 {
    level & LEVEL_MASK
}

const FLOW_DIR_EPS_SQ: f32 = 1e-4;

/// Rendered surface height (0..1) of a water cell with the given metadata, given
/// whether water sits directly above it (a capped cell fills to the top). The
/// canonical meta->height mapping, shared with the mesher so flow geometry and
/// simulation stay in lockstep: `amount / 9`, so a source's top sits slightly
/// recessed (8/9) and reads as liquid, and each level steps down from there.
pub(crate) fn fluid_height(meta: u8, water_above: bool) -> f32 {
    if fills_cell(meta, water_above) {
        return 1.0;
    }
    amount(meta) as f32 / 9.0
}

/// True when this water cell should render as a full block-height volume rather
/// than an open, recessed/sloped surface: capped by water above, or a falling
/// stream (a full column that joins seamlessly to the cell above and to the
/// water it lands in — no mid-waterfall step).
pub(crate) fn fills_cell(meta: u8, water_above: bool) -> bool {
    water_above || is_falling(meta)
}

/// Horizontal direction of the rendered water flow at a cell, using the same
/// surface-gradient rule that rotates the flowing-water top texture. Returns
/// zero for still/flat water and for non-water cells.
pub(crate) fn surface_flow_dir<B, F>(wx: i32, wy: i32, wz: i32, block_at: &B, fluid_at: &F) -> Vec3
where
    B: Fn(i32, i32, i32) -> Block,
    F: Fn(i32, i32, i32) -> Option<f32>,
{
    let Some(my_h) = fluid_at(wx, wy, wz) else {
        return Vec3::ZERO;
    };

    let mut fvx = 0.0f32;
    let mut fvz = 0.0f32;
    for d in CARDINALS {
        let nb = block_at(wx + d.x, wy, wz + d.z);
        let nh = if nb == Block::Water {
            fluid_at(wx + d.x, wy, wz + d.z).unwrap_or(my_h)
        } else if nb == Block::Air {
            0.0
        } else {
            continue;
        };
        let diff = my_h - nh;
        fvx += d.x as f32 * diff;
        fvz += d.z as f32 * diff;
    }

    let flow = Vec3::new(fvx, 0.0, fvz);
    if flow.length_squared() > FLOW_DIR_EPS_SQ {
        flow.normalize()
    } else {
        Vec3::ZERO
    }
}

/// Can water occupy this block, displacing it? Empty air, or any fragile block — water
/// treats a fragile cell (grass, a flower, a torch) as empty space it may flow or fall
/// into, washing the block away as it moves in (see [`fill_with_water`]). Matches "flow
/// to the adjacent empty space", with fragile blocks counting as empty for the flow.
#[inline]
fn fillable(block: Block) -> bool {
    block == Block::Air || block.is_fragile()
}

const DOWN: IVec3 = IVec3::new(0, -1, 0);
const UP: IVec3 = IVec3::new(0, 1, 0);
/// North (-Z), east (+X), south (+Z), west (-X) — the horizontal flow set.
const CARDINALS: [IVec3; 4] = [
    IVec3::new(0, 0, -1),
    IVec3::new(1, 0, 0),
    IVec3::new(0, 0, 1),
    IVec3::new(-1, 0, 0),
];

#[inline]
fn opposite(d: IVec3) -> IVec3 {
    IVec3::new(-d.x, -d.y, -d.z)
}

/// Read a block at world coords through a `World`. The flow algorithm only ever
/// touches the world as a block/water read-write surface; these two helpers plus
/// [`World::set_water_world`] are that whole surface.
#[inline]
fn block_at(world: &World, p: IVec3) -> Block {
    world.physics_block(p.x, p.y, p.z)
}
#[inline]
fn meta_at(world: &World, p: IVec3) -> u8 {
    world.water_meta_world(p.x, p.y, p.z)
}

/// Fill `pos` with water of metadata `meta`, first washing away any fragile block
/// (grass, a flower, a torch) that occupied it — it breaks as the water moves in,
/// dropping and bursting like a hand-break (recorded for the presentation layer via
/// [`World::note_block_destroyed`]). The single choke point for water ENTERING a cell
/// that was not already water, so every flow path that displaces a fragile block breaks
/// it. The caller has already checked [`fillable`], so the occupant is air or fragile.
fn fill_with_water(world: &mut World, pos: IVec3, meta: u8) {
    let occupant = block_at(world, pos);
    if occupant.is_fragile() {
        world.note_block_destroyed(pos, occupant);
    }
    world.set_water_world(pos, Block::Water, meta);
}

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
            let water_above = block_at(p.x, p.y + 1, p.z) == Block::Water;
            Some(fluid_height(meta_at(self, p), water_above))
        };
        surface_flow_dir(wx, wy, wz, &block_at, &fluid_at)
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
        let water_above = block_at(self, IVec3::new(c.x, c.y + 1, c.z)) == Block::Water;
        if p.y - c.y as f32 >= fluid_height(meta_at(self, c), water_above) {
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
    pub(super) fn set_water_world(&mut self, pos: IVec3, block: Block, meta: u8) -> bool {
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
            self.mark_sky_cover_edited_around(cpos.chunk_pos(), change);
        }
        self.queue_dirty_mesh(cpos);
        // A border cell changes neighbour sections' culled faces: re-mesh the 3×3×3.
        self.mark_dirty_neighborhood(cpos, false);
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
/// reborrows) without ever storing a `&mut World` — see [`super::tick`]. The
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

#[cfg(test)]
mod tests {
    use super::*;
    // Source/flow tests place water at y>=65, above flat_world's stone floor.
    use crate::world::testutil::flat_world;

    fn run_ticks(w: &mut World, n: u32) {
        // Water flow needs no recipes; an empty set keeps the furnace step a no-op.
        let recipes = crate::crafting::Recipes::default();
        for _ in 0..n {
            w.game_tick(&recipes);
        }
    }

    fn block(w: &World, x: i32, y: i32, z: i32) -> Block {
        Block::from_id(w.chunk_block(x, y, z))
    }

    fn carve(w: &mut World, x: i32, y: i32, z: i32) {
        w.set_block_world(x, y, z, Block::Air);
    }

    /// One full ring advance: the flow delay, plus slack for the update dispatch.
    const RING: u32 = WATER_FLOW_DELAY as u32 + 2;

    #[test]
    fn bucket_source_check_accepts_only_still_sources() {
        let mut w = flat_world();
        assert!(w.set_water_world(IVec3::new(2, 65, 2), Block::Water, 0));
        assert!(w.set_water_world(IVec3::new(3, 65, 2), Block::Water, flowing(3)));
        assert!(w.set_water_world(IVec3::new(4, 65, 2), Block::Water, FALLING));

        assert!(w.is_water_source_world(IVec3::new(2, 65, 2)));
        assert!(!w.is_water_source_world(IVec3::new(3, 65, 2)), "flowing");
        assert!(!w.is_water_source_world(IVec3::new(4, 65, 2)), "falling");
        assert!(!w.is_water_source_world(IVec3::new(5, 65, 2)), "air");
        assert!(!w.is_water_source_world(IVec3::new(2, 64, 2)), "stone");
    }

    #[test]
    fn water_flow_dir_matches_surface_gradient_used_by_texture() {
        let mut w = flat_world();
        // A one-wide channel: side walls remove sideways air, so the gradient at
        // the flowing cell points east, the same direction its top texture faces.
        for x in 0..=5 {
            w.set_block_world(x, 65, 7, Block::Stone);
            w.set_block_world(x, 65, 9, Block::Stone);
        }
        assert!(w.set_water_world(IVec3::new(2, 65, 8), Block::Water, 0));
        assert!(w.set_water_world(IVec3::new(3, 65, 8), Block::Water, flowing(4)));

        let dir = w.water_flow_dir_at(3, 65, 8);
        assert!(dir.x > 0.99, "expected eastward flow, got {dir:?}");
        assert!(dir.z.abs() < 1e-5, "expected no sideways flow, got {dir:?}");
    }

    /// The body-probe sampler only pushes below the fluid's real surface: a
    /// probe in a flowing cell's top sliver (feet standing on a 15/16 block
    /// beside an irrigation channel) catches no current, while a submerged
    /// probe in the same cell does.
    #[test]
    fn flow_at_a_point_stops_above_the_fluid_surface() {
        let mut w = flat_world();
        for x in 0..=5 {
            w.set_block_world(x, 65, 7, Block::Stone);
            w.set_block_world(x, 65, 9, Block::Stone);
        }
        assert!(w.set_water_world(IVec3::new(2, 65, 8), Block::Water, 0));
        assert!(w.set_water_world(IVec3::new(3, 65, 8), Block::Water, flowing(4)));

        let submerged = w.water_flow_at_point(Vec3::new(3.5, 65.2, 8.5));
        assert!(
            submerged.x > 0.99,
            "a submerged probe drifts: {submerged:?}"
        );
        // 15/16 = 0.9375, above even a full source's 8/9 surface.
        let skimming = w.water_flow_at_point(Vec3::new(3.5, 65.9375, 8.5));
        assert_eq!(skimming, Vec3::ZERO, "above the surface there is no water");
        let source_top = w.water_flow_at_point(Vec3::new(2.5, 65.9375, 8.5));
        assert_eq!(source_top, Vec3::ZERO, "a source tops out at 8/9 too");
        // A capped cell fills to the brim and pushes through its whole height.
        assert!(w.set_water_world(IVec3::new(3, 66, 8), Block::Water, flowing(1)));
        let capped = w.water_flow_at_point(Vec3::new(3.5, 65.9375, 8.5));
        assert!(
            capped.length_squared() > 0.0,
            "water above caps the cell full: {capped:?}"
        );
    }

    #[test]
    fn game_tick_advances_and_block_update_schedules_a_water_check() {
        let mut w = flat_world();
        assert_eq!(w.current_tick(), 0);
        w.set_block_world(8, 65, 8, Block::Water);
        // First tick dispatches the placement update and schedules the flow check;
        // the source has not spread yet.
        w.game_tick(&crate::crafting::Recipes::default());
        assert_eq!(w.current_tick(), 1);
        assert_eq!(block(&w, 9, 65, 8), Block::Air);
        // After the flow delay the source has spread to its cardinal neighbours.
        run_ticks(&mut w, WATER_FLOW_DELAY as u32 + 1);
        assert_eq!(block(&w, 9, 65, 8), Block::Water);
    }

    #[test]
    fn source_spreads_one_ring_per_delay_on_a_flat_floor() {
        let mut w = flat_world();
        w.set_block_world(8, 65, 8, Block::Water);
        run_ticks(&mut w, RING);
        for (dx, dz) in [(0, -1), (1, 0), (0, 1), (-1, 0)] {
            let (x, z) = (8 + dx, 8 + dz);
            assert_eq!(block(&w, x, 65, z), Block::Water, "cardinal {dx},{dz}");
            assert_eq!(level(w.water_meta_world(x, 65, z)), 1);
            assert!(!is_source(w.water_meta_world(x, 65, z)));
        }
        // Diagonals are only reached on a later ring.
        assert_eq!(block(&w, 9, 65, 9), Block::Air);
        // The source itself stays a full source.
        assert!(is_source(w.water_meta_world(8, 65, 8)));
    }

    #[test]
    fn flowing_water_dies_out_after_seven_blocks() {
        let mut w = flat_world();
        w.set_block_world(8, 65, 8, Block::Water);
        // Plenty of rings: 7 spreads at 5 ticks each, plus slack.
        run_ticks(&mut w, 200);
        // The level grows by one per block away from the source...
        assert_eq!(level(w.water_meta_world(12, 65, 8)), 4);
        assert_eq!(level(w.water_meta_world(15, 65, 8)), 7);
        // ...and the flow dies past the last level: the 8th block is dry.
        assert_eq!(block(&w, 16, 65, 8), Block::Air);
    }

    #[test]
    fn source_prefers_flowing_toward_a_downhill_drop() {
        let mut w = flat_world();
        // A hole in the floor two blocks east makes (10,65,8) a drop.
        carve(&mut w, 10, 64, 8);
        w.set_block_world(8, 65, 8, Block::Water);
        run_ticks(&mut w, 12);
        // Water heads east toward the drop only — the other cardinals stay dry.
        assert_eq!(block(&w, 9, 65, 8), Block::Water);
        assert_eq!(block(&w, 7, 65, 8), Block::Air);
        assert_eq!(block(&w, 8, 65, 7), Block::Air);
        assert_eq!(block(&w, 8, 65, 9), Block::Air);
    }

    /// The slope search steers toward a drop up to five cells out — one ring plus
    /// [`SLOPE_FIND_DIST`] steps — and no farther: a drop six cells out is invisible
    /// and the flow spreads every open way instead.
    #[test]
    fn slope_search_sees_a_drop_five_cells_out_but_not_six() {
        let mut w = flat_world();
        carve(&mut w, 13, 64, 8); // five cells east of the source at x=8
        w.set_block_world(8, 65, 8, Block::Water);
        run_ticks(&mut w, RING);
        assert_eq!(block(&w, 9, 65, 8), Block::Water, "toward the drop");
        assert_eq!(block(&w, 7, 65, 8), Block::Air, "away from the drop");
        assert_eq!(block(&w, 8, 65, 7), Block::Air);

        let mut w = flat_world();
        carve(&mut w, 14, 64, 8); // six cells east: out of slope-search range
        w.set_block_world(8, 65, 8, Block::Water);
        run_ticks(&mut w, RING);
        for (dx, dz) in [(0, -1), (1, 0), (0, 1), (-1, 0)] {
            assert_eq!(
                block(&w, 8 + dx, 65, 8 + dz),
                Block::Water,
                "unseen drop: spread all ways ({dx},{dz})"
            );
        }
    }

    #[test]
    fn water_pours_one_block_per_tick_not_the_whole_column_at_once() {
        let mut w = flat_world();
        // Source floats five blocks above the floor (cells y=65..69 are air).
        w.set_block_world(8, 70, 8, Block::Water);

        // Shortly after it begins to pour, only the TOP of the column has filled —
        // the water has not teleported all the way to the floor.
        run_ticks(&mut w, RING);
        assert!(
            is_falling(w.water_meta_world(8, 69, 8)),
            "the block just below the source should be falling"
        );
        assert_eq!(
            block(&w, 8, 65, 8),
            Block::Air,
            "water must not have fallen all the way down in one tick"
        );

        // Given enough ticks it reaches the floor, one block per tick.
        run_ticks(&mut w, 6 * WATER_FLOW_DELAY as u32);
        for y in 65..=69 {
            assert_eq!(block(&w, 8, y, 8), Block::Water, "column y={y}");
            assert!(is_falling(w.water_meta_world(8, y, 8)), "falling y={y}");
        }
        // It rests on the floor, not inside it.
        assert_eq!(block(&w, 8, 64, 8), Block::Stone);
    }

    /// A source over open air pours straight down on its first check — and only
    /// once its stream exists (water below closes the pour) does its next check
    /// take the sideways branch reserved for sources, fanning one ring out. The
    /// two-phase order is the visible signature of the down-first spread rule.
    #[test]
    fn a_source_over_air_pours_first_then_fans_out() {
        let mut w = flat_world();
        w.set_block_world(8, 70, 8, Block::Water);

        // First check: the pour, and nothing else.
        run_ticks(&mut w, RING);
        assert!(is_falling(w.water_meta_world(8, 69, 8)), "poured below");
        assert_eq!(block(&w, 9, 70, 8), Block::Air, "no sideways spread yet");

        // Second check (rescheduled by the stream appearing below): the source
        // now sits on water, and a source on water spreads across it.
        run_ticks(&mut w, RING);
        assert_eq!(block(&w, 9, 70, 8), Block::Water, "fans out after pouring");
        assert!(!is_source(w.water_meta_world(9, 70, 8)));
    }

    /// A source resting on other water spreads across its surface (that is how a
    /// poured bucket sheets over a pond) — but the FLOWING ring it creates, also
    /// suspended over water, must not creep any farther on its own.
    #[test]
    fn a_source_on_a_pool_surface_spreads_across_it_but_its_flow_does_not_creep() {
        let mut w = flat_world();
        w.set_block_world(8, 65, 8, Block::Water); // pool cell on the floor
        w.set_block_world(8, 66, 8, Block::Water); // source resting on it
        run_ticks(&mut w, 3 * RING);

        assert_eq!(block(&w, 9, 66, 8), Block::Water, "sheets over the pool");
        assert_eq!(
            block(&w, 10, 66, 8),
            Block::Air,
            "the flowing sheet, over water itself, must not creep onward"
        );
    }

    /// Flowing water with a way down only goes down — it must NOT also creep
    /// sideways, even after its waterfall is established (the cell's later ticks
    /// see falling water below, not air).
    #[test]
    fn flowing_water_over_a_drop_only_goes_down_not_sideways() {
        let mut w = flat_world();
        // A 1-wide channel at z=8 (walls at z=7/z=9) so the only path east is
        // THROUGH the cell over the drop — water can't go around a flat sheet.
        for x in 0..14 {
            for y in 65..=66 {
                w.set_block_world(x, y, 7, Block::Stone);
                w.set_block_world(x, y, 9, Block::Stone);
            }
        }
        // A one-deep hole at (10,64): floor carved out, a floor one block lower.
        carve(&mut w, 10, 64, 8);
        w.set_block_world(10, 63, 8, Block::Stone);
        // Source three blocks west; water runs east to the hole.
        w.set_block_world(7, 65, 8, Block::Water);
        // Long enough that the cell over the hole ticks many times after pouring.
        run_ticks(&mut w, 300);

        // It reached the drop and poured a falling column...
        assert_eq!(
            block(&w, 10, 65, 8),
            Block::Water,
            "stream reached the drop"
        );
        assert!(
            is_falling(w.water_meta_world(10, 64, 8)),
            "the cell over the hole should pour straight down"
        );
        // ...but never crept east past it (the bug: a cell with falling water below
        // it must keep going down, not start spreading once the column exists).
        assert_eq!(
            block(&w, 11, 65, 8),
            Block::Air,
            "must not creep past the drop"
        );
        assert_eq!(
            block(&w, 12, 65, 8),
            Block::Air,
            "must not creep past the drop"
        );
    }

    /// Flowing water must NOT treat other flowing water as a surface to flow on —
    /// otherwise a layer of flowing water creeps across the top of the water below
    /// it and the body climbs higher over time. Two stacked sources feed an upper
    /// flow sitting directly over a lower sheet; the upper flow must not propagate.
    #[test]
    fn flowing_water_does_not_flow_on_top_of_flowing_water() {
        let mut w = flat_world(); // stone floor at y=64
                                  // Carve the floor and lay a lower one at y=62, in a 1-wide channel: the
                                  // lower sheet sits on y=62 (water at y=63), and the upper flow would sit
                                  // directly on that lower water (at y=64).
        for x in 5..14 {
            carve(&mut w, x, 64, 8);
            w.set_block_world(x, 62, 8, Block::Stone);
            for y in 63..=66 {
                w.set_block_world(x, y, 7, Block::Stone);
                w.set_block_world(x, y, 9, Block::Stone);
            }
        }
        // A two-high source wall at x=6: y=63 feeds the lower sheet, y=64 the upper.
        w.set_block_world(6, 63, 8, Block::Water);
        w.set_block_world(6, 64, 8, Block::Water);
        run_ticks(&mut w, 400);

        // The lower sheet spreads out along its floor...
        assert_eq!(
            block(&w, 11, 63, 8),
            Block::Water,
            "lower sheet should spread"
        );
        // ...but the upper level must NOT ride along on top of it. (A single cell
        // beside the source is fine; it must not propagate down the channel.)
        assert_eq!(
            block(&w, 10, 64, 8),
            Block::Air,
            "flowing water must not flow on water"
        );
        assert_eq!(
            block(&w, 12, 64, 8),
            Block::Air,
            "flowing water must not climb the channel"
        );
    }

    /// The critical invariant: flowing water can never become its own source.
    /// A source on a pillar makes water fall off every side and pool on the floor;
    /// once the source is cut, EVERY flowing and falling cell must drain away —
    /// nothing may sustain itself (no orphaned waterfalls or self-supporting
    /// columns).
    #[test]
    fn cut_off_waterfall_and_pool_fully_drain() {
        let mut w = flat_world();
        // A 2-high stone pillar at (8,8) with a source on top; water spills off all
        // four sides, falls to the floor (y=64) and pools.
        w.set_block_world(8, 65, 8, Block::Stone);
        w.set_block_world(8, 66, 8, Block::Stone);
        w.set_block_world(8, 67, 8, Block::Water);
        run_ticks(&mut w, 250);

        // Sanity: water really did fall and pool (else the test proves nothing).
        let any_falling = (60..68).any(|y| {
            [(7, 8), (9, 8), (8, 7), (8, 9)]
                .iter()
                .any(|&(x, z)| block(&w, x, y, z) == Block::Water)
        });
        let any_pool = block(&w, 6, 65, 8) == Block::Water;
        assert!(
            any_falling && any_pool,
            "setup should produce a waterfall + pool"
        );

        // Cut the source.
        w.set_block_world(8, 67, 8, Block::Air);
        run_ticks(&mut w, 600);

        // Nothing may remain anywhere in the region.
        for y in 65..=68 {
            for z in 0..16 {
                for x in 0..16 {
                    assert_ne!(
                        block(&w, x, y, z),
                        Block::Water,
                        "water left at ({x},{y},{z}) — flowing water sustained itself"
                    );
                }
            }
        }
    }

    #[test]
    fn flowing_water_recedes_when_its_source_is_removed() {
        let mut w = flat_world();
        w.set_block_world(8, 65, 8, Block::Water);
        run_ticks(&mut w, 40); // let the sheet form
        assert_eq!(block(&w, 10, 65, 8), Block::Water);

        // Remove the source; the sheet must drain back to nothing.
        w.set_block_world(8, 65, 8, Block::Air);
        run_ticks(&mut w, 200);
        for r in 1..=4 {
            assert_eq!(
                block(&w, 8 + r, 65, 8),
                Block::Air,
                "ring {r} should be dry"
            );
        }
        assert_eq!(block(&w, 8, 65, 8), Block::Air);
    }

    /// Draining is the re-level rule, not a special path: a cut-off cell steps
    /// DOWN through the levels as its (equally doomed) neighbours weaken, rather
    /// than vanishing outright — its first re-check leans on the still-stale ring
    /// beyond it and lands two levels weaker, not dry.
    #[test]
    fn a_cut_off_sheet_steps_down_through_levels_rather_than_vanishing() {
        let mut w = flat_world();
        w.set_block_world(8, 65, 8, Block::Water);
        run_ticks(&mut w, 60); // full sheet: level == distance from source
        assert_eq!(level(w.water_meta_world(9, 65, 8)), 1);

        w.set_block_world(8, 65, 8, Block::Air);
        run_ticks(&mut w, RING);
        // One re-check later: fed only by the level-2 ring (amount 6), the old
        // level-1 cell now carries amount 5 — level 3. Still water, weaker.
        assert_eq!(block(&w, 9, 65, 8), Block::Water);
        assert_eq!(level(w.water_meta_world(9, 65, 8)), 3);
    }

    #[test]
    fn flowing_water_washes_away_a_fragile_plant() {
        let mut w = flat_world();
        // A flower standing on the floor, in the path of a source two cells west.
        let flower = IVec3::new(10, 65, 8);
        w.set_block_world(flower.x, flower.y, flower.z, Block::Poppy);
        w.set_block_world(8, 65, 8, Block::Water);
        run_ticks(&mut w, 80); // let the sheet reach the flower's cell

        // Water flowed INTO the fragile cell, displacing the plant (a fragile block
        // counts as fillable; the flower didn't stay standing in the water).
        assert_eq!(
            block(&w, flower.x, flower.y, flower.z),
            Block::Water,
            "water should flood the flower's cell, washing it away"
        );
        // ...and it was recorded as a hand-style break (drop + particle burst).
        let breaks = w.take_natural_breaks();
        assert!(
            breaks
                .iter()
                .any(|&(p, b)| p == flower && b == Block::Poppy),
            "the washed-away flower was recorded for its drop + burst"
        );
    }

    /// A water write must schedule the matching relight, like every other block
    /// update. The water path used to skip it ("water is transparent"), but water
    /// can move INTO a cell that held a torch (a light emitter) and wash it away —
    /// and then the torch's glow lingered in the now-stale light. We settle the
    /// owning section's light first so a still-dirty band can't mask the regression.
    #[test]
    fn a_water_write_reschedules_the_light() {
        use crate::chunk::SECTION_VOLUME;
        let mut w = flat_world();
        let cell = IVec3::new(10, 65, 8); // section (0,4,0)
        w.set_block_world(cell.x, cell.y, cell.z, Block::Torch);

        // Install a settled skylight cube so the section's `light_dirty` flag is clear —
        // the baseline a fresh block update has to dirty again.
        w.section_at_world_mut_for_test(cell.x, cell.y, cell.z)
            .unwrap()
            .set_skylight(vec![0u8; SECTION_VOLUME].into());
        assert!(
            !w.section_at_world_for_test(cell.x, cell.y, cell.z)
                .unwrap()
                .light_dirty,
            "baseline: the section's light is settled"
        );

        // Water moves into the torch's cell; the announce must re-dirty the light
        // so the lingering emitter glow gets rebaked.
        assert!(w.set_water_world(cell, Block::Water, FALLING));
        assert_eq!(block(&w, cell.x, cell.y, cell.z), Block::Water);
        assert!(
            w.section_at_world_for_test(cell.x, cell.y, cell.z)
                .unwrap()
                .light_dirty,
            "a water write must reschedule the relight"
        );
    }

    /// The infinite-water-source rule: two sources two cells apart on a solid
    /// floor fill the gap with a flow fed from both sides, and that flow settles
    /// into a source of its own.
    #[test]
    fn one_deep_flow_between_two_sources_becomes_a_source() {
        let mut w = flat_world();
        w.set_block_world(7, 65, 8, Block::Water);
        w.set_block_world(9, 65, 8, Block::Water);
        run_ticks(&mut w, 60);

        assert_eq!(
            block(&w, 8, 65, 8),
            Block::Water,
            "the gap filled with water"
        );
        assert!(
            is_source(w.water_meta_world(8, 65, 8)),
            "a flow on solid ground flanked by two sources must become a source"
        );
        // The flanking sources are of course still sources, and the conversion
        // did not run away across the open floor.
        assert!(is_source(w.water_meta_world(7, 65, 8)));
        assert!(is_source(w.water_meta_world(9, 65, 8)));
        assert!(
            !is_source(w.water_meta_world(8, 65, 7)),
            "the surrounding ring (one source neighbour) stays flowing"
        );
    }

    /// A flow resting on a SOURCE counts as grounded too, so the top layer of a
    /// stacked pool can fill in as well. Stage a lower source, two flanking
    /// sources above it, and a flow between them: the flow rests on the lower
    /// source and converts.
    #[test]
    fn a_one_deep_flow_resting_on_a_source_becomes_a_source() {
        let mut w = flat_world();
        assert!(w.set_water_world(IVec3::new(8, 65, 8), Block::Water, 0)); // lower source
        assert!(w.set_water_world(IVec3::new(7, 66, 8), Block::Water, 0)); // flank
        assert!(w.set_water_world(IVec3::new(9, 66, 8), Block::Water, 0)); // flank
        assert!(w.set_water_world(IVec3::new(8, 66, 8), Block::Water, flowing(1)));
        run_ticks(&mut w, 30);

        assert!(
            is_source(w.water_meta_world(8, 66, 8)),
            "a flow resting on a source, flanked by two sources, converts"
        );
    }

    /// Conversion is judged before the falling state: even a FALLING cell flanked
    /// by two sources over solid ground settles into a source (the base of a
    /// waterfall pouring into an infinite pool heals into the pool).
    #[test]
    fn a_falling_cell_between_two_sources_converts_to_a_source() {
        let mut w = flat_world();
        w.set_block_world(7, 65, 8, Block::Water);
        w.set_block_world(9, 65, 8, Block::Water);
        assert!(w.set_water_world(IVec3::new(8, 65, 8), Block::Water, FALLING));
        run_ticks(&mut w, 30);

        assert!(
            is_source(w.water_meta_world(8, 65, 8)),
            "a falling cell flanked by two sources over solid ground converts"
        );
    }

    /// The anti-flood guard: a flow perched over a drop (air below) is the lip of
    /// a waterfall, NOT grounded, so it must NOT convert even when flanked by
    /// two sources. This is what keeps a flooding cave from turning to sources at
    /// an exponential pace.
    #[test]
    fn a_flow_over_a_drop_never_converts_even_between_two_sources() {
        let mut w = flat_world();
        carve(&mut w, 8, 64, 8); // air below the middle cell — a one-block drop
        w.set_block_world(7, 65, 8, Block::Water);
        w.set_block_world(9, 65, 8, Block::Water);
        run_ticks(&mut w, 80);

        assert_eq!(
            block(&w, 8, 65, 8),
            Block::Water,
            "the gap still carries flowing water poured from the sources"
        );
        assert!(
            !is_source(w.water_meta_world(8, 65, 8)),
            "a flow resting on air (a waterfall lip) must never become a source"
        );
    }
}
