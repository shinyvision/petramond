//! The torch: a tilted pole with no collision, mounted from stored placement.
//!
//! Sim, render, and placement for this family live together here; the shared
//! seam helpers and the singleton table stay in the parent.

use super::*;

/// A torch: no collision (selectable by its pole in `player::interaction`); its
/// item is a flat sprite.
pub struct TorchFamily;

impl ShapeSim for TorchFamily {
    fn mount(
        &self,
        _p: &ShapeParams,
        nb: &dyn ShapeNeighborhood,
        pos: IVec3,
        _b: Block,
    ) -> Option<ShapeMount> {
        // The stored placement decides both: a floor torch grips the cell
        // below, a wall torch the wall behind it.
        let placement = TorchPlacement::from_cell(nb.shape_state(pos));
        Some(ShapeMount {
            cell: placement.support_cell(pos),
            normal: placement.support_normal(),
        })
    }
}

impl ShapeRender for TorchFamily {
    /// A tilted pole, not a box set: the ray must meet the actual geometry.
    fn precise_pick(&self, _p: &ShapeParams) -> bool {
        true
    }

    fn item_render(&self, _p: &ShapeParams, _block: Block) -> ItemRender {
        ItemRender::ItemSprite
    }
}

impl ShapePlacement for TorchFamily {
    fn placement_plan(
        &self,
        w: &WorldData,
        block: Block,
        inputs: &PlaceInputs,
        occupied: &mut dyn FnMut(IVec3, &[Aabb]) -> bool,
    ) -> PlacementOutcome {
        // A torch-shaped block mounts on a floor or wall (never a ceiling) and
        // needs a usable support face. Replacing a plant drops it to the FLOOR.
        // Then the shared single-cell tail applies (substrate/replaceable/body).
        let p = inputs.place_pos;
        let tp = if inputs.replacing_in_place {
            TorchPlacement::Floor
        } else {
            match TorchPlacement::from_place_normal(inputs.normal) {
                Some(tp) => tp,
                None => return PlacementOutcome::Refused,
            }
        };
        if !w.torch_supported_at(p, tp) {
            return PlacementOutcome::Refused;
        }
        let state = tp.to_cell();
        match w.finish_single_cell_placement(block, p, state, &[], occupied) {
            Some(plan) => PlacementOutcome::Plan(plan),
            None => PlacementOutcome::Refused,
        }
    }
}
