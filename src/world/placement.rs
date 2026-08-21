//! The placement LADDER: how a use-click's plan is resolved against the live
//! world (slab stacking, general placement, support checks, finish paths).
//! The plan/outcome/trait VOCABULARY lives in `petramond_world::world::placement`.

use crate::world::World;
use petramond_world::block::{Aabb, Block, ShapeState};
pub use petramond_world::world::placement::*;

use petramond_math::math::IVec3;

impl World {
    /// The placement ladder: whether `block` can be placed at all for this
    /// click, and if so which state write lands where. `None` is a refused
    /// spot — the click neither places nor consumes the held item. `occupied`
    /// answers whether a gameplay body overlaps the given boxes at a cell;
    /// collisionless shapes (torch, plants) pass empty boxes and trap nothing.
    pub fn placement_plan(
        &self,
        block: Block,
        inputs: &PlaceInputs,
        occupied: &mut dyn FnMut(IVec3, &[Aabb]) -> bool,
    ) -> Option<PlacementPlan> {
        // The family owns its placement — the engine holds no per-family
        // placement dispatch. A family either resolves the plan, refuses, or
        // defers to the generic single-cell path.
        match block
            .shape_kind_def()
            .placement
            .placement_plan(self, block, inputs, occupied)
        {
            PlacementOutcome::Plan(plan) => Some(plan),
            PlacementOutcome::Refused => None,
            PlacementOutcome::General => self.general_placement_plan(block, inputs, occupied),
        }
    }

    /// The generic single-cell placement path: the write a plain cube / log /
    /// directional / plant block commits, gated by substrate, replaceability,
    /// and body occupancy. Every family that does not override its placement
    /// (and the torch / ladder families, which pre-gate then delegate here)
    /// reaches this.
    pub fn general_placement_plan(
        &self,
        block: Block,
        inputs: &PlaceInputs,
        occupied: &mut dyn FnMut(IVec3, &[Aabb]) -> bool,
    ) -> Option<PlacementPlan> {
        let state = if block.is_log() {
            // The log's own reading of the held rotation; the default
            // vertical axis stays stateless (the codec elides it).
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
        self.finish_single_cell_placement(
            block,
            inputs.place_pos,
            state,
            block.collision_boxes(),
            occupied,
        )
    }

    /// Whether the support at `s` presents the face shape `block`'s row
    /// Commit a validated plan's world write — ONE generic path for every
    /// family, the same write on both sides, which is what keeps a predicted
    /// ghost's mesh identical to the authoritative delta that confirms it.
    /// Blocks + states land raw across the whole footprint first (the region
    /// is consistent before any announce), then one region refresh relights,
    /// remeshes, announces, and runs the refine cascade.
    /// `with_block_entities` is true on the authoritative world (a placed
    /// engine container fabricates its empty machine state at once); the
    /// client replica passes false — container/furnace machine state is
    /// server-owned and arrives with the delta. That fabrication is
    /// block-ENTITY vocabulary (see `world::container`), not shape knowledge.
    pub fn commit_placement(&mut self, plan: &PlacementPlan, with_block_entities: bool) -> bool {
        for w in &plan.writes {
            if !self.materialize_section_at(w.cell) {
                return false;
            }
        }
        let mut cells = Vec::with_capacity(plan.writes.len());
        for &CellWrite {
            cell: c,
            block: b,
            state,
            augments,
            ..
        } in &plan.writes
        {
            let Some((section, lx, ly, lz)) = self.chunk_at_world_mut(c.x, c.y, c.z) else {
                return false;
            };
            // An AUGMENTING write adds a part to a cell that already holds
            // others, so their data has to survive `set_block`'s wholesale KV
            // clear (see `CellWrite::augments`). Detach + re-attach around the
            // write; the courier then overwrites just the claimed part's keys.
            let kept = augments.then(|| section.cell_kv_take(lx, ly, lz)).flatten();
            section.set_block(lx, ly, lz, b);
            if let Some(map) = kept {
                section.cell_kv_restore(lx, ly, lz, map);
            }
            if !state.is_empty() {
                section.set_cell_state(lx, ly, lz, state);
            }
            section.modified = true;
            cells.push(c);
        }
        for &CellWrite {
            cell: c,
            block: b,
            state,
            ..
        } in &plan.writes
        {
            // The WASM-bake fan-out + deep-visibility invalidation every
            // block write owes (the cube path had them via `set_block_world`;
            // the old per-family commits skipped them).
            self.mark_custom_bake_edit(c.x, c.y, c.z, b);
            if b.directional_view() {
                self.note_block_entity_change(c);
            }
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
        self.terrain.vis_dirty = true;
        self.refresh_region(&cells);
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
        // Two plain cubes share one shape-kind row; a ladder is another kind.
        let held = Block::Stone;
        let kind = held.shape_kind().0;
        assert_eq!(Block::Dirt.shape_kind().0, kind);
        assert_ne!(Block::Ladder.shape_kind().0, kind);
        let pp = IVec3::new(10, 64, -3);

        // The default writes the held row at the anchor; an empty `cells`
        // claims just the anchor.
        let (anchor, write) =
            validate_custom_plan(&guest_plan(pp, &[], None), held, kind, pp).expect("held write");
        assert_eq!((anchor, write), (pp, held));
        // A sibling row of the SAME shape kind is a legal orientation override.
        let (_, write) =
            validate_custom_plan(&guest_plan(pp, &[pp], Some(Block::Dirt)), held, kind, pp)
                .expect("sibling override");
        assert_eq!(write, Block::Dirt);
        // A row of a DIFFERENT shape kind can never be written — a plan may
        // not reach outside its own variant family.
        assert!(
            validate_custom_plan(&guest_plan(pp, &[pp], Some(Block::Ladder)), held, kind, pp)
                .is_none()
        );
        // A wider footprint than the anchor cell is refused, as is a `cells`
        // list naming a different cell.
        let other = pp + IVec3::new(0, 1, 0);
        assert!(
            validate_custom_plan(&guest_plan(pp, &[pp, other], None), held, kind, pp).is_none()
        );
        assert!(validate_custom_plan(&guest_plan(pp, &[other], None), held, kind, pp).is_none());
        // The anchor is bounded to the click's neighbourhood (Chebyshev ≤ 2).
        let near = pp + IVec3::new(2, -2, 0);
        assert!(validate_custom_plan(&guest_plan(near, &[near], None), held, kind, pp).is_some());
        let far = pp + IVec3::new(3, 0, 0);
        assert!(validate_custom_plan(&guest_plan(far, &[far], None), held, kind, pp).is_none());
    }
}
