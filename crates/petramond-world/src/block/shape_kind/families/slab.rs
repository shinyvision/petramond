use super::*;

pub struct SlabFamily;

impl ShapeSim for SlabFamily {
    fn accepts_row_uv_rotation(&self) -> bool {
        true
    }

    fn row_flags(&self) -> crate::block::BlockFlags {
        crate::block::BlockFlags::SLAB
    }

    fn rotate_y(&self, block: Block, state: ShapeState) -> crate::block::rotation::CellRotation {
        let mut slab = SlabState::from_cell(state);
        let swap = slab.split == crate::block_state::SlabSplit::Z;
        slab.split = match slab.split {
            crate::block_state::SlabSplit::X => crate::block_state::SlabSplit::Z,
            crate::block_state::SlabSplit::Z => crate::block_state::SlabSplit::X,
            other => other,
        };
        if swap {
            slab.layers.swap(0, 1);
        }
        crate::block::rotation::CellRotation {
            block,
            state: slab.to_cell(),
            swap_parts: swap,
        }
    }

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

    fn shade_boxes(
        &self,
        _p: &ShapeParams,
        nb: &dyn ShapeNeighborhood,
        pos: IVec3,
        b: Block,
        out: &mut Vec<Aabb>,
    ) -> bool {
        let state = crate::slab::normalize_state(b, slab_state_at(nb, pos));
        if state.is_full() {
            out.push(Aabb {
                min: [-OPEN_ENDED; 3],
                max: [OPEN_ENDED; 3],
            });
        } else {
            push_octants(out, |ix, iy, iz| {
                crate::slab::half_cell_occupied(state, ix, iy, iz)
            });
        }
        true
    }

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

    fn keeping_parts(
        &self,
        _p: &ShapeParams,
        b: Block,
        state: ShapeState,
        keep: &[crate::block::CellPart],
    ) -> Option<(Block, ShapeState)> {
        let whole = crate::slab::normalize_state(b, SlabState::from_cell(state));
        let kept = crate::slab::layer_slots(whole)
            .filter(|(slot, _)| keep.contains(&(slot.index as crate::block::CellPart)))
            .try_fold(SlabState::default(), |kept, (slot, block)| {
                crate::slab::add_layer(kept, slot, block)
            })?;
        (!kept.is_empty()).then(|| (crate::slab::representative_block(kept), kept.to_cell()))
    }
}

impl ShapeRender for SlabFamily {
    fn mesh_emitter(&self, _p: &ShapeParams) -> MeshEmitter {
        MeshEmitter::Boxes
    }

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
        for (slot, layer_block) in crate::slab::layer_slots(held) {
            let (min, max) = crate::shape_mesh::slab::slot_box(slot);
            let mut item = crate::block::ItemBox::solid(min, max);
            item.material = Some(layer_block);
            let turns = layer_block.uv_turns();
            item.uv_turns[2] = turns[0];
            item.uv_turns[3] = turns[1];
            out.push(item);
        }
    }

    fn meshes_as_cube(&self, ctx: &ShapeCtx<'_>) -> bool {
        let state = crate::slab::normalize_state(ctx.block, slab_state_at(ctx.nb, ctx.pos));
        if !crate::slab::is_uniform_full_stack(state) {
            return false;
        }
        (ctx.part_tint)(0) == (ctx.part_tint)(1)
    }

    fn boxes(&self, ctx: &ShapeCtx<'_>, out: &mut Vec<ShapeBox>) {
        let state = crate::slab::normalize_state(ctx.block, slab_state_at(ctx.nb, ctx.pos));
        for (slot, layer_block) in crate::slab::layer_slots(state) {
            let (min, max) = crate::shape_mesh::slab::slot_box(slot);
            out.push(
                ShapeBox::uniform(Aabb { min, max }, layer_block.tiles(), ctx.tint_for)
                    .with_slot_uv_turns(layer_block.uv_turns())
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
    fn held_rotations(&self) -> u8 {
        3
    }

    fn held_state(
        &self,
        block: Block,
        rotation: &crate::world::placement::HeldRotation,
        selected: Option<crate::item::ItemType>,
    ) -> Option<crate::block_state::HeldBlockState> {
        let slot = crate::slab::slot_for_rotation(
            rotation.slab_rotation(selected),
            IVec3::ZERO,
            Facing::South,
        );
        Some(crate::block_state::HeldBlockState::Slab(
            crate::block_state::SlabState::single(slot.split, slot.index, block),
        ))
    }

    fn authored_plan(
        &self,
        block: Block,
        inputs: &mut crate::world::placement::authored::Inputs<'_>,
    ) -> Result<PlacementPlan, String> {
        use crate::block_state::SlabSplit;
        let normal = match inputs.property("half", "bottom") {
            "bottom" => -IVec3::Y,
            "top" => IVec3::Y,
            "north" => -IVec3::Z,
            "south" => IVec3::Z,
            "west" => -IVec3::X,
            "east" => IVec3::X,
            value => return Err(format!("unknown slab half '{value}'")),
        };
        let normal = inputs.turn.apply(normal);
        let (split, lane) = if normal.x != 0 {
            (SlabSplit::X, normal.x)
        } else if normal.y != 0 {
            (SlabSplit::Y, normal.y)
        } else {
            (SlabSplit::Z, normal.z)
        };
        Ok(PlacementPlan::single(
            inputs.anchor,
            block,
            SlabState::single(split, usize::from(lane > 0), block).to_cell(),
        ))
    }

    fn placement_plan(
        &self,
        w: &WorldData,
        block: Block,
        inputs: &PlaceInputs,
        occupied: &mut dyn FnMut(IVec3, &[Aabb]) -> bool,
    ) -> PlacementOutcome {
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
        PlacementOutcome::Plan(PlacementPlan::single_part(
            target,
            crate::slab::representative_block(next),
            next.to_cell(),
            slot.index as crate::block::CellPart,
            crate::slab::is_slab(target_block),
        ))
    }
}
