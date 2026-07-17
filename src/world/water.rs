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
use crate::mathh::{IVec3, Vec3};

use super::store::World;

mod sim;

#[cfg(test)]
mod tests;

use sim::FluidSim;

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

/// Rendered/contact surface height (0..1) of a water cell — `1.0` when
/// [`fills_cell`] says the cell presents no open surface, else the canonical
/// meta->height mapping shared with the mesher so flow geometry and simulation
/// stay in lockstep: `amount / 9`, so a source's top sits slightly recessed
/// (8/9) and reads as liquid, and each level steps down from there.
pub(crate) fn fluid_height(meta: u8, above: Block) -> f32 {
    if fills_cell(meta, above) {
        return 1.0;
    }
    amount(meta) as f32 / 9.0
}

/// True when this water cell renders (and contact-probes) as a full
/// block-height volume rather than an open, recessed/sloped surface: more
/// WATER directly above (a mid-column cell), or a FALLING stream cell (a full
/// column that joins seamlessly to the cell above and to the water it lands
/// in — no mid-waterfall step).
///
/// SOLID lids deliberately do NOT cap: water under ANY block — ice, stone, a
/// placed block — keeps the same recessed 8/9 pocket under it, uniformly
/// (three lid variants were tried on 2026-07-16 and all rejected by playtest:
/// any-solid seals, still-source-under-solid seals, still-source-under-ice
/// seals). The calm look of those pockets comes from the STILL-SOURCE flow
/// rules instead ([`surface_flow_dir`] + the mesher's still side tiles), not
/// from faking the height. The flow SIM is untouched by all of this; the
/// mesher, buoyancy/contact probes, and the underwater-camera test share this
/// one rule.
pub(crate) fn fills_cell(meta: u8, above: Block) -> bool {
    above == Block::Water || is_falling(meta)
}

/// Whether this water meta is a STILL SOURCE — exposed for the mesher's flow
/// probe (see [`surface_flow_dir`]): two adjacent still sources never flow
/// into each other, whatever their rendered heights.
#[inline]
pub(crate) fn is_still_source(meta: u8) -> bool {
    is_source(meta)
}

/// Horizontal direction of the rendered water flow at a cell, using the same
/// surface-gradient rule that rotates the flowing-water top texture. Returns
/// zero for still/flat water and for non-water cells.
///
/// Flow direction is a statement about the SIM STATE, not about rendered
/// heights: between two STILL SOURCES there is no flow — period — so their
/// height difference contributes nothing. Without that rule, the recessed
/// 8/9 cell under any block sitting in the sea slopes against its full
/// mid-column neighbours and the whole neighbourhood grows animated flow
/// streaks plus a phantom current, on water that is entirely still. Real
/// gradients survive: flowing/falling metas, and the pull toward an open
/// air edge (where a source genuinely will spread).
pub(crate) fn surface_flow_dir<B, F, S>(
    wx: i32,
    wy: i32,
    wz: i32,
    block_at: &B,
    fluid_at: &F,
    still_at: &S,
) -> Vec3
where
    B: Fn(i32, i32, i32) -> Block,
    F: Fn(i32, i32, i32) -> Option<f32>,
    S: Fn(i32, i32, i32) -> bool,
{
    let Some(my_h) = fluid_at(wx, wy, wz) else {
        return Vec3::ZERO;
    };
    let i_am_still = still_at(wx, wy, wz);

    let mut fvx = 0.0f32;
    let mut fvz = 0.0f32;
    for d in CARDINALS {
        let (nx, nz) = (wx + d.x, wz + d.z);
        let nb = block_at(nx, wy, nz);
        let nh = if nb == Block::Water {
            if i_am_still && still_at(nx, wy, nz) {
                continue; // still source ↔ still source: no flow between them
            }
            fluid_at(nx, wy, nz).unwrap_or(my_h)
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
