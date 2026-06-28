//! Flowing-water simulation, Minecraft-style.
//!
//! State lives in one metadata byte per water cell (see `Chunk::water_meta`):
//!   - bits 0..4 `falloff`: 0 = source (full, still), 1..=8 = horizontal distance
//!     from a source. A sheet on a flat surface spans the source plus up to
//!     [`MAX_FALLOFF`] flowing cells.
//!   - bit 4 [`FALLING`]: a vertical stream. Renders full and, where it lands,
//!     spreads like a source.
//!
//! Worldgen oceans/rivers are written as plain `Block::Water` with meta 0, i.e.
//! sources — they sit still until disturbed (a neighbouring block change queues a
//! block update; see [`super::tick`]). 10 ticks (0.5 s) after a disturbance a
//! cell runs its [`FluidSim::flow_check`], which:
//!   1. (flowing only) re-levels from its upstream supplier, or dries up if the
//!      supply is gone — this is how a sheet recedes when its source is removed;
//!   2. (flowing only) settles into a SOURCE if it is one cell deep and flanked
//!      by two or more source blocks — the infinite-water-source rule;
//!   3. pours straight down if there is space below;
//!   4. spreads horizontally, preferring the direction of the nearest downhill
//!      drop within [`SLOPE_FIND_DIST`], else all four cardinals.
//!
//! Every cell it sets announces itself to its neighbours, so the sheet advances
//! one ring per flow delay and naturally crosses chunk borders.

use std::collections::{HashMap, VecDeque};

use crate::block::{Block, BlockBehavior};
use crate::chunk::{ChunkPos, CHUNK_SX, CHUNK_SZ};
use crate::mathh::{IVec3, Vec3};

use super::store::World;

/// Ticks between a water cell being disturbed and its flow check running.
pub(super) const WATER_FLOW_DELAY: u64 = 10;

/// Water's block behaviour. Both hooks delegate to the [`FluidSim`] below, so this
/// is just the wiring that puts water on the generic reaction path. It lives here
/// in `world` (not in `block`) because it drives the world tick scheduler and
/// `FluidSim` — world internals a `block`-side behaviour can't reach — while still
/// implementing the `block`-defined [`BlockBehavior`].
pub struct Water;

