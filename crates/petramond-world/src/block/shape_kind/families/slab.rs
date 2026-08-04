//! The half-cell slab; state stores the split axis plus up to two layers.
//!
//! Sim, render, and placement for this family live together here; the shared
//! seam helpers and the singleton table stay in the parent.

use super::*;

/// A half-cell slab; state stores split axis + up to two layers.
pub struct SlabFamily;

impl ShapeSim for SlabFamily {
    fn default_boxes(&self, _p: &ShapeParams, _b: Block) -> &'static [Aabb] {
        crate::slab::default_boxes()
    }

    fn collision_boxes(
        &self,
        _p: &ShapeParams,
        nb: &dyn ShapeNeighborhood,
        pos: IVec3,
        b: Block,
    ) -> &'static [Aabb] {
        crate::slab::boxes_for_state(crate::slab::normalize_state(b, slab_state_at(nb, pos)))
    }

    fn full_face(
        &self,
        _p: &ShapeParams,
        nb: &dyn ShapeNeighborhood,
        pos: IVec3,
        b: Block,
        dir: IVec3,
    ) -> Option<crate::block::shape_kind::facets::FullFace> {
        let state = crate::slab::normalize_state(b, slab_state_at(nb, pos));
        crate::slab::face_full(state, dir)
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
        b: Block,
        lo: [f32; 3],
        hi: [f32; 3],
    ) -> bool {
        let state = crate::slab::normalize_state(b, slab_state_at(nb, pos));
        state.is_full()
            || any_octant(lo, hi, &|ix, iy, iz| {
                crate::slab::half_cell_occupied(state, ix, iy, iz)
            })
    }

    /// A slab cell is composed of its filled layer slots, and the slot INDEX
    /// is the part number — the same numbering `boxes` tags its boxes with and
    /// the placement plan claims, so "the layer this click filled" and "the
    /// layer this drop came from" address the same cell KV.
    fn parts(
        &self,
        _p: &ShapeParams,
        nb: &dyn ShapeNeighborhood,
        pos: IVec3,
        b: Block,
    ) -> Option<Vec<(crate::block::CellPart, Block)>> {
        let state = crate::slab::normalize_state(b, slab_state_at(nb, pos));
        Some(
            crate::slab::layer_slots(state)
                .map(|(slot, block)| (slot.index as crate::block::CellPart, block))
                .collect(),
        )
    }
}

impl ShapeRender for SlabFamily {
    fn item_boxes(
        &self,
        _p: &ShapeParams,
        b: Block,
        state: crate::block_state::HeldBlockState,
        out: &mut Vec<crate::block::ItemBox>,
    ) {
        let held = match state {
            crate::block_state::HeldBlockState::Slab(s) => crate::slab::normalize_state(b, s),
            _ => crate::slab::default_state(b),
        };
        // Each occupied layer draws in its OWN material — a stacked two-tone
        // slab's item shows both.
        for (slot, block) in crate::slab::layer_slots(held) {
            let (min, max) = crate::shape_mesh::slab::slot_box(slot);
            let mut item = crate::block::ItemBox::solid(min, max);
            item.material = Some(block);
            out.push(item);
        }
    }

    fn meshes_as_cube(&self, ctx: &ShapeCtx<'_>) -> bool {
        // A same-material full stack IS the material's full cube: it falls to
        // the cube path so it culls, lights, and GREEDY-MERGES like one (the
        // merge is load-bearing for streaming perf). A mixed-material stack
        // keeps the per-layer boxes so each layer shows its own texture.
        let state = crate::slab::normalize_state(ctx.block, slab_state_at(ctx.nb, ctx.pos));
        if !crate::slab::is_uniform_full_stack(state) {
            return false;
        }
        // Same material, but the two layers may be DYED differently — then the
        // cell is not one cube at all and the per-layer boxes have to draw it
        // (the cube path has a single whole-cell tint). Only the family knows
        // it has exactly these two parts to compare.
        (ctx.part_tint)(0) == (ctx.part_tint)(1)
    }

    fn boxes(&self, ctx: &ShapeCtx<'_>, out: &mut Vec<ShapeBox>) {
        let state = crate::slab::normalize_state(ctx.block, slab_state_at(ctx.nb, ctx.pos));
        for (slot, layer_block) in crate::slab::layer_slots(state) {
            let (min, max) = crate::shape_mesh::slab::slot_box(slot);
            out.push(
                ShapeBox::uniform(Aabb { min, max }, layer_block.tiles(), ctx.tint_for)
                    .with_part(slot.index as crate::block::CellPart),
            );
        }
    }

    fn picks_by_boxes(&self, _p: &ShapeParams) -> bool {
        true
    }
    fn selection_box(
        &self,
        _p: &ShapeParams,
        nb: &dyn ShapeNeighborhood,
        pos: IVec3,
        b: Block,
    ) -> Option<([f32; 3], [f32; 3])> {
        crate::slab::visual_aabb(crate::slab::normalize_state(b, slab_state_at(nb, pos)))
    }
    fn item_render(&self, _p: &ShapeParams, block: Block) -> ItemRender {
        ItemRender::BlockForm(block)
    }
}

impl ShapePlacement for SlabFamily {
    fn placement_plan(
        &self,
        w: &WorldData,
        block: Block,
        inputs: &PlaceInputs,
        occupied: &mut dyn FnMut(IVec3, &[Aabb]) -> bool,
    ) -> PlacementOutcome {
        // A stack lands in the CLICKED cell when the clicked face fronts the
        // half it would fill; otherwise a fresh layer builds into the
        // adjacent cell.
        let rotation = inputs.held_rotation.slab_rotation(inputs.held);
        let (target, slot) = match w.slab_stack_slot_in_hit(
            block,
            inputs.hit,
            rotation,
            inputs.normal,
            inputs.player_facing,
        ) {
            Some(slot) => (inputs.hit, slot),
            None => (
                inputs.place_pos,
                crate::slab::slot_for_rotation(rotation, inputs.normal, inputs.player_facing),
            ),
        };
        let target_block = Block::from_id(w.chunk_block(target.x, target.y, target.z));
        if !crate::slab::is_slab(target_block) && !w.placement_cell_open(target) {
            return PlacementOutcome::Refused;
        }
        let Some(next) = w.slab_layer_target_state(target, block, slot) else {
            return PlacementOutcome::Refused;
        };
        if occupied(target, crate::slab::boxes_for_state(next)) {
            return PlacementOutcome::Refused;
        }
        // The resulting stack is the write: representative block id + the
        // full layer state, whether this creates the cell or fills a half.
        // The write claims the slot it filled, so its carried data lands on
        // that layer; stacking into a cell that already holds a layer AUGMENTS
        // it, keeping the sitting layer's colour instead of handing it the
        // newcomer's.
        PlacementOutcome::Plan(PlacementPlan::single_part(
            target,
            crate::slab::representative_block(next),
            next.to_cell(),
            slot.index as crate::block::CellPart,
            crate::slab::is_slab(target_block),
        ))
    }
}
