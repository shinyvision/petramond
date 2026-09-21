//! What reachability searches learned about the cells of a box, kept in the
//! world between searches. A caller that probes or floods the same ground
//! again and again pays for reading the world once: the per-cell facts, every
//! foothold's moves, and what moves into each cell stay true until a cell
//! they read changes. The world's change log says which did — a changed cell
//! forgets exactly the cells that read it — and a box any of whose sections
//! streamed in or out (which the log does not record), or that the log has
//! slid past, is thrown away whole.

use std::cell::RefCell;
use std::hash::Hasher;

use petramond_math::math::IVec3;

use crate::mob::path::{self, BoxFacts, BoxLeads, Fact, PathParams, Reads};
use crate::mob::Mob;
use crate::world::World;

/// The lanes of a kept box's [`BoxFacts`]. Every one is a fact of the world
/// alone, never of one search's planned cells, so the table outlives the
/// search.
#[derive(Clone, Copy)]
pub(super) enum FactLane {
    Solid,
    Support,
    Fluid,
    Foothold,
    Passable,
    Partial,
    Hazard,
    HazardFoothold,
    Plain,
}

/// One box's kept facts, for one species (its body decides every fact).
pub struct KeptBox {
    kind: Mob,
    /// The box searches are answered over, and the one their probes read.
    inner: (IVec3, IVec3),
    outer: (IVec3, IVec3),
    facts: BoxFacts,
    /// Where each foothold leads, and what leads into each cell.
    pub(super) leads: BoxLeads,
    pub(super) comes: BoxLeads,
    pub(super) reads: Reads,
    /// Where in the change log all of it is true as of.
    seq: u64,
    streamed: u64,
}

impl KeptBox {
    fn new(world: &World, kind: Mob, inner: (IVec3, IVec3), reads: Reads) -> Self {
        let outer = reads.around(inner.0, inner.1);
        KeptBox {
            kind,
            inner,
            outer,
            facts: BoxFacts::new(outer.0, outer.1),
            leads: BoxLeads::new(inner.0, inner.1),
            comes: BoxLeads::new(inner.0, inner.1),
            reads,
            seq: world.changes_end(),
            streamed: stream_witness(world, outer),
        }
    }

    pub(super) fn lane(&self, lane: FactLane) -> Fact<'_> {
        self.facts.fact(lane as u32)
    }

    fn holds(&self, (min, max): (IVec3, IVec3)) -> bool {
        min.cmpge(self.inner.0).all() && max.cmple(self.inner.1).all()
    }

    /// Bring the box up to date with the world, or report that it cannot be.
    fn refresh(&mut self, world: &World) -> bool {
        if self.streamed != stream_witness(world, self.outer) {
            return false;
        }
        let (next, changed, lost) = world.changes_since(self.seq);
        if lost {
            return false;
        }
        for cell in changed {
            let (min, max) = Reads::readers(cell, self.reads.standing);
            self.facts.forget(min, max);
            let (min, max) = Reads::readers(cell, self.reads.leads);
            self.leads.forget(min, max);
            let (min, max) = Reads::readers(cell, self.reads.comes);
            self.comes.forget(min, max);
        }
        self.seq = next;
        true
    }
}

/// The boxes a world keeps, most recently searched last.
#[derive(Default)]
pub struct KeptBoxes(RefCell<Vec<KeptBox>>);

/// Table cells all kept boxes may hold between them: a handful of building
/// sites, or a few dozen probe neighbourhoods. The least recently searched
/// go first.
const KEPT_CELLS: usize = 2 << 20;

impl KeptBoxes {
    /// Run `search` over the kept box of this species that holds `span`,
    /// brought up to date — or over a fresh one for `fresh_span`, kept after.
    pub(super) fn over<R>(
        &self,
        world: &World,
        kind: Mob,
        params: PathParams,
        span: (IVec3, IVec3),
        fresh_span: (IVec3, IVec3),
        search: impl FnOnce(&KeptBox) -> R,
    ) -> R {
        // Taken out for the search's life: a search never finds itself here.
        let taken = {
            let mut boxes = self.0.borrow_mut();
            boxes
                .iter()
                .position(|b| b.kind == kind && b.holds(span))
                .map(|at| boxes.remove(at))
        };
        let kept = taken
            .and_then(|mut kept| kept.refresh(world).then_some(kept))
            .unwrap_or_else(|| KeptBox::new(world, kind, fresh_span, Reads::of(params)));
        let found = search(&kept);
        let mut boxes = self.0.borrow_mut();
        boxes.push(kept);
        let mut held: usize = boxes.iter().map(|b| b.facts.cells()).sum();
        while held > KEPT_CELLS && boxes.len() > 1 {
            held -= boxes.remove(0).facts.cells();
        }
        found
    }

    /// Forget everything (a test comparing kept answers with fresh ones).
    #[cfg(test)]
    pub(super) fn take(&self) -> Vec<KeptBox> {
        std::mem::take(&mut *self.0.borrow_mut())
    }

    #[cfg(test)]
    pub(super) fn put(&self, boxes: Vec<KeptBox>) {
        *self.0.borrow_mut() = boxes;
    }
}

/// Which of the box's sections are loaded and final, folded to one number:
/// streaming replaces what a cell reads without any change being announced.
fn stream_witness(world: &World, (min, max): (IVec3, IVec3)) -> u64 {
    let mut hasher = rustc_hash::FxHasher::default();
    let step = petramond_world::chunk::SECTION_SIZE as i32;
    let (lo, hi) = (
        min.div_euclid(IVec3::splat(step)),
        max.div_euclid(IVec3::splat(step)),
    );
    for cy in lo.y..=hi.y {
        for cz in lo.z..=hi.z {
            for cx in lo.x..=hi.x {
                let (x, y, z) = (cx * step, cy * step, cz * step);
                hasher.write_u8(
                    u8::from(world.section_loaded_at(x, y, z))
                        | u8::from(world.physics_cell_final_at(x, y, z)) << 1,
                );
            }
        }
    }
    hasher.finish()
}

/// The box a lone probe between two cells is answered over: their bounds with
/// room for a detour, on a coarse grid so probes nearby find it again.
pub(super) fn probe_span(a: IVec3, b: IVec3) -> (IVec3, IVec3) {
    const ROOM: IVec3 = IVec3::new(24, 12, 24);
    const GRID: i32 = 16;
    let snap_down = |c: IVec3| c.div_euclid(IVec3::splat(GRID)) * GRID;
    let (min, max) = (a.min(b) - ROOM, a.max(b) + ROOM);
    let span = (snap_down(min), snap_down(max) + IVec3::splat(GRID - 1));
    // Too far apart for a table: a box of nothing, every cell worked out.
    if path::fits_a_table(span.0, span.1) {
        span
    } else {
        (a, a)
    }
}
