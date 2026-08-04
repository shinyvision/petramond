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

use crate::block::Block;
pub use petramond_world::water_math::{
    amount, fills_cell, fluid_height, is_falling, is_source, is_still_source, level,
    surface_flow_dir, CARDINALS, DOWN, FALLING, LEVEL_MASK, UP,
};

use crate::mathh::IVec3;

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

impl crate::world::engine_behavior::EngineBlockBehavior for Water {

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

/// Encode a flowing cell at the given level (1..=7).
#[inline]
fn flowing(level: u8) -> u8 {
    level & LEVEL_MASK
}

/// Can water occupy this block, displacing it? Empty air, or any fragile block — water
/// treats a fragile cell (grass, a flower, a torch) as empty space it may flow or fall
/// into, washing the block away as it moves in (see [`fill_with_water`]). Matches "flow
/// to the adjacent empty space", with fragile blocks counting as empty for the flow.
#[inline]
fn fillable(block: Block) -> bool {
    block == Block::Air || block.is_fragile()
}


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