impl BlockBehavior for Water {
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

/// Maximum horizontal falloff: flowing water dies out this many blocks from a
/// source on a flat surface ("at most 8 blocks").
const MAX_FALLOFF: u8 = 8;

/// How far the slope search reaches looking for a downhill drop before giving up
/// and spreading in every open direction.
const SLOPE_FIND_DIST: i32 = 5;

/// `meta` byte layout for a water cell:
///   bit 7     — FALLING: a vertical stream (renders full).
///   bits 4..7 — `thickness - 1` (thickness 1..=8): how far behind the leading
///               EDGE of the flow this cell is. The edge is the thinnest film (1)
///               and the body fills in behind it (up to 8) as the flow extends —
///               so water grows into its shape instead of snapping to it. Only
///               meaningful for flowing water.
///   bits 0..4 — falloff: 0 = source, 1..=8 = distance from a source.
const FALLING: u8 = 0x80;
const FALLOFF_MASK: u8 = 0x0F;
const THICKNESS_MASK: u8 = 0x70;
const THICKNESS_SHIFT: u8 = 4;

#[inline]
fn falloff(meta: u8) -> u8 {
    meta & FALLOFF_MASK
}
#[inline]
fn is_falling(meta: u8) -> bool {
    meta & FALLING != 0
}
/// A full, still source: falloff 0 and not falling (worldgen water is all this).
#[inline]
fn is_source(meta: u8) -> bool {
    meta & (FALLOFF_MASK | FALLING) == 0
}
/// Distance behind the leading edge (1 = the edge itself, the thinnest film).
#[inline]
fn thickness(meta: u8) -> u8 {
    ((meta & THICKNESS_MASK) >> THICKNESS_SHIFT) + 1
}
/// Encode a flowing cell from its falloff and build-up thickness (both 1..=8).
#[inline]
fn flowing(falloff: u8, thickness: u8) -> u8 {
    (falloff & FALLOFF_MASK) | ((thickness.clamp(1, 8) - 1) << THICKNESS_SHIFT)
}

/// Full (source / falling) water surface height in blocks — slightly recessed so
/// the top reads as liquid, the classic look.
const FULL_HEIGHT: f32 = 0.875;
const FLOW_DIR_EPS_SQ: f32 = 1e-4;

/// Rendered surface height (0..1) of a water cell with the given metadata, given
/// whether water sits directly above it (a falling column fills to the top). The
/// canonical meta->height mapping, shared with the mesher so flow geometry and
/// simulation stay in lockstep.
pub(crate) fn fluid_height(meta: u8, water_above: bool) -> f32 {
    // Filled to the very top when capped by water above, or when this is a falling
    // vertical stream — a full column that joins seamlessly to the water it lands
    // in and to the cell above it (no mid-waterfall step).
    if fills_cell(meta, water_above) {
        return 1.0;
    }
    if is_source(meta) {
        return FULL_HEIGHT;
    }
    // Height grows with thickness: the leading edge (1) is a thin film, and the
    // body fills toward full (8) as the flow extends behind it.
    FULL_HEIGHT * thickness(meta) as f32 / 8.0
}

/// True when this water cell should render as a full block-height volume rather
/// than an open, recessed/sloped surface.
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

/// Read a block at world coords through a `World`. The flow algorithm only ever
/// touches the world as a block/water read-write surface; these two helpers plus
/// [`World::set_water_world`] are that whole surface.
#[inline]
fn block_at(world: &World, p: IVec3) -> Block {
    Block::from_id(world.chunk_block(p.x, p.y, p.z))
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
        {
            let Some(c) = self.chunks.get_mut(&cpos) else {
                return false;
            };
            c.set_water(lx, ly, lz, block, meta);
            c.modified = true;
        }
        self.invalidate_section_visibility(cpos);
        self.queue_dirty_mesh(cpos);
        // Only a border cell changes a neighbour chunk's culled faces.
        if lx == 0 {
            self.mark_dirty_pos(ChunkPos::new(cpos.cx - 1, cpos.cz));
        } else if lx == CHUNK_SX - 1 {
            self.mark_dirty_pos(ChunkPos::new(cpos.cx + 1, cpos.cz));
        }
        if lz == 0 {
            self.mark_dirty_pos(ChunkPos::new(cpos.cx, cpos.cz - 1));
        } else if lz == CHUNK_SZ - 1 {
            self.mark_dirty_pos(ChunkPos::new(cpos.cx, cpos.cz + 1));
        }
        self.notify_block_and_neighbors(pos.x, pos.y, pos.z);
        true
    }
}

