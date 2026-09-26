//! Batched announcement of fluid writes.
//!
//! A flow front rewrites the same cells and sections many times per tick
//! (re-level, then a neighbour's spread, then a dry-up), and every write used
//! to pay the whole editor fan-out on the spot: the section's emitter index,
//! a sky-cover sweep over the 3×3 column stacks, the mesh-sampling fan-out,
//! the replication delta, the change log, the relight and seven block
//! updates. The fluid pass instead records its writes here and announces the
//! batch once when it ends:
//!
//! - block updates queue at a cell's FIRST write — the update queue is an
//!   ordered, deduplicated batch drained only after the fluid pass, so the
//!   dispatch order is exactly what per-write announcing produced;
//! - the emitter index refreshes once per touched section;
//! - sky-cover moves merge per world column into one invalidation sweep;
//! - the mesh fan-out, replication delta, change-log entry and relight run
//!   once per touched cell, reading the cell's FINAL state — the delta log is
//!   latest-wins per cell and the relight reads the stored cubes at drain
//!   time, so the outcome equals announcing every intermediate write.
//!
//! [`World::set_fluid_world`] is a batch of one, flushed immediately.

use rustc_hash::{FxHashMap, FxHashSet};

use crate::world::store::SkyCoverChange;
use crate::world::{World, WorldSide};
use petramond_math::math::IVec3;
use petramond_world::chunk::SectionPos;

/// The writes of one fluid batch awaiting their announcement.
#[derive(Default)]
pub(in crate::world) struct FluidAnnounce {
    /// Touched cells in first-write order.
    cells: Vec<IVec3>,
    touched: FxHashSet<IVec3>,
    /// Sections whose cells were written (emitter-index refresh).
    sections: FxHashSet<SectionPos>,
    /// Merged sky-cover moves per world `(x, z)` column.
    sky_cover: FxHashMap<(i32, i32), SkyCoverChange>,
}

impl FluidAnnounce {
    /// Record one written cell of section `section`. Returns whether this is
    /// the cell's first write in the batch — the caller queues its block
    /// updates then.
    pub(super) fn note_write(&mut self, pos: IVec3, section: SectionPos) -> bool {
        self.sections.insert(section);
        if self.touched.insert(pos) {
            self.cells.push(pos);
            true
        } else {
            false
        }
    }

    /// Record a sky-cover move a write caused at world column `(x, z)`.
    pub(super) fn note_sky_cover(&mut self, x: i32, z: i32, change: SkyCoverChange) {
        self.sky_cover
            .entry((x, z))
            .and_modify(|merged| merged.merge(change))
            .or_insert(change);
    }

    /// Cells written in this batch, in first-write order.
    #[cfg(test)]
    pub(super) fn cells(&self) -> &[IVec3] {
        &self.cells
    }

    /// Sections written in this batch.
    #[cfg(test)]
    pub(super) fn section_count(&self) -> usize {
        self.sections.len()
    }

    /// Announce every recorded write to `world` (see the module docs).
    pub(super) fn flush<S: WorldSide>(self, world: &mut World<S>) {
        let FluidAnnounce {
            cells,
            touched: _,
            sections,
            sky_cover,
        } = self;
        for sp in sections {
            world.refresh_particle_emitter_index(sp);
        }
        for ((x, z), change) in sky_cover {
            world.mark_sky_cover_edited_at(x, z, change);
        }
        for p in cells {
            // A border cell changes neighbour sections' culled faces: re-mesh
            // every section whose pad samples this cell (a no-op on the
            // server, which has no meshes).
            world.queue_dirty_meshes_sampling_cell(p.x, p.y, p.z);
            // A fluid is itself transparent and a fluid cell's emission is
            // seeded by the light gather, but fluid can move INTO a cell that
            // held a torch or other emitter (see `fill_with_fluid`), so the
            // relight rides with the announce like any other edit.
            world.record_cell_change(p, World::<S>::LIGHT_REACH, true);
        }
    }
}
