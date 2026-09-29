use super::*;

pub struct CustomFamily;

impl ShapeSim for CustomFamily {
    fn light_shape(&self, p: &ShapeParams, _b: Block) -> crate::block::BlockLightShape {
        match p.custom() {
            Some(c) if c.light_shape == crate::block::shape_kind::CustomLight::OpaqueCube => {
                crate::block::BlockLightShape::OpaqueCube
            }
            Some(c) if c.light_shape == crate::block::shape_kind::CustomLight::CustomAperture => {
                crate::block::BlockLightShape::Shaped
            }
            _ => crate::block::BlockLightShape::Open,
        }
    }

    fn collision_boxes(
        &self,
        _p: &ShapeParams,
        nb: &dyn ShapeNeighborhood,
        pos: IVec3,
        block: Block,
    ) -> &'static [Aabb] {
        nb.baked_collision(pos)
            .unwrap_or_else(|| block.collision_boxes())
    }
    fn nav_reads_solid(&self, p: &ShapeParams) -> bool {
        p.custom().is_some_and(|c| c.nav_solid)
    }

    fn occupies_pocket(
        &self,
        _p: &ShapeParams,
        nb: &dyn ShapeNeighborhood,
        pos: IVec3,
        _b: Block,
        lo: [f32; 3],
        hi: [f32; 3],
    ) -> bool {
        nb.baked_collision(pos)
            .is_some_and(|boxes| boxes.iter().any(|bx| overlaps(lo, hi, bx.min, bx.max)))
    }

    fn shade_boxes(
        &self,
        _p: &ShapeParams,
        nb: &dyn ShapeNeighborhood,
        pos: IVec3,
        _b: Block,
        out: &mut Vec<Aabb>,
    ) -> bool {
        if let Some(boxes) = nb.baked_collision(pos) {
            out.extend_from_slice(boxes);
        }
        true
    }

    fn full_face(
        &self,
        _p: &ShapeParams,
        nb: &dyn ShapeNeighborhood,
        pos: IVec3,
        _b: Block,
        dir: IVec3,
    ) -> Option<crate::block::shape_kind::facets::FullFace> {
        crate::block::shape_kind::facets::face_is_solid(nb, pos, dir)
            .then_some(crate::block::shape_kind::facets::FullFace::Shaped)
    }
}

impl ShapeRender for CustomFamily {
    fn mesh_emitter(&self, _p: &ShapeParams) -> MeshEmitter {
        MeshEmitter::Boxes
    }

    fn boxes(&self, ctx: &ShapeCtx<'_>, out: &mut Vec<ShapeBox>) {
        let Some(baked) = ctx.nb.baked(ctx.pos) else {
            return;
        };
        let tiles = ctx.block.tiles();
        out.extend(baked.iter().map(|b| {
            let tint_for = |tile: crate::tile::Tile| {
                let world = (ctx.tint_for)(tile);
                [
                    world[0] * b.tint[0],
                    world[1] * b.tint[1],
                    world[2] * b.tint[2],
                ]
            };
            let mut mb = ShapeBox::uniform(b.aabb, tiles, tint_for).with_ao_strength(b.ao_strength);
            mb.dyed = b.dyed;
            mb
        }));
    }

    fn picks_by_boxes(&self, _p: &ShapeParams) -> bool {
        true
    }
    fn item_render(&self, _p: &ShapeParams, block: Block) -> ItemRender {
        ItemRender::BlockForm(block)
    }
}

impl ShapePlacement for CustomFamily {
    fn authored_state(&self, _block: Block, _state: ShapeState) -> ShapeState {
        ShapeState::NONE
    }
}
