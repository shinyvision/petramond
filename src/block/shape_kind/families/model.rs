//! The bbmodel block: geometry and collision baked from the model, per-cell oriented.
//!
//! Sim, render, and placement for this family live together here; the shared
//! seam helpers and the singleton table stay in the parent.

use super::*;

/// A bbmodel block; geometry/collision baked from the model, oriented per cell.
pub struct ModelFamily;

impl ShapeSim for ModelFamily {
    fn collision_boxes(
        &self,
        p: &ShapeParams,
        nb: &dyn ShapeNeighborhood,
        pos: IVec3,
        _b: Block,
    ) -> &'static [Aabb] {
        let kind = p.model_kind().expect("model family carries a model kind");
        let st = model_state_at(nb, pos);
        crate::block_model::collision_boxes_oriented(kind, st.offset, st.facing)
    }
}

impl ShapeRender for ModelFamily {
    fn selection_box(
        &self,
        p: &ShapeParams,
        nb: &dyn ShapeNeighborhood,
        pos: IVec3,
        _b: Block,
    ) -> Option<([f32; 3], [f32; 3])> {
        let kind = p.model_kind().expect("model family carries a model kind");
        let st = model_state_at(nb, pos);
        crate::block_model::selection_aabb_oriented(kind, st.offset, st.facing)
    }

    fn default_selection_box(
        &self,
        p: &ShapeParams,
        _block: Block,
    ) -> Option<([f32; 3], [f32; 3])> {
        // The MODEL's box, independent of collision — a walk-through model
        // block is still selectable. Position-less: the footprint-origin cell.
        let kind = p.model_kind().expect("model family carries a model kind");
        crate::block_model::selection_aabb(kind, [0, 0, 0])
    }

    fn item_render(&self, p: &ShapeParams, _block: Block) -> ItemRender {
        ItemRender::Model(p.model_kind().expect("model family carries a model kind"))
    }
}

impl ShapePlacement for ModelFamily {
    fn placement_plan(
        &self,
        w: &World,
        block: Block,
        inputs: &PlaceInputs,
        occupied: &mut dyn FnMut(IVec3, &[Aabb]) -> bool,
    ) -> PlacementOutcome {
        // A bbmodel block places its WHOLE footprint: every occupied cell must
        // be loaded + replaceable AND clear of blocking bodies, or the
        // placement fails as a unit. Multi-cell / directionalView models orient
        // from the player's facing; a `centered` model centres its footprint on
        // the clicked cell with the default facing.
        let p = inputs.place_pos;
        let kind = block
            .model_kind()
            .expect("model family carries a model kind");
        let centered = matches!(
            crate::block_model::def(kind).orientation,
            crate::block_model::PlacementOrientation::Centered
        );
        let oriented = !centered
            && (block.directional_view() || crate::block_model::instance(kind).cells.len() > 1);
        let facing = if oriented {
            crate::block_model::def(kind)
                .orientation
                .apply(inputs.player_facing)
        } else {
            crate::block_model::DEFAULT_MODEL_FACING
        };
        let base = if centered {
            crate::block_model::base_from_centered_anchor(p, kind)
        } else if oriented {
            crate::block_model::base_from_front_left_anchor(p, kind, facing)
        } else {
            p
        };
        if !w.model_footprint_clear_facing(base, kind, facing) {
            return PlacementOutcome::Refused;
        }
        let footprint = crate::block_model::oriented_footprint_cells(base, kind, facing);
        if footprint.iter().any(|&(c, off)| {
            occupied(
                c,
                crate::block_model::collision_boxes_oriented(kind, off, facing),
            )
        }) {
            return PlacementOutcome::Refused;
        }
        PlacementOutcome::Plan(PlacementPlan {
            anchor: base,
            writes: footprint
                .into_iter()
                .map(|(c, off)| {
                    PlacementPlan::whole(
                        c,
                        block,
                        crate::block_model::ModelCellState {
                            offset: off,
                            facing,
                        }
                        .to_cell(),
                    )
                })
                .collect(),
        })
    }
}
