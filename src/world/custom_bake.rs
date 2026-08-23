//! The custom-shape SIM bake cache: per-cell collision boxes a pack's
//! WASM baked, read by the shape's collision facet. A cache MISS (never baked,
//! or a trapped/timed-out bake) falls back to the block row's static collision
//! boxes — the failure policy that keeps placed world data intact while only the
//! bake logic is suspended.
//!
//! Boxes are CONTENT-INTERNED to `'static`, so `World::collision_boxes_at` keeps
//! its `&'static [Aabb]` return without leaking per cell: a gate has two
//! configurations (open / closed), so at most two box sets are ever interned no
//! matter how many gates exist. The intern set is bounded by the shapes'
//! distinct geometries, not by the world.

use crate::world::WorldData;

use petramond_math::math::IVec3;
use petramond_world::block::{Block, ShapeFamily};

use super::store::World;

impl World {
    /// A block at `(wx, wy, wz)` became `new_block`: drop the cached bake for the
    /// cell and its face neighbours (a custom shape may read them), and re-mark
    /// any custom cell dirty for the next bake pump. The single hook both the
    /// authoritative edit (`set_block_world`) and the replica ingest
    /// (`apply_remote_delta`) call, so client prediction bakes the same cells the
    /// server does.
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
            self.invalidate_custom_bake(p);
            let cell = if (dx, dy, dz) == (0, 0, 0) {
                new_block
            } else {
                Block::from_id(self.chunk_block(p.x, p.y, p.z))
            };
            if cell.shape_family() == ShapeFamily::Custom {
                self.content.custom_bake_dirty.insert(p);
            } else {
                // The cell is no longer a custom shape: drop any stale baked
                // light aperture so a later ungated read can't see it (the
                // render-box cache re-bakes with the cell, but the aperture map
                // has no such rewrite path).
                self.clear_custom_light_aperture(p);
            }
        }
    }
    /// Record a custom shape cell's freshly-baked RENDER boxes on its section
    /// (a no-op if the section isn't loaded) — the client render-bake pump. The
    /// section keeps it (and bumps its mesh revision) so the next mesh job draws
    /// the baked geometry instead of the cube fallback.
    pub fn set_custom_render_bake(
        &mut self,
        pos: IVec3,
        boxes: Box<[petramond_world::block::ShapeRenderBox]>,
    ) {
        if let Some((sp, lx, ly, lz)) = WorldData::split_world(pos.x, pos.y, pos.z) {
            if let Some(section) = self.section_mut(sp) {
                let idx = petramond_world::chunk::section_idx(lx, ly, lz) as u16;
                section.set_shape_render(idx, boxes);
                // A fresh bake must ALWAYS end in a remesh. The revision bump
                // above only re-triggers a mesh job already in flight — a
                // block edit queues one on its own apply, but a KV-ONLY
                // re-bake (a dye color change) has no other trigger, so
                // without this the new geometry sat installed and undrawn
                // until an unrelated edit queued the section (the
                // stale-dye-color bug, 2026-07-23).
                self.queue_dirty_meshes_sampling_cell(pos.x, pos.y, pos.z);
            }
        }
    }
    /// Record a custom shape cell's baked light aperture on its section (the
    /// deterministic SIM bake). The wire aperture is already a per-cell "opaque to
    /// light" decision — Opaque blocks light, Open passes it. A real opacity
    /// TRANSITION relights the cell's section neighbourhood so the change
    /// propagates; an unchanged bake costs nothing.
    pub fn set_custom_light_aperture(&mut self, pos: IVec3, aperture: mod_api::LightAperture) {
        let opaque = match aperture {
            mod_api::LightAperture::Opaque => true,
            mod_api::LightAperture::Open => false,
        };
        if let Some((sp, lx, ly, lz)) = WorldData::split_world(pos.x, pos.y, pos.z) {
            if let Some(section) = self.section_mut(sp) {
                let idx = petramond_world::chunk::section_idx(lx, ly, lz) as u16;
                if section.set_custom_light_aperture(idx, opaque) {
                    self.mark_light_dirty_neighborhood(sp, true);
                }
            }
        }
    }
    /// Clear a cell's stored baked light aperture (it stopped being a custom
    /// shape), relighting its section neighbourhood only on a real change.
    fn clear_custom_light_aperture(&mut self, pos: IVec3) {
        if let Some((sp, lx, ly, lz)) = WorldData::split_world(pos.x, pos.y, pos.z) {
            if let Some(section) = self.section_mut(sp) {
                let idx = petramond_world::chunk::section_idx(lx, ly, lz) as u16;
                if section.clear_custom_light_aperture(idx) {
                    self.mark_light_dirty_neighborhood(sp, true);
                }
            }
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
        // Equal content interns to the SAME 'static slice (pointer identity).
        assert!(std::ptr::eq(
            intern_boxes(&a).unwrap(),
            intern_boxes(&b).unwrap()
        ));
    }

    #[test]
    fn cache_stores_reads_and_invalidates() {
        let mut w = World::new(0, 4);
        let pos = IVec3::new(3, 64, -7);
        let half = [Aabb {
            min: [0.0, 0.0, 0.0],
            max: [1.0, 0.5, 1.0],
        }];
        assert_eq!(w.custom_shape_boxes(pos), None, "no bake yet");
        w.set_custom_bake(pos, &half);
        assert_eq!(w.custom_shape_boxes(pos), Some(&half[..]));
        // An edit at the cell drops the bake (the next read falls back / re-bakes).
        w.invalidate_custom_bake(pos);
        assert_eq!(w.custom_shape_boxes(pos), None);
    }
}
