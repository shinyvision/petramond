#[cfg(test)]
use crate::world::ServerWorld;
use crate::world::WorldData;
use crate::world::{World, WorldSide};

use petramond_math::math::IVec3;
use petramond_world::block::Block;

impl<S: WorldSide> World<S> {
    pub fn mark_custom_bake_edit(&mut self, wx: i32, wy: i32, wz: i32, new_block: Block) {
        for (dx, dy, dz) in [
            (0, 0, 0),
            (-1, 0, 0),
            (1, 0, 0),
            (0, -1, 0),
            (0, 1, 0),
            (0, 0, -1),
            (0, 0, 1),
        ] {
            let p = IVec3::new(wx + dx, wy + dy, wz + dz);
            self.data.invalidate_custom_bake(p);
            let cell = if (dx, dy, dz) == (0, 0, 0) {
                new_block
            } else {
                Block::from_id(self.data.chunk_block(p.x, p.y, p.z))
            };
            if cell.is_custom_shape() {
                self.data.content.custom_bake_dirty.insert(p);
            } else {
                self.clear_custom_light_aperture(p);
            }
        }
    }
    pub fn set_custom_render_bake(
        &mut self,
        pos: IVec3,
        boxes: Box<[petramond_world::block::ShapeRenderBox]>,
    ) {
        if let Some((sp, lx, ly, lz)) = WorldData::split_world(pos.x, pos.y, pos.z) {
            if let Some(section) = self.data.section_mut(sp) {
                let idx = petramond_world::chunk::section_idx(lx, ly, lz) as u16;
                section.set_shape_render(idx, boxes);
                self.queue_dirty_meshes_sampling_cell(pos.x, pos.y, pos.z);
            }
        }
    }
    pub fn set_custom_light_aperture(&mut self, pos: IVec3, aperture: mod_api::LightAperture) {
        let opaque = match aperture {
            mod_api::LightAperture::Opaque => true,
            mod_api::LightAperture::Open => false,
        };
        if let Some((sp, lx, ly, lz)) = WorldData::split_world(pos.x, pos.y, pos.z) {
            if let Some(section) = self.data.section_mut(sp) {
                let idx = petramond_world::chunk::section_idx(lx, ly, lz) as u16;
                if section.set_custom_light_aperture(idx, opaque) {
                    self.relight_aperture_change(pos, sp);
                }
            }
        }
    }
    fn clear_custom_light_aperture(&mut self, pos: IVec3) {
        if let Some((sp, lx, ly, lz)) = WorldData::split_world(pos.x, pos.y, pos.z) {
            if let Some(section) = self.data.section_mut(sp) {
                let idx = petramond_world::chunk::section_idx(lx, ly, lz) as u16;
                if section.clear_custom_light_aperture(idx) {
                    self.relight_aperture_change(pos, sp);
                }
            }
        }
    }

    fn relight_aperture_change(&mut self, pos: IVec3, sp: petramond_world::chunk::SectionPos) {
        if !self.queue_incremental_relight(pos) {
            self.mark_light_dirty_neighborhood(sp, true);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use petramond_world::block::Aabb;
    use petramond_world::world::custom_bake::intern_boxes;

    #[test]
    fn interning_dedups_equal_box_sets() {
        let a = [Aabb {
            min: [0.0, 0.0, 0.0],
            max: [1.0, 0.5, 1.0],
        }];
        let b = [Aabb {
            min: [0.0, 0.0, 0.0],
            max: [1.0, 0.5, 1.0],
        }];
        assert!(std::ptr::eq(
            intern_boxes(&a).unwrap(),
            intern_boxes(&b).unwrap()
        ));
    }

    #[test]
    fn cache_stores_reads_and_invalidates() {
        let mut w = ServerWorld::new(0, 4);
        let pos = IVec3::new(3, 64, -7);
        let half = [Aabb {
            min: [0.0, 0.0, 0.0],
            max: [1.0, 0.5, 1.0],
        }];
        assert_eq!(w.data.custom_shape_boxes(pos), None, "no bake yet");
        w.data.set_custom_bake(pos, &half);
        assert_eq!(w.data.custom_shape_boxes(pos), Some(&half[..]));
        w.data.invalidate_custom_bake(pos);
        assert_eq!(w.data.custom_shape_boxes(pos), None);
    }
}
