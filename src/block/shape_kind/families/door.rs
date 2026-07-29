//! The wooden door: a thin slab on a cell edge with per-cell facing/open/half state.
//!
//! Sim, render, and placement for this family live together here; the shared
//! seam helpers and the singleton table stay in the parent.

use super::*;

/// A wooden door; a thin slab on a cell edge, per-cell facing/open/half state.
pub struct DoorFamily;

impl ShapeSim for DoorFamily {
    fn collision_boxes(
        &self,
        _p: &ShapeParams,
        nb: &dyn ShapeNeighborhood,
        pos: IVec3,
        b: Block,
    ) -> &'static [Aabb] {
        match door_state_at(nb, pos) {
            Some(state) => crate::door::collision_boxes(state),
            None => b.collision_boxes(),
        }
    }
}

impl ShapeRender for DoorFamily {
    fn selection_box(
        &self,
        _p: &ShapeParams,
        nb: &dyn ShapeNeighborhood,
        pos: IVec3,
        b: Block,
    ) -> Option<([f32; 3], [f32; 3])> {
        match door_state_at(nb, pos) {
            Some(state) => Some(crate::door::selection_aabb(state)),
            None => b.visual_aabb(),
        }
    }
    fn item_render(&self, _p: &ShapeParams, _block: Block) -> ItemRender {
        ItemRender::ItemSprite
    }
}

impl ShapePlacement for DoorFamily {
    fn placement_plan(
        &self,
        w: &World,
        block: Block,
        inputs: &PlaceInputs,
        occupied: &mut dyn FnMut(IVec3, &[Aabb]) -> bool,
    ) -> PlacementOutcome {
        // A door is a 2-tall thin block: both cells must be loaded +
        // replaceable with a floor to stand on, and the closed slab must not
        // trap a body. It sits on the edge nearest the placer.
        let p = inputs.place_pos;
        if !w.door_footprint_clear(p) {
            return PlacementOutcome::Refused;
        }
        let upper = p + IVec3::new(0, 1, 0);
        let closed = |top: bool| {
            crate::door::collision_boxes(crate::door::DoorState {
                facing: inputs.player_facing,
                open: false,
                top,
            })
        };
        if occupied(p, closed(false)) || occupied(upper, closed(true)) {
            return PlacementOutcome::Refused;
        }
        let half = |top| {
            crate::door::DoorState::to_cell(&crate::door::DoorState {
                facing: inputs.player_facing,
                open: false,
                top,
            })
        };
        PlacementOutcome::Plan(PlacementPlan {
            anchor: p,
            writes: vec![
                PlacementPlan::whole(p, block, half(false)),
                PlacementPlan::whole(upper, block, half(true)),
            ],
        })
    }
}
