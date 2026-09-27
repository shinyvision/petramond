use crate::block::{Aabb, Block};
use crate::block_model::{self, BlockModelKind};
use crate::facing::Facing;
use crate::mathh::{IVec3, Mat4, Vec3};
use crate::world::data::WorldData;

impl WorldData {
    #[inline]
    pub fn model_offset_at(&self, wx: i32, wy: i32, wz: i32) -> [u8; 3] {
        match self.chunk_at_world(wx, wy, wz) {
            Some((c, lx, ly, lz)) => c.model_offset(lx, ly, lz),
            None => [0, 0, 0],
        }
    }

    #[inline]
    pub fn model_facing_at(&self, wx: i32, wy: i32, wz: i32) -> Facing {
        match self.chunk_at_world(wx, wy, wz) {
            Some((c, lx, ly, lz)) => c.model_facing(lx, ly, lz),
            None => block_model::DEFAULT_MODEL_FACING,
        }
    }

    #[inline]
    pub fn collision_boxes_at(&self, wx: i32, wy: i32, wz: i32) -> &'static [Aabb] {
        let block = self.physics_block(wx, wy, wz);
        if let Some(boxes) = block.static_collision_boxes() {
            return boxes;
        }
        let k = block.shape_kind_def();
        k.sim
            .collision_boxes(&k.params, self, IVec3::new(wx, wy, wz), block)
    }

    #[inline]
    pub fn target_boxes_at(
        &self,
        wx: i32,
        wy: i32,
        wz: i32,
        out: &mut Vec<crate::block::PosedBox>,
    ) {
        let block = self.physics_block(wx, wy, wz);
        let k = block.shape_kind_def();
        k.sim
            .target_boxes(&k.params, self, IVec3::new(wx, wy, wz), block, out)
    }

    #[inline]
    pub fn selection_box_at(&self, wx: i32, wy: i32, wz: i32) -> Option<([f32; 3], [f32; 3])> {
        let block = Block::from_id(self.chunk_block(wx, wy, wz));
        let k = block.shape_kind_def();
        k.render
            .selection_box(&k.params, self, IVec3::new(wx, wy, wz), block)
    }

    #[inline]
    pub fn point_blocked(&self, p: petramond_math::world_pos::WorldPos) -> bool {
        crate::collision::point_in_solid(p.to_array(), |x, y, z| self.collision_boxes_at(x, y, z))
    }

    pub fn model_outline_box(&self, pos: IVec3) -> Option<(IVec3, [f32; 3], [f32; 3])> {
        let block = Block::from_id(self.chunk_block(pos.x, pos.y, pos.z));
        let kind = block.model_kind()?;
        let off = self.model_offset_at(pos.x, pos.y, pos.z);
        let facing = self.model_facing_at(pos.x, pos.y, pos.z);
        let base = block_model::base_from_cell(pos, kind, off, facing);
        let (mn, mx) = block_model::outline_bounds(kind);
        let m = block_model::placement_transform(kind, facing);
        let (mn, mx) = transform_box(m, mn, mx);
        Some((base, mn, mx))
    }

    pub fn model_footprint_cells(base: IVec3, kind: BlockModelKind) -> Vec<IVec3> {
        Self::model_footprint_cells_facing(base, kind, block_model::DEFAULT_MODEL_FACING)
    }

    pub fn model_footprint_cells_facing(
        base: IVec3,
        kind: BlockModelKind,
        facing: Facing,
    ) -> Vec<IVec3> {
        block_model::oriented_footprint_cells(base, kind, facing)
            .into_iter()
            .map(|(cell, _)| cell)
            .collect()
    }

    pub fn model_footprint_clear(&self, origin: IVec3, kind: BlockModelKind) -> bool {
        self.model_footprint_clear_facing(origin, kind, block_model::DEFAULT_MODEL_FACING)
    }

    pub fn model_footprint_clear_facing(
        &self,
        base: IVec3,
        kind: BlockModelKind,
        facing: Facing,
    ) -> bool {
        Self::model_footprint_cells_facing(base, kind, facing)
            .into_iter()
            .all(|c| self.placement_cell_open(c))
    }
}

fn transform_box(m: Mat4, min: [f32; 3], max: [f32; 3]) -> ([f32; 3], [f32; 3]) {
    let mn = Vec3::from(min);
    let mx = Vec3::from(max);
    let mut out_min = Vec3::splat(f32::INFINITY);
    let mut out_max = Vec3::splat(f32::NEG_INFINITY);
    for x in [mn.x, mx.x] {
        for y in [mn.y, mx.y] {
            for z in [mn.z, mx.z] {
                let p = m.transform_point3(Vec3::new(x, y, z));
                out_min = out_min.min(p);
                out_max = out_max.max(p);
            }
        }
    }
    (out_min.to_array(), out_max.to_array())
}
