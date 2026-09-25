//! The trapdoor: a thin panel lying flat across a cell, swinging up onto the
//! edge it is hinged to.
//!
//! Sim, render, and placement for this family live together here; the shared
//! seam helpers and the singleton table stay in the parent.

use super::*;

/// A trapdoor; a thin panel across a cell with per-cell facing/open/half state.
pub struct TrapdoorFamily;

impl ShapeSim for TrapdoorFamily {
    fn rotate_y(&self, block: Block, state: ShapeState) -> crate::block::rotation::CellRotation {
        let mut panel = crate::trapdoor::TrapdoorState::from_cell(state);
        panel.facing = crate::block::rotation::facing(panel.facing);
        crate::block::rotation::CellRotation::unchanged(block, panel.to_cell())
    }

    fn collision_boxes(
        &self,
        _p: &ShapeParams,
        nb: &dyn ShapeNeighborhood,
        pos: IVec3,
        b: Block,
    ) -> &'static [Aabb] {
        match trapdoor_state_at(nb, pos) {
            Some(state) => crate::trapdoor::collision_boxes(state),
            None => b.collision_boxes(),
        }
    }
}

impl ShapeRender for TrapdoorFamily {
    fn selection_box(
        &self,
        _p: &ShapeParams,
        nb: &dyn ShapeNeighborhood,
        pos: IVec3,
        b: Block,
    ) -> Option<([f32; 3], [f32; 3])> {
        match trapdoor_state_at(nb, pos) {
            Some(state) => Some(crate::trapdoor::selection_aabb(state)),
            None => b.visual_aabb(),
        }
    }
    fn item_render(&self, _p: &ShapeParams, _block: Block) -> ItemRender {
        ItemRender::ItemSprite
    }
}

impl ShapePlacement for TrapdoorFamily {
    fn authored_state(&self, _block: Block, state: ShapeState) -> ShapeState {
        crate::trapdoor::TrapdoorState {
            open: false,
            ..crate::trapdoor::TrapdoorState::from_cell(state)
        }
        .to_cell()
    }

    fn authored_plan(
        &self,
        block: Block,
        inputs: &mut crate::world::placement::authored::Inputs<'_>,
    ) -> Result<PlacementPlan, String> {
        let top = match inputs.property("half", "bottom") {
            "bottom" => false,
            "top" => true,
            value => return Err(format!("unknown trapdoor half '{value}'")),
        };
        let open = match inputs.property("open", "false") {
            "true" => true,
            "false" => false,
            value => return Err(format!("unknown trapdoor open value '{value}'")),
        };
        let facing = inputs.facing()?;
        Ok(PlacementPlan::single(
            inputs.anchor,
            block,
            crate::trapdoor::TrapdoorState { facing, open, top }.to_cell(),
        ))
    }

    fn placement_plan(
        &self,
        w: &WorldData,
        block: Block,
        inputs: &PlaceInputs,
        occupied: &mut dyn FnMut(IVec3, &[Aabb]) -> bool,
    ) -> PlacementOutcome {
        // A trapdoor is hung ON the thing it was clicked against: it hinges on
        // the edge facing that block, and a click on a WALL puts the panel in
        // the half of the cell the click landed in (upper half → hung from the
        // ceiling). A click on a horizontal face has no wall to hinge on, so
        // the placer's own facing picks the edge and the face picks the half —
        // landing on a block's top lays the panel on the floor, landing under
        // one hangs it from the ceiling.
        let p = inputs.place_pos;
        let state = match inputs.support_side() {
            Some(side) => crate::trapdoor::TrapdoorState {
                facing: side,
                open: false,
                top: inputs.spot[1] > 0.5,
            },
            None => crate::trapdoor::TrapdoorState {
                facing: inputs.player_facing,
                open: false,
                top: inputs.normal.y < 0,
            },
        };
        if !w.placement_cell_open(p) || occupied(p, crate::trapdoor::collision_boxes(state)) {
            return PlacementOutcome::Refused;
        }
        PlacementOutcome::Plan(PlacementPlan::single(p, block, state.to_cell()))
    }
}
