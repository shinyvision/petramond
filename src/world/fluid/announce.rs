use rustc_hash::{FxHashMap, FxHashSet};

use crate::world::store::SkyCoverChange;
use crate::world::{World, WorldSide};
use petramond_math::math::IVec3;
use petramond_world::chunk::SectionPos;

#[derive(Default)]
pub(in crate::world) struct FluidAnnounce {
    cells: Vec<IVec3>,
    touched: FxHashSet<IVec3>,
    sections: FxHashSet<SectionPos>,
    sky_cover: FxHashMap<(i32, i32), SkyCoverChange>,
}

impl FluidAnnounce {
    pub(super) fn note_write(&mut self, pos: IVec3, section: SectionPos) -> bool {
        self.sections.insert(section);
        if self.touched.insert(pos) {
            self.cells.push(pos);
            true
        } else {
            false
        }
    }

    pub(super) fn note_sky_cover(&mut self, x: i32, z: i32, change: SkyCoverChange) {
        self.sky_cover
            .entry((x, z))
            .and_modify(|merged| merged.merge(change))
            .or_insert(change);
    }

    #[cfg(test)]
    pub(super) fn cells(&self) -> &[IVec3] {
        &self.cells
    }

    #[cfg(test)]
    pub(super) fn section_count(&self) -> usize {
        self.sections.len()
    }

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
            world.queue_dirty_meshes_sampling_cell(p.x, p.y, p.z);
            world.record_cell_change(p, World::<S>::LIGHT_REACH, true);
        }
    }
}