/// The flowing-water simulation: the fluid-flow algorithm (falloff, thickness,
/// supply chains, slope BFS), factored out of `World`.
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
        let meta = meta_at(world, pos);

        if is_falling(meta) {
            self.falling_flow_check(world, pos);
            return;
        }

        let source = is_source(meta);
        let mut fo = falloff(meta);

        // Flowing water re-levels from its upstream supplier (or dries up when the
        // supply is gone — how a sheet recedes once its source is removed), then
        // recomputes its build-up thickness from the cells downstream of it. The
        // cell re-sets when either changes, which notifies upstream neighbours to
        // thicken in turn, so the body fills in one step behind the advancing edge.
        if !source {
            match self.best_supply(world, pos, fo) {
                None => {
                    world.set_water_world(pos, Block::Air, 0);
                    return;
                }
                Some(supply) => fo = supply,
            }
            // A shallow flow flanked by sources settles into a source itself (the
            // infinite-water-source rule). Restricted to a one-cell-deep flow so a
            // flooding cave can't convert to sources at an exponential pace — see
            // [`FluidSim::becomes_source`]. The cell's identity changes here, so
            // stop and let the fresh source pour/spread on its own next flow check.
            if self.becomes_source(world, pos) {
                world.set_water_world(pos, Block::Water, 0);
                return;
            }
            let th = self.flow_thickness(world, pos, fo);
            if falloff(meta) != fo || thickness(meta) != th {
                world.set_water_world(pos, Block::Water, flowing(fo, th));
            }
        }

        // Flowing water spreads horizontally ONLY when it rests on SOLID ground.
        // With air below it pours straight down; with ANY water below it — a
        // falling stream it is feeding, a pool, or a source — it is part of a
        // water COLUMN, not a surface. Treating water as a floor is what lets
        // flowing water flow on flowing water and climb upward over time, so a
        // non-source cell that is not on solid ground never spreads. (A source
        // always falls AND spreads.)
        let below = pos + DOWN;
        let below_block = block_at(world, below);
        if fillable(below_block) {
            self.pour_down(world, below);
        }
        let on_solid = below_block != Block::Air && below_block != Block::Water;
        if !source && !on_solid {
            return;
        }

        // Spread horizontally: source feeds level 1, flowing decrements by one.
        let spread = if source { 1 } else { fo + 1 };
        if spread <= MAX_FALLOFF {
            self.spread_horizontally(world, pos, spread);
        }
    }

    /// Flow update for a falling stream cell.
    fn falling_flow_check(&self, world: &mut World, pos: IVec3) {
        // Falling water is fed only from directly above (the cell that poured into
        // it). Once nothing waters it from above, it is orphaned and must dry up —
        // otherwise a waterfall whose source is cut off (and any pool it feeds)
        // would persist forever. This is what guarantees flowing water always
        // recedes to a real source rather than sustaining itself.
        if block_at(world, pos + UP) != Block::Water {
            world.set_water_world(pos, Block::Air, 0);
            return;
        }
        let below = block_at(world, pos + DOWN);
        if fillable(below) {
            self.pour_down(world, pos + DOWN); // keep falling to the floor
        } else if below != Block::Water {
            // Landed on a solid floor: spread a level-1 ring like a source.
            self.spread_horizontally(world, pos, 1);
        }
        // else: mid-column (water below) — nothing to do.
    }

    /// Pour ONE block of falling water into `start`. The new falling cell carries
    /// the descent downward one block per water tick on its own scheduled tick —
    /// the column is never filled all at once (water falls at the same rate it
    /// spreads, instead of teleporting straight to the floor).
    fn pour_down(&self, world: &mut World, start: IVec3) {
        if start.y >= 0 && fillable(block_at(world, start)) {
            fill_with_water(world, start, FALLING);
        }
    }

    fn spread_horizontally(&self, world: &mut World, pos: IVec3, spread: u8) {
        for d in self.optimal_flow_dirs(world, pos, spread) {
            let np = pos + d;
            let nb = block_at(world, np);
            // A freshly-reached cell is the new leading edge: thinnest (thickness 1).
            // Its own flow checks thicken it once the flow extends past it.
            if fillable(nb) {
                fill_with_water(world, np, flowing(spread, 1));
            } else if nb == Block::Water {
                let nm = meta_at(world, np);
                // Raise weaker downstream flowing water up to our level.
                if !is_source(nm) && !is_falling(nm) && falloff(nm) > spread {
                    world.set_water_world(np, Block::Water, flowing(spread, 1));
                }
            }
        }
    }

    /// Build-up thickness of a flowing cell: a thin film (1) at the leading edge,
    /// plus the greatest thickness among its strictly-downstream flowing
    /// neighbours (higher falloff), capped at the falloff target. So a cell stays
    /// thin until the flow has extended past it, then fills in toward full.
    fn flow_thickness(&self, world: &World, pos: IVec3, fo: u8) -> u8 {
        let mut max_down = 0u8;
        for d in CARDINALS {
            let np = pos + d;
            if block_at(world, np) != Block::Water {
                continue;
            }
            let nm = meta_at(world, np);
            if !is_source(nm) && !is_falling(nm) && falloff(nm) > fo {
                max_down = max_down.max(thickness(nm));
            }
        }
        (1 + max_down).min(9 - fo)
    }

    /// Best (smallest) falloff this flowing cell can be supplied at, or `None` if
    /// nothing upstream feeds it (so it should dry up). A cell is fed by a source
    /// or falling stream directly above, an adjacent source/falling stream, or an
    /// adjacent flowing cell strictly closer to a source — every chain therefore
    /// terminates at a real source, so cutting the source drains everything.
    fn best_supply(&self, world: &World, pos: IVec3, fo: u8) -> Option<u8> {
        // A vertical feed from above counts ONLY if it is a source or a falling
        // stream. Plain flowing water above is just horizontal flow that happens
        // to be stacked, NOT a feed — counting it would let a column of flowing
        // water mutually support itself and never recede (flowing water must
        // never become its own source).
        let up = pos + UP;
        if block_at(world, up) == Block::Water {
            let um = meta_at(world, up);
            if is_source(um) || is_falling(um) {
                return Some(1);
            }
        }
        let mut best: Option<u8> = None;
        for d in CARDINALS {
            let np = pos + d;
            if block_at(world, np) != Block::Water {
                continue;
            }
            let nm = meta_at(world, np);
            let cand = if is_source(nm) || is_falling(nm) {
                1
            } else {
                falloff(nm) + 1
            };
            // Only a strictly-upstream neighbour (cand <= our level) supports us.
            if cand <= fo {
                best = Some(best.map_or(cand, |b| b.min(cand)));
                if best == Some(1) {
                    break;
                }
            }
        }
        best
    }

    /// The infinite-water-source rule: a flowing cell settles into a source of
    /// its own when it is **one cell deep** and flanked by sources on two or more
    /// of its four horizontal faces. Evaluated on the cell's flow check — the
    /// water tick ~[`WATER_FLOW_DELAY`] ticks after it formed. The caller has
    /// already established this is flowing (non-source, non-falling) water.
    ///
    /// Requiring one-cell depth is what stops a flooding cave from turning to
    /// sources at an exponential pace: a waterfall lip (air below) or the surface
    /// of a churning column (flowing/falling below) is never one deep, so only
    /// genuinely shallow pools fed from two sources fill in.
    fn becomes_source(&self, world: &World, pos: IVec3) -> bool {
        if !self.one_cell_deep(world, pos) {
            return false;
        }
        let source_faces = CARDINALS
            .iter()
            .filter(|&&d| {
                let np = pos + d;
                block_at(world, np) == Block::Water && is_source(meta_at(world, np))
            })
            .count();
        source_faces >= 2
    }

    /// Is this water cell exactly one cell deep — resting on firm support rather
    /// than perched over a drop or a deeper body? Solid ground qualifies, and so
    /// does a settled source directly below (a one-deep film on top of a source
    /// still counts, per the spec). Air or moving water below does NOT: that is
    /// the lip of a waterfall or the surface of a column, which must never convert
    /// (see [`FluidSim::becomes_source`]).
    fn one_cell_deep(&self, world: &World, pos: IVec3) -> bool {
        match block_at(world, pos + DOWN) {
            Block::Water => is_source(meta_at(world, pos + DOWN)),
            ground => !fillable(ground),
        }
    }

    /// Directions to spread into: prefer the cardinal(s) leading to the nearest
    /// downhill drop within [`SLOPE_FIND_DIST`]; if none, every open direction.
    /// Always includes adjacent weaker water that wants re-levelling.
    fn optimal_flow_dirs(&self, world: &World, pos: IVec3, spread: u8) -> Vec<IVec3> {
        let mut air_dirs: Vec<IVec3> = Vec::new();
        let mut upgrade_dirs: Vec<IVec3> = Vec::new();
        for d in CARDINALS {
            let np = pos + d;
            let b = block_at(world, np);
            if fillable(b) {
                air_dirs.push(d);
            } else if b == Block::Water {
                let m = meta_at(world, np);
                if !is_source(m) && !is_falling(m) && falloff(m) > spread {
                    upgrade_dirs.push(d);
                }
            }
        }

        if air_dirs.is_empty() {
            return upgrade_dirs;
        }

        let mut out = match self.nearest_drop_dirs(world, pos, &air_dirs) {
            Some(dirs) => dirs,
            None => air_dirs,
        };
        out.extend(upgrade_dirs);
        out
    }

    /// Breadth-first slope search over empty cells at this Y, labelled by the
    /// cardinal they started from. Returns the start directions whose path
    /// reaches a downhill drop (an empty cell with empty space below) soonest, or
    /// `None` if no drop is in range.
    fn nearest_drop_dirs(
        &self,
        world: &World,
        pos: IVec3,
        air_dirs: &[IVec3],
    ) -> Option<Vec<IVec3>> {
        let mut visited: HashMap<IVec3, usize> = HashMap::new();
        let mut queue: VecDeque<(IVec3, usize, i32)> = VecDeque::new();
        for (i, &d) in air_dirs.iter().enumerate() {
            let np = pos + d;
            visited.insert(np, i);
            queue.push_back((np, i, 1));
        }

        let mut best_dist = i32::MAX;
        let mut chosen: Vec<usize> = Vec::new();
        while let Some((cell, origin, dist)) = queue.pop_front() {
            if dist > best_dist {
                continue;
            }
            if fillable(block_at(world, cell + DOWN)) {
                if dist < best_dist {
                    best_dist = dist;
                    chosen.clear();
                }
                if !chosen.contains(&origin) {
                    chosen.push(origin);
                }
                continue; // don't search past a drop
            }
            if dist >= SLOPE_FIND_DIST {
                continue;
            }
            for d2 in CARDINALS {
                let n2 = cell + d2;
                if visited.contains_key(&n2) || !fillable(block_at(world, n2)) {
                    continue;
                }
                visited.insert(n2, origin);
                queue.push_back((n2, origin, dist + 1));
            }
        }

        if best_dist == i32::MAX {
            return None;
        }
        Some(chosen.into_iter().map(|i| air_dirs[i]).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chunk::Chunk;

    /// A world with a 3x3 block of loaded chunks around the origin, a solid stone
    /// floor at y=64, air above. Source/flow tests place water at y>=65.
    fn flat_world() -> World {
        let mut w = World::new(0, 1);
        for cz in -1..=1 {
            for cx in -1..=1 {
                let mut c = Chunk::new(cx, cz);
                for z in 0..CHUNK_SZ {
                    for x in 0..CHUNK_SX {
                        c.set_block(x, 64, z, Block::Stone);
                    }
                }
                w.chunks.insert(ChunkPos::new(cx, cz), c);
            }
        }
        w
    }

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
        assert!(w.set_water_world(IVec3::new(3, 65, 8), Block::Water, flowing(4, 1)));

        let dir = w.water_flow_dir_at(3, 65, 8);
        assert!(dir.x > 0.99, "expected eastward flow, got {dir:?}");
        assert!(dir.z.abs() < 1e-5, "expected no sideways flow, got {dir:?}");
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
        // After the 10-tick delay the source has spread to its cardinal neighbours.
        run_ticks(&mut w, 11);
        assert_eq!(block(&w, 9, 65, 8), Block::Water);
    }

    #[test]
    fn source_spreads_one_ring_per_delay_on_a_flat_floor() {
        let mut w = flat_world();
        w.set_block_world(8, 65, 8, Block::Water);
        run_ticks(&mut w, 12);
        for (dx, dz) in [(0, -1), (1, 0), (0, 1), (-1, 0)] {
            let (x, z) = (8 + dx, 8 + dz);
            assert_eq!(block(&w, x, 65, z), Block::Water, "cardinal {dx},{dz}");
            assert_eq!(falloff(w.water_meta_world(x, 65, z)), 1);
            assert!(!is_source(w.water_meta_world(x, 65, z)));
        }
        // Diagonals are only reached on a later ring.
        assert_eq!(block(&w, 9, 65, 9), Block::Air);
        // The source itself stays a full source.
        assert!(is_source(w.water_meta_world(8, 65, 8)));
    }

    #[test]
    fn flowing_water_dies_out_after_eight_blocks() {
        let mut w = flat_world();
        w.set_block_world(8, 65, 8, Block::Water);
        // Plenty of rings: 8 spreads at 10 ticks each, plus slack.
        run_ticks(&mut w, 200);
        // Falloff grows by one per block away from the source...
        assert_eq!(falloff(w.water_meta_world(12, 65, 8)), 4);
        assert_eq!(falloff(w.water_meta_world(15, 65, 8)), 7);
        assert_eq!(falloff(w.water_meta_world(16, 65, 8)), 8);
        // ...and stops at MAX_FALLOFF: the 9th block is dry.
        assert_eq!(block(&w, 17, 65, 8), Block::Air);
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

    #[test]
    fn water_pours_one_block_per_tick_not_the_whole_column_at_once() {
        let mut w = flat_world();
        // Source floats five blocks above the floor (cells y=65..69 are air).
        w.set_block_world(8, 70, 8, Block::Water);

        // Shortly after it begins to pour, only the TOP of the column has filled —
        // the water has not teleported all the way to the floor.
        run_ticks(&mut w, WATER_FLOW_DELAY as u32 + 2);
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

    /// Flowing water with a way down only goes down — it must NOT also creep
    /// sideways, even after its waterfall is established (the cell's later ticks
    /// see falling water below, not air). A source, by contrast, falls and spreads.
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

    /// Flowing water grows into its shape: a just-reached cell is the thinnest
    /// film, and the body fills in behind the advancing edge over time. The
    /// leading edge always stays a thin film; the base thickens as the flow
    /// extends, until it is full.
    #[test]
    fn flowing_water_builds_up_thickness_behind_the_advancing_edge() {
        let mut w = flat_world();
        // 1-wide channel so the flow is linear (falloff = distance from source).
        for x in 0..14 {
            for y in 65..=66 {
                w.set_block_world(x, y, 7, Block::Stone);
                w.set_block_world(x, y, 9, Block::Stone);
            }
        }
        w.set_block_world(2, 65, 8, Block::Water); // source

        // Early on, the cell next to the source has only just been reached — still
        // near the leading edge, so still thin.
        run_ticks(&mut w, 2 * WATER_FLOW_DELAY as u32);
        let base_early = thickness(w.water_meta_world(3, 65, 8));

        // After the flow has fully extended its 8 blocks east, the base has built
        // up while the leading edge stays a thin film.
        run_ticks(&mut w, 300);
        let base_late = thickness(w.water_meta_world(3, 65, 8)); // falloff 1
        let edge = thickness(w.water_meta_world(10, 65, 8)); // falloff 8

        assert!(
            base_late > base_early,
            "the base must thicken as the flow extends ({base_early} -> {base_late})"
        );
        assert!(
            base_late >= 7,
            "the base should fill in toward full (got {base_late})"
        );
        assert_eq!(edge, 1, "the leading edge stays the thinnest film");
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
        run_ticks(&mut w, 40); // let a few rings form
        assert_eq!(block(&w, 10, 65, 8), Block::Water);

        // Remove the source; the sheet must drain back to nothing.
        w.set_block_world(8, 65, 8, Block::Air);
        run_ticks(&mut w, 120);
        for r in 1..=4 {
            assert_eq!(
                block(&w, 8 + r, 65, 8),
                Block::Air,
                "ring {r} should be dry"
            );
        }
        assert_eq!(block(&w, 8, 65, 8), Block::Air);
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
    /// and then the torch's glow lingered in the now-stale light band. We settle
    /// the chunk's light first so a still-dirty band can't mask the regression.
    #[test]
    fn a_water_write_reschedules_the_light() {
        let mut w = flat_world();
        let cell = IVec3::new(10, 65, 8);
        w.set_block_world(cell.x, cell.y, cell.z, Block::Torch);

        // Bake the chunk's skylight so its `light_dirty` flag is clear — the
        // baseline a fresh block update has to dirty again.
        let pos = ChunkPos::new(0, 0);
        let (band, ylo, yhi) = crate::mesh::compute_chunk_skylight(w.chunks.get(&pos).unwrap());
        w.chunks.get_mut(&pos).unwrap().set_skylight(band, ylo, yhi);
        assert!(
            !w.chunks.get(&pos).unwrap().light_dirty,
            "baseline: the chunk's light is settled"
        );

        // Water moves into the torch's cell; the announce must re-dirty the light
        // so the lingering emitter glow gets rebaked.
        assert!(w.set_water_world(cell, Block::Water, FALLING));
        assert_eq!(block(&w, cell.x, cell.y, cell.z), Block::Water);
        assert!(
            w.chunks.get(&pos).unwrap().light_dirty,
            "a water write must reschedule the relight"
        );
    }

    /// The infinite-water-source rule: two sources two cells apart on a solid
    /// floor fill the gap with a one-cell-deep flow fed from both sides, and that
    /// flow settles into a source of its own.
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
            "a one-deep flow flanked by two sources must become a source"
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

    /// A flow resting on a SOURCE counts as one cell deep too (the spec's explicit
    /// clause), so the top layer of a stacked pool can fill in as well. Stage a
    /// lower source, two flanking sources above it, and a flow between them: the
    /// flow rests on the lower source and converts.
    #[test]
    fn a_one_deep_flow_resting_on_a_source_becomes_a_source() {
        let mut w = flat_world();
        assert!(w.set_water_world(IVec3::new(8, 65, 8), Block::Water, 0)); // lower source
        assert!(w.set_water_world(IVec3::new(7, 66, 8), Block::Water, 0)); // flank
        assert!(w.set_water_world(IVec3::new(9, 66, 8), Block::Water, 0)); // flank
        assert!(w.set_water_world(IVec3::new(8, 66, 8), Block::Water, flowing(1, 1)));
        run_ticks(&mut w, 30);

        assert!(
            is_source(w.water_meta_world(8, 66, 8)),
            "a one-deep flow resting on a source, flanked by two sources, converts"
        );
    }

    /// The anti-flood guard: a flow perched over a drop (air below) is the lip of
    /// a waterfall, NOT one cell deep, so it must NOT convert even when flanked by
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
