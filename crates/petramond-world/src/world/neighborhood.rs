use crate::block::{
    Aabb, Block, CellPart, ShapeBox, ShapeNeighborhood, ShapeRenderBox, ShapeState,
};
use crate::chunk::section_idx;
use crate::mathh::IVec3;

use super::data::WorldData;

impl WorldData {
    pub fn shape_draw_boxes(&self, pos: IVec3, out: &mut Vec<ShapeBox>) {
        out.clear();
        let block = self.block(pos);
        if !block.has_box_shape() {
            return;
        }
        let k = block.shape_kind_def();
        let tint_for = |_: crate::tile::Tile| [1.0f32; 3];
        k.render.boxes(
            &crate::block::ShapeCtx {
                nb: self,
                pos,
                block,
                params: &k.params,
                tint_for: &tint_for,
                part_tint: crate::block::NO_PART_TINT,
            },
            out,
        );
    }

    pub fn cell_parts(&self, pos: IVec3) -> Option<Vec<(CellPart, Block)>> {
        let block = self.physics_block(pos.x, pos.y, pos.z);
        let k = block.shape_kind_def();
        k.sim.parts(&k.params, self, pos, block)
    }

    pub fn cell_burst_tint(&self, pos: IVec3) -> Option<[u8; 3]> {
        let read = |part: CellPart| {
            self.cell_kv_get(
                pos.x,
                pos.y,
                pos.z,
                &crate::block::part_kv_key(crate::block::TINT_KV_KEY, part),
            )
            .and_then(|v| <[u8; 3]>::try_from(v).ok())
        };
        let (part, tint) = match self.cell_parts(pos) {
            Some(parts) => parts
                .into_iter()
                .find_map(|(part, _)| Some((part, read(part)?))),
            None => Some((0, read(0)?)),
        }?;
        // Bursts wear the block's own tiles, so they take the tint only where
        // the body does: a dyed flag's pole breaks into undyed chips.
        let mut boxes = Vec::new();
        self.shape_draw_boxes(pos, &mut boxes);
        (boxes.is_empty() || boxes.iter().any(|b| b.part == part && b.dyeable)).then_some(tint)
    }
}

impl ShapeNeighborhood for WorldData {
    fn block(&self, pos: IVec3) -> Block {
        self.physics_block(pos.x, pos.y, pos.z)
    }

    fn shape_state(&self, pos: IVec3) -> ShapeState {
        match self.chunk_at_world(pos.x, pos.y, pos.z) {
            Some((c, lx, ly, lz)) => c.cell_state(lx, ly, lz),
            None => ShapeState::NONE,
        }
    }

    fn baked(&self, pos: IVec3) -> Option<&[ShapeRenderBox]> {
        let (c, lx, ly, lz) = self.chunk_at_world(pos.x, pos.y, pos.z)?;
        c.shape_render_boxes(section_idx(lx, ly, lz) as u16)
    }

    fn baked_collision(&self, pos: IVec3) -> Option<&'static [Aabb]> {
        self.custom_shape_boxes(pos)
    }
}
