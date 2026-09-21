//! The bbmodel block: geometry and collision baked from the model, per-cell oriented.
//!
//! Sim, render, and placement for this family live together here; the shared
//! seam helpers and the singleton table stay in the parent.

use super::*;

/// A bbmodel block; geometry/collision baked from the model, oriented per cell.
pub struct ModelFamily;

impl ShapeSim for ModelFamily {
    fn rotate_y(&self, block: Block, state: ShapeState) -> crate::block::rotation::CellRotation {
        let mut model = crate::block_model::ModelCellState::from_cell(state);
        model.facing = crate::block::rotation::facing(model.facing);
        crate::block::rotation::CellRotation::unchanged(block, model.to_cell())
    }

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

/// Whether a placement of this model takes its facing from the placer: a
/// centred model and a lone undirected cell always stand the default way.
fn turns_with_the_placer(block: Block, kind: crate::block_model::BlockModelKind) -> bool {
    !matches!(
        crate::block_model::def(kind).orientation,
        crate::block_model::PlacementOrientation::Centered
    ) && (block.directional_view() || crate::block_model::instance(kind).cells.len() > 1)
}

/// The pose construction builds a recorded model in. One that turns with its
/// placer is built as recorded. One that never turns was turned by something
/// other than a click (a design laid down rotated): it is built the default
/// way over the same cells where they allow it, which is all a click can do.
fn clickable_pose(
    block: Block,
    kind: crate::block_model::BlockModelKind,
    base: IVec3,
    facing: Facing,
) -> (IVec3, Facing) {
    let default = crate::block_model::DEFAULT_MODEL_FACING;
    if facing == default || turns_with_the_placer(block, kind) {
        return (base, facing);
    }
    let cells = |base, facing| {
        let mut cells: Vec<IVec3> =
            crate::block_model::oriented_footprint_cells(base, kind, facing)
                .into_iter()
                .map(|(cell, _)| cell)
                .collect();
        cells.sort_by_key(|c| (c.x, c.y, c.z));
        cells
    };
    let recorded = cells(base, facing);
    let corner = recorded
        .iter()
        .fold(IVec3::MAX, |corner, &cell| corner.min(cell));
    let upright = crate::block_model::base_from_cell(corner, kind, [0, 0, 0], default);
    if cells(upright, default) == recorded {
        (upright, default)
    } else {
        (base, facing)
    }
}

impl ShapePlacement for ModelFamily {
    fn construction_writes(
        &self,
        block: Block,
        state: ShapeState,
        pos: IVec3,
    ) -> crate::world::placement::ConstructionWrites {
        let kind = block
            .model_kind()
            .expect("model family carries a model kind");
        let cell = crate::block_model::ModelCellState::from_cell(state);
        let (base, facing) = clickable_pose(
            block,
            kind,
            crate::block_model::base_from_cell(pos, kind, cell.offset, cell.facing),
            cell.facing,
        );
        if base != pos {
            return crate::world::placement::ConstructionWrites::Member(base);
        }
        crate::world::placement::ConstructionWrites::Anchor(PlacementPlan {
            anchor: base,
            writes: crate::block_model::oriented_footprint_cells(base, kind, facing)
                .into_iter()
                .map(|(c, offset)| {
                    PlacementPlan::whole(
                        c,
                        block,
                        crate::block_model::ModelCellState { offset, facing }.to_cell(),
                    )
                })
                .collect(),
        })
    }

    fn member_state(&self, block: Block, state: ShapeState, pos: IVec3) -> ShapeState {
        let kind = block
            .model_kind()
            .expect("model family carries a model kind");
        let cell = crate::block_model::ModelCellState::from_cell(state);
        let recorded = crate::block_model::base_from_cell(pos, kind, cell.offset, cell.facing);
        let (base, facing) = clickable_pose(block, kind, recorded, cell.facing);
        crate::block_model::oriented_footprint_cells(base, kind, facing)
            .into_iter()
            .find(|(c, _)| *c == pos)
            .map_or(state, |(_, offset)| {
                crate::block_model::ModelCellState { offset, facing }.to_cell()
            })
    }

    fn authored_plan(
        &self,
        block: Block,
        inputs: &mut crate::world::placement::authored::Inputs<'_>,
    ) -> Result<PlacementPlan, String> {
        let kind = block
            .model_kind()
            .expect("model family carries a model kind");
        let facing = inputs.facing()?;
        let base = crate::block_model::base_from_cell(inputs.anchor, kind, [0, 0, 0], facing);
        Ok(PlacementPlan {
            anchor: inputs.anchor,
            writes: crate::block_model::oriented_footprint_cells(base, kind, facing)
                .into_iter()
                .map(|(cell, offset)| {
                    PlacementPlan::whole(
                        cell,
                        block,
                        crate::block_model::ModelCellState { offset, facing }.to_cell(),
                    )
                })
                .collect(),
        })
    }

    fn placement_plan(
        &self,
        w: &WorldData,
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
        let oriented = turns_with_the_placer(block, kind);
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
        // The row's own substrate/support declaration (`support`, `roots_on`,
        // `roots_face`) gates a model exactly as it gates a single cell — at
        // the anchor, the cell the row's support direction is read from. A
        // rail declaring `roots_face: solid_face` must refuse open air here,
        // or the fragile update shatters it a tick later and eats the item.
        if !w.placement_support_ok(block, base) {
            return PlacementOutcome::Refused;
        }
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
