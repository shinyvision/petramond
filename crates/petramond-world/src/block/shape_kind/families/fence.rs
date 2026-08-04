//! The fence (and any parameterized wall/hedge): post + neighbour-resolved rails.
//!
//! Sim, render, and placement for this family live together here; the shared
//! seam helpers and the singleton table stay in the parent.

use super::*;

/// A fence (or a parameterized wall/hedge): post + arms resolved from neighbours, read
/// solid by nav, all dimensions/rule/item-form from the connection params.
pub struct FenceFamily;

impl ShapeSim for FenceFamily {
    fn default_boxes(&self, p: &ShapeParams, _b: Block) -> &'static [Aabb] {
        crate::connect::boxes_for_mask(conn(p).boxes, 0)
    }

    fn collision_boxes(
        &self,
        p: &ShapeParams,
        nb: &dyn ShapeNeighborhood,
        pos: IVec3,
        _b: Block,
    ) -> &'static [Aabb] {
        connection_boxes(nb, pos, conn(p), ShapeFamily::Fence)
    }

    fn refine_state(
        &self,
        p: &ShapeParams,
        nb: &dyn ShapeNeighborhood,
        pos: IVec3,
        _b: Block,
        _state: ShapeState,
    ) -> ShapeState {
        crate::connect::ConnectionMask(resolve_connection_mask(
            nb,
            pos,
            conn(p).rule,
            ShapeFamily::Fence,
        ))
        .to_cell()
    }

    fn full_face(
        &self,
        _p: &ShapeParams,
        _nb: &dyn ShapeNeighborhood,
        _pos: IVec3,
        _b: Block,
        dir: IVec3,
    ) -> Option<crate::block::shape_kind::facets::FullFace> {
        // The post's flat top holds a floor mount (a torch on a fence post);
        // the sides are never a complete wall face.
        (dir.y > 0).then_some(crate::block::shape_kind::facets::FullFace::Shaped)
    }

    fn nav_reads_solid(&self, _p: &ShapeParams) -> bool {
        true
    }

    fn occupies_pocket(
        &self,
        p: &ShapeParams,
        _nb: &dyn ShapeNeighborhood,
        _pos: IVec3,
        _b: Block,
        lo: [f32; 3],
        hi: [f32; 3],
    ) -> bool {
        let c = conn(p);
        lo[0] < c.post_hi && hi[0] > c.post_lo && lo[2] < c.post_hi && hi[2] > c.post_lo
    }
}

impl ShapeRender for FenceFamily {
    fn item_boxes(
        &self,
        p: &ShapeParams,
        _b: Block,
        _state: crate::block_state::HeldBlockState,
        out: &mut Vec<crate::block::ItemBox>,
    ) {
        // The item is an authored SEGMENT — two posts joined by two rails —
        // not the bare post a neighbourless cell resolves to. Extents come
        // from the row's own connection params, so a modded wall's item
        // matches its placed thickness.
        //
        // The ROW decides: a connection shape declaring `item_form: "cube"`
        // gets the plain cube icon, not this segment. (Before the item form
        // moved onto the facet, `render::item_cube` branched on the FAMILY and
        // drew the segment regardless of what the row asked for.)
        let Some(c) = p.connection() else { return };
        if c.item_form != ItemForm::Segment {
            return;
        }
        let (post_lo, post_hi) = (c.post_lo, c.post_hi);
        for post in crate::fence::item_posts(post_lo, post_hi) {
            out.push(crate::block::ItemBox::solid(post.min, post.max));
        }
        for rail in crate::fence::item_rails(post_lo, post_hi) {
            // Rail ends butt against the posts, so only the long faces draw.
            let mut item = crate::block::ItemBox::solid(rail.min, rail.max);
            item.faces[petramond_math::face::Face::PosX as usize] = false;
            item.faces[petramond_math::face::Face::NegX as usize] = false;
            out.push(item);
        }
    }

    fn boxes(&self, ctx: &ShapeCtx<'_>, out: &mut Vec<ShapeBox>) {
        let tiles = ctx.block.tiles();
        let c = conn(ctx.params);
        let mask = crate::connect::ConnectionMask::from_cell(ctx.nb.shape_state(ctx.pos)).0;
        crate::shape_mesh::fence::push_mesh_boxes(
            out,
            c.post_lo,
            c.post_hi,
            mask,
            tiles,
            (ctx.tint_for)(tiles[2]),
        );
    }

    fn picks_by_boxes(&self, _p: &ShapeParams) -> bool {
        true
    }
    fn selection_box(
        &self,
        p: &ShapeParams,
        nb: &dyn ShapeNeighborhood,
        pos: IVec3,
        _b: Block,
    ) -> Option<([f32; 3], [f32; 3])> {
        union(connection_boxes(nb, pos, conn(p), ShapeFamily::Fence))
    }
    fn item_render(&self, p: &ShapeParams, block: Block) -> ItemRender {
        item_from_form(conn(p).item_form, block)
    }
}

impl ShapePlacement for FenceFamily {
    fn placement_plan(
        &self,
        w: &WorldData,
        block: Block,
        inputs: &PlaceInputs,
        occupied: &mut dyn FnMut(IVec3, &[Aabb]) -> bool,
    ) -> PlacementOutcome {
        connection_placement(w, block, inputs.place_pos, ShapeFamily::Fence, occupied)
    }
}
