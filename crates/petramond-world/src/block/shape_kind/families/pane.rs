//! The glass pane (and any parameterized bar): post + neighbour-resolved arms.
//!
//! Sim, render, and placement for this family live together here; the shared
//! seam helpers and the singleton table stay in the parent.

use super::*;

/// A glass pane (or a parameterized bar): post + arms resolved from neighbours, all
/// dimensions/rule/item-form from the connection params.
pub struct PaneFamily;

impl ShapeSim for PaneFamily {
    fn default_boxes(&self, p: &ShapeParams, _b: Block) -> &'static [Aabb] {
        // The bare no-neighbour post.
        crate::connect::boxes_for_mask(conn(p).boxes, 0)
    }

    fn collision_boxes(
        &self,
        p: &ShapeParams,
        nb: &dyn ShapeNeighborhood,
        pos: IVec3,
        _b: Block,
    ) -> &'static [Aabb] {
        connection_boxes(nb, pos, conn(p), ShapeFamily::Pane)
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
            ShapeFamily::Pane,
        ))
        .to_cell()
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
        // The mask-free POST: rails are thin and corner-distant, and a mask
        // read would make the answer differ between the two meshers.
        let c = conn(p);
        lo[0] < c.post_hi && hi[0] > c.post_lo && lo[2] < c.post_hi && hi[2] > c.post_lo
    }
}

impl ShapeRender for PaneFamily {
    fn boxes(&self, ctx: &ShapeCtx<'_>, out: &mut Vec<ShapeBox>) {
        // [top, bottom, side] tiles = [edge, edge, glass].
        let [edge_tile, _bottom, glass_tile] = ctx.block.tiles();
        let c = conn(ctx.params);
        let mask = crate::connect::ConnectionMask::from_cell(ctx.nb.shape_state(ctx.pos)).0;
        crate::shape_mesh::pane::push_mesh_boxes(
            out,
            c.post_lo,
            c.post_hi,
            mask,
            glass_tile,
            edge_tile,
            (ctx.tint_for)(glass_tile),
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
        union(connection_boxes(nb, pos, conn(p), ShapeFamily::Pane))
    }
    fn item_render(&self, p: &ShapeParams, block: Block) -> ItemRender {
        item_from_form(conn(p).item_form, block)
    }
}

impl ShapePlacement for PaneFamily {
    fn placement_plan(
        &self,
        w: &WorldData,
        block: Block,
        inputs: &PlaceInputs,
        occupied: &mut dyn FnMut(IVec3, &[Aabb]) -> bool,
    ) -> PlacementOutcome {
        connection_placement(w, block, inputs.place_pos, ShapeFamily::Pane, occupied)
    }
}
