use crate::world::{World, WorldSide};
use petramond_world::block::{Aabb, Block, ShapeState};
pub use petramond_world::world::placement::*;

use petramond_math::math::IVec3;

use super::cell_change::{CellChange, ChangeKind};

impl<S: WorldSide> World<S> {
    pub fn placement_plan(
        &self,
        block: Block,
        inputs: &PlaceInputs,
        occupied: &mut dyn FnMut(IVec3, &[Aabb]) -> bool,
    ) -> Option<PlacementPlan> {
        match block
            .shape_kind_def()
            .placement
            .placement_plan(&self.data, block, inputs, occupied)
        {
            PlacementOutcome::Plan(plan) => Some(plan),
            PlacementOutcome::Refused => None,
            PlacementOutcome::General => self.general_placement_plan(block, inputs, occupied),
        }
    }

    pub fn general_placement_plan(
        &self,
        block: Block,
        inputs: &PlaceInputs,
        occupied: &mut dyn FnMut(IVec3, &[Aabb]) -> bool,
    ) -> Option<PlacementPlan> {
        let state = if block.is_axial() {
            let axis = inputs
                .held_rotation
                .log_axis_for_facing(inputs.held, inputs.player_facing);
            petramond_world::block::CellCodec::to_cell(&axis)
        } else if block.directional_view() {
            petramond_world::block::CellCodec::to_cell(&petramond_world::block_state::EntityFront(
                inputs.player_facing,
            ))
        } else {
            ShapeState::NONE
        };
        self.data.finish_single_cell_placement(
            block,
            inputs.place_pos,
            state,
            block.collision_boxes(),
            occupied,
        )
    }

    pub fn carry_into_cell(
        &mut self,
        stack: &petramond_world::item::ItemStack,
        block: Block,
        anchor: IVec3,
        part: petramond_world::block::CellPart,
    ) {
        let Some(map) = petramond_world::item::variant::get(stack.variant) else {
            return;
        };
        for &key in block.carry() {
            if let Some(v) = map.get(key) {
                self.cell_kv_set(
                    anchor.x,
                    anchor.y,
                    anchor.z,
                    petramond_world::block::part_kv_key(key, part),
                    v.clone(),
                );
            }
        }
    }

    /// Commit a validated plan's world write. One generic path for every family, same write on both
    /// sides, so a predicted ghost's mesh matches the authoritative delta that confirms it.
    /// Blocks + states land raw across the whole footprint first, region stays consistent before
    /// any announce. Then one region refresh relights, remeshes, announces, runs the refine
    /// cascade.
    /// `with_block_entities` is true on the authoritative world: a placed engine container
    /// fabricates its empty machine state right away. Client replica passes false;
    /// container/furnace machine state is server-owned and comes with the delta. That fabrication
    /// is block-ENTITY vocabulary (see `world::container`), not shape knowledge.
    pub fn commit_placement(&mut self, plan: &PlacementPlan, with_block_entities: bool) -> bool {
        for w in &plan.writes {
            if !self.materialize_section_at(w.cell) {
                return false;
            }
        }
        let mut changes = Vec::with_capacity(plan.writes.len());
        for &CellWrite {
            cell: c,
            block: b,
            state,
            augments,
            ..
        } in &plan.writes
        {
            let Some((section, lx, ly, lz)) = self.data.chunk_at_world_mut(c.x, c.y, c.z) else {
                return false;
            };
            let kept = augments.then(|| section.cell_kv_take(lx, ly, lz)).flatten();
            let old = section.block(lx, ly, lz);
            section.set_block(lx, ly, lz, b);
            if let Some(map) = kept {
                section.cell_kv_restore(lx, ly, lz, map);
            }
            if !state.is_empty() {
                section.set_cell_state(lx, ly, lz, state);
            }
            section.modified = true;
            changes.push(CellChange::new(c, old, ChangeKind::Place));
        }
        for &CellWrite {
            cell: c,
            block: b,
            state,
            ..
        } in &plan.writes
        {
            if with_block_entities {
                let facing =
                    <petramond_world::block_state::EntityFront as petramond_world::block::CellView>::from_cell(state).0;
                if b == Block::Furnace {
                    self.insert_furnace(c, facing);
                } else if b == Block::Chest {
                    self.insert_chest(c, facing);
                }
            }
        }
        self.apply_cell_changes(&changes);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn guest_plan(
        anchor: IVec3,
        cells: &[IVec3],
        block: Option<Block>,
    ) -> mod_api::ShapePlacementResult {
        mod_api::ShapePlacementResult {
            accepted: true,
            anchor: anchor.to_array(),
            cells: cells.iter().map(|c| c.to_array()).collect(),
            block: block.map(|b| mod_api::BlockId(b.id())),
        }
    }

    #[test]
    fn custom_plan_validation_gates_the_guests_answer() {
        let held = Block::Stone;
        let kind = held.shape_kind().0;
        assert_eq!(Block::Dirt.shape_kind().0, kind);
        assert_ne!(Block::Ladder.shape_kind().0, kind);
        let pp = IVec3::new(10, 64, -3);

        let (anchor, write) =
            validate_custom_plan(&guest_plan(pp, &[], None), held, kind, pp).expect("held write");
        assert_eq!((anchor, write), (pp, held));
        let (_, write) =
            validate_custom_plan(&guest_plan(pp, &[pp], Some(Block::Dirt)), held, kind, pp)
                .expect("sibling override");
        assert_eq!(write, Block::Dirt);
        assert!(
            validate_custom_plan(&guest_plan(pp, &[pp], Some(Block::Ladder)), held, kind, pp)
                .is_none()
        );
        let other = pp + IVec3::new(0, 1, 0);
        assert!(
            validate_custom_plan(&guest_plan(pp, &[pp, other], None), held, kind, pp).is_none()
        );
        assert!(validate_custom_plan(&guest_plan(pp, &[other], None), held, kind, pp).is_none());
        let near = pp + IVec3::new(2, -2, 0);
        assert!(validate_custom_plan(&guest_plan(near, &[near], None), held, kind, pp).is_some());
        let far = pp + IVec3::new(3, 0, 0);
        assert!(validate_custom_plan(&guest_plan(far, &[far], None), held, kind, pp).is_none());
    }
}
