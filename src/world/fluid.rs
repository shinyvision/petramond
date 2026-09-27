//! Flowing-fluid simulation: every fluid row runs one machine on its own
//! descriptor (`FluidDef`).
//!
//! State lives in one metadata byte per fluid cell (see `Section`'s per-cell
//! fluid meta buffer):
//!   - bits 0..4 `level`: 0 = a still SOURCE; 1..=7 = flowing fluid, with per-fluid
//!     drop-off per horizontal block travelled. A cell's *amount* is `8 - level`
//!     (sources and falling cells hold the full 8) and its rendered height is
//!     `amount / 9`.
//!   - bit 7 [`FALLING`]: a vertical stream cell (full amount, renders full).
//!
//! Worldgen writes its fluid bodies as plain fluid blocks with meta 0 — sources
//! that sit still until disturbed. A neighbouring block change queues a block
//! update, which schedules the cell's flow check its fluid's delay ticks out.
//! The check, `FluidSim::flow_check`, does two things:
//!
//!   1. **Re-level** (flowing/falling cells only; a source is never
//!      re-evaluated, only a bucket or block edit removes one): recompute the
//!      cell from its neighbours — two or more SOURCE neighbours over a solid
//!      floor or over another source convert it into a source itself only if
//!      the fluid is `renewable`; any fluid directly above
//!      forces a full falling cell; otherwise it takes the strongest horizontal
//!      neighbour's amount minus the fluid's drop-off, drying up when nothing
//!      feeds it. This one rule is also the decay path: cut the source and every
//!      cell re-levels downward, ring by ring, until the sheet is gone.
//!   2. **Spread**: pour into the cell below when it can accept fluid (also
//!      spreading sideways then only when flanked by 3+ sources — the interior
//!      edge of a big pool keeps feeding outward while its edge pours down).
//!      When it cannot pour it spreads sideways only from solid ground, or —
//!      a source only — across a still body of its fluid (a source below it,
//!      the bucket poured onto a pond). A cell over its own FALLING stream
//!      never spreads sideways, source or not: it is the head of a column,
//!      not a surface, and the column fans out where it LANDS. Sideways flow prefers the
//!      direction(s) whose open path reaches a drop soonest (a bounded slope
//!      search); with no drop in range it spreads every open way. A falling
//!      cell landing on solid ground spreads a full-strength ring, like a
//!      source's outflow.
//!
//! Flow never replaces one fluid with another: existing fluid re-levels itself
//! on its own scheduled check. Every write
//! announces itself to its neighbours, so a sheet advances one ring per flow
//! delay and naturally crosses chunk borders.
//!
//! Flow checks run on their OWN scheduler (`TickState::fluid`), drained each
//! tick under [`FLUID_CHECKS_PER_TICK`]: a lake breach or a flooded cave
//! spreads its work over later ticks (the carry-over stays queued,
//! oldest-first, so no check is lost and the order stays deterministic)
//! instead of stalling the tick for everyone, and other behaviours' scheduled
//! ticks never queue behind it. Checks read the world through a
//! [`SectionCursor`](crate::world::SectionCursor) — a spreading cell's slope
//! search probes dozens of neighbours, nearly all in the section it just
//! read — and the tick's writes are announced as one batch (see
//! [`announce`]).
//!
//! A fluid with a `quench` row reacts with its quencher (`quench.by`): the
//! quencher above or beside it turns the quenching cell into `quench.result`
//! immediately on either fluid's block update. A quenching fluid coming from
//! above must flow into the quencher on its own scheduled flow check: the
//! receiving cell becomes the result and the upstream cell survives. Neighbor
//! notifications alone never advance this downward reaction, and neither
//! fluid's metadata changes the outcome.

