//! The climbable wall panel; its facing is block identity, not cell state.
//!
//! Sim, render, and placement for this family live together here; the shared
//! seam helpers and the singleton table stay in the parent.

use super::*;

/// A climbable wall panel (the ladder); facing is block identity.
pub struct LadderFamily;

impl ShapeSim for LadderFamily {
    fn mount(
        &self,
        _p: &ShapeParams,
        _nb: &dyn ShapeNeighborhood,
        pos: IVec3,
        b: Block,
    ) -> Option<ShapeMount> {
        // A panel grips the wall its declared facing points away from.
        let facing = b.declared_panel_facing()?;
        Some(ShapeMount {
            cell: crate::ladder::support_cell(pos, facing),
            normal: facing.dir(),
        })
    }

    fn collision_boxes(
        &self,
        _p: &ShapeParams,
        _nb: &dyn ShapeNeighborhood,
        _pos: IVec3,
        b: Block,
    ) -> &'static [Aabb] {
        let (t, h) = b.ladder_dims();
        crate::ladder::collision_boxes_dim(b.panel_facing(), t, h)
    }

    fn occupies_pocket(
        &self,
        _p: &ShapeParams,
        _nb: &dyn ShapeNeighborhood,
        _pos: IVec3,
        b: Block,
        lo: [f32; 3],
        hi: [f32; 3],
    ) -> bool {
        let (thickness, height) = b.ladder_dims();
        let (mn, mx) = crate::ladder::panel_aabb_dim(b.panel_facing(), thickness, height);
        overlaps(lo, hi, mn, mx)
    }
}

impl ShapeRender for LadderFamily {
    /// A thin panel against a wall: aiming must meet the panel, not the cell.
    fn precise_pick(&self, _p: &ShapeParams) -> bool {
        true
    }

    fn boxes(&self, ctx: &ShapeCtx<'_>, out: &mut Vec<ShapeBox>) {
        let tile = ctx.block.tiles()[0];
        let (thickness, height) = ctx.block.ladder_dims();
        crate::shape_mesh::ladder::push_mesh_box(
            out,
            ctx.block.panel_facing(),
            thickness,
            height,
            tile,
            (ctx.tint_for)(tile),
        );
    }

    fn selection_box(
        &self,
        _p: &ShapeParams,
        _nb: &dyn ShapeNeighborhood,
        _pos: IVec3,
        b: Block,
    ) -> Option<([f32; 3], [f32; 3])> {
        let (t, h) = b.ladder_dims();
        Some(crate::ladder::panel_aabb_dim(b.panel_facing(), t, h))
    }
    fn item_render(&self, _p: &ShapeParams, _block: Block) -> ItemRender {
        ItemRender::ItemSprite
    }
}

impl ShapePlacement for LadderFamily {
    fn placement_plan(
        &self,
        w: &WorldData,
        block: Block,
        inputs: &PlaceInputs,
        occupied: &mut dyn FnMut(IVec3, &[Aabb]) -> bool,
    ) -> PlacementOutcome {
        // A ladder-shaped block mounts on a vertical wall face with a complete
        // face behind its panel; the clicked normal names the panel front. The
        // panel is real collision, so the shared tail's body gate applies.
        let p = inputs.place_pos;
        let Some(facing) = Facing::from_horizontal_normal(inputs.normal) else {
            return PlacementOutcome::Refused;
        };
        if !w.ladder_supported_at(p, facing) {
            return PlacementOutcome::Refused;
        }
        let (t, h) = block.ladder_dims();
        let boxes = crate::ladder::collision_boxes_dim(facing, t, h);
        // The facing IS the block row (the sapling-stage pattern): the plan
        // writes the sibling row whose panel fronts the clicked normal — no
        // per-cell state, and no engine write vocabulary.
        let row = block.wall_panel_row(facing);
        match w.finish_single_cell_placement(row, p, ShapeState::NONE, boxes, occupied) {
            Some(plan) => PlacementOutcome::Plan(plan),
            None => PlacementOutcome::Refused,
        }
    }
}
