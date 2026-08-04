//! The directional stair; its corner form resolves from neighbours and is STORED.
//!
//! Sim, render, and placement for this family live together here; the shared
//! seam helpers and the singleton table stay in the parent.

use super::*;

/// A directional stair; boxes resolve corner shape from neighbours.
pub struct StairFamily;

impl ShapeSim for StairFamily {
    fn default_boxes(&self, _p: &ShapeParams, _b: Block) -> &'static [Aabb] {
        crate::stair::boxes(crate::block_model::DEFAULT_MODEL_FACING)
    }

    fn collision_boxes(
        &self,
        _p: &ShapeParams,
        nb: &dyn ShapeNeighborhood,
        pos: IVec3,
        _b: Block,
    ) -> &'static [Aabb] {
        crate::stair::boxes_for_shape(stair_shape_at(nb, pos))
    }

    fn refine_state(
        &self,
        _p: &ShapeParams,
        nb: &dyn ShapeNeighborhood,
        pos: IVec3,
        _b: Block,
        state: ShapeState,
    ) -> ShapeState {
        // Byte 0 (the PLACED facing + half) is identity and never refined;
        // byte 1 is the corner shape joined against the neighbour stairs'
        // placed bits. Corner resolution reads neighbours' PLACED state only,
        // so stair refinement can never cascade through other stairs.
        let placed = StairState::from_cell(state);
        let shape = crate::stair::resolved_shape(pos, placed, |q| stair_state_at(nb, q));
        ShapeState::new(&[placed.encode(), shape.mask])
    }

    fn full_face(
        &self,
        _p: &ShapeParams,
        nb: &dyn ShapeNeighborhood,
        pos: IVec3,
        _b: Block,
        dir: IVec3,
    ) -> Option<crate::block::shape_kind::facets::FullFace> {
        crate::stair::face_full(
            crate::stair::StairShape::from_cell(nb.shape_state(pos)),
            dir,
        )
        .then_some(crate::block::shape_kind::facets::FullFace::Shaped)
    }

    fn light_shape(&self, _p: &ShapeParams, _b: Block) -> crate::block::BlockLightShape {
        crate::block::BlockLightShape::Shaped
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
        // The REFINED shape — the same stored corner byte `boxes` draws.
        // Occupancy must track the geometry, not the placement: two
        // placements refining to one corner shape must shade (and light)
        // neighbours identically.
        let shape = stair_shape_at(nb, pos);
        any_octant(lo, hi, &|ix, iy, iz| {
            crate::stair::shape_half_cell_occupied(shape, ix, iy, iz)
        })
    }
}

impl ShapeRender for StairFamily {
    fn item_boxes(
        &self,
        _p: &ShapeParams,
        _b: Block,
        state: crate::block_state::HeldBlockState,
        out: &mut Vec<crate::block::ItemBox>,
    ) {
        let held = match state {
            crate::block_state::HeldBlockState::Stair(s) => s,
            _ => StairState::new(crate::facing::Facing::South, Default::default()),
        };
        out.extend(
            crate::stair::boxes_for_shape(crate::stair::shape(held))
                .iter()
                .map(|b| crate::block::ItemBox::solid(b.min, b.max)),
        );
    }

    fn boxes(&self, ctx: &ShapeCtx<'_>, out: &mut Vec<ShapeBox>) {
        let tiles = ctx.block.tiles();
        let shape = stair_shape_at(ctx.nb, ctx.pos);
        out.extend(
            crate::stair::boxes_for_shape(shape)
                .iter()
                .map(|a| ShapeBox::uniform(*a, tiles, ctx.tint_for)),
        );
    }

    fn picks_by_boxes(&self, _p: &ShapeParams) -> bool {
        true
    }
    fn selection_box(
        &self,
        _p: &ShapeParams,
        _nb: &dyn ShapeNeighborhood,
        _pos: IVec3,
        _b: Block,
    ) -> Option<([f32; 3], [f32; 3])> {
        // A stair targets the whole cell (targeting is the whole cube).
        Some(([0.0, 0.0, 0.0], [1.0, 1.0, 1.0]))
    }
    fn item_render(&self, _p: &ShapeParams, block: Block) -> ItemRender {
        ItemRender::BlockForm(block)
    }
}

impl ShapePlacement for StairFamily {
    fn placement_plan(
        &self,
        w: &WorldData,
        block: Block,
        inputs: &PlaceInputs,
        occupied: &mut dyn FnMut(IVec3, &[Aabb]) -> bool,
    ) -> PlacementOutcome {
        let p = inputs.place_pos;
        let half = inputs.held_rotation.stair_half(inputs.held);
        let state = StairState::new(inputs.player_facing, half);
        // The boxes the stair WOULD have: its hypothetical own state (the cell
        // is still empty) corner-resolved against the placed neighbours,
        // through the same seam the placed shape will read.
        let boxes = crate::stair::resolved_boxes_state(p, state, |q| stair_state_at(w, q));
        if !w.placement_cell_open(p) || occupied(p, boxes) {
            return PlacementOutcome::Refused;
        }
        // The placed bits only; the refine cascade appends the corner byte.
        PlacementOutcome::Plan(PlacementPlan::single(p, block, state.to_cell()))
    }
}