use crate::world::sim_guard::{SimReadiness, SIM_RETRY_DELAY};
use crate::world::{SectionCursor, ServerWorld, World, WorldSide};
use petramond_world::block::Block;
use petramond_world::fluid::FluidDef;
pub use petramond_world::fluid_math::{
    amount, fills_cell, fluid_height, is_falling, is_source, is_still_source, level,
    surface_flow_dir, CARDINALS, DOWN, FALLING, LEVEL_MASK, UP,
};

use petramond_math::math::IVec3;

mod announce;
mod contact;
mod sim;

#[cfg(test)]
mod tests;

pub(in crate::world) use announce::FluidAnnounce;
use sim::FluidSim;

pub(in crate::world) const FLUID_CHECKS_PER_TICK: usize = 2048;

pub(super) struct FluidBehavior;

#[inline]
pub(super) fn fluid_of(block: Block) -> Option<&'static FluidDef> {
    block.fluid_def()
}

impl crate::world::engine_behavior::EngineBlockBehavior for FluidBehavior {
    fn neighbor_update(&self, world: &mut ServerWorld, pos: IVec3) {
        if let Some(fluid) = fluid_of(block_at(world, pos)) {
            contact::react(world, pos, fluid);
            if block_at(world, pos) == fluid.block {
                world.schedule_fluid_tick(pos, fluid.delay);
            }
        }
    }

    fn scheduled_tick(&self, world: &mut ServerWorld, pos: IVec3) {
        let mut announce = FluidAnnounce::default();
        FluidSim.flow_check(world, pos, &mut announce);
        announce.flush(world);
    }
}

impl ServerWorld {
    pub(in crate::world) fn run_fluid_checks(&mut self, now: u64, due: &mut Vec<IVec3>) {
        due.clear();
        while due.len() < FLUID_CHECKS_PER_TICK {
            let Some(pos) = self.data.sim.fluid.pop_due(now) else {
                break;
            };
            due.push(pos);
        }
        if due.is_empty() {
            return;
        }
        let mut announce = FluidAnnounce::default();
        for pos in due.drain(..) {
            match self.sim_readiness_at(pos) {
                SimReadiness::Ready => FluidSim.flow_check(self, pos, &mut announce),
                SimReadiness::Wait => self.schedule_fluid_tick(pos, SIM_RETRY_DELAY),
                SimReadiness::Drop => {}
            }
        }
        announce.flush(self);
    }

    pub fn pending_fluid_checks(&self) -> usize {
        self.data.sim.fluid.len()
    }
}

pub(super) static FLUID: FluidBehavior = FluidBehavior;

const SLOPE_FIND_DIST: i32 = 4;

#[inline]
fn flowing(level: u8) -> u8 {
    level & LEVEL_MASK
}

#[inline]
fn fillable(block: Block) -> bool {
    block == Block::Air || block.is_fragile()
}

#[inline]
fn opposite(d: IVec3) -> IVec3 {
    IVec3::new(-d.x, -d.y, -d.z)
}

#[inline]
fn block_at<S: WorldSide>(world: &World<S>, p: IVec3) -> Block {
    world.data.physics_block(p.x, p.y, p.z)
}

pub(super) struct FluidReads<'w> {
    cursor: SectionCursor<'w>,
}

impl<'w> FluidReads<'w> {
    #[inline]
    pub(super) fn new(world: &'w ServerWorld) -> Self {
        Self {
            cursor: world.cursor(),
        }
    }

    #[inline]
    pub(super) fn block(&self, p: IVec3) -> Block {
        self.cursor.physics_block(p)
    }

    #[inline]
    pub(super) fn meta(&self, p: IVec3) -> u8 {
        self.cursor.fluid_meta(p)
    }
}

fn fill_with_fluid(
    world: &mut ServerWorld,
    pos: IVec3,
    block: Block,
    meta: u8,
    announce: &mut FluidAnnounce,
) {
    let occupant = block_at(world, pos);
    if occupant.is_fragile() {
        world.note_block_destroyed(pos, occupant);
    }
    world.write_fluid_cell(pos, block, meta, announce);
}
