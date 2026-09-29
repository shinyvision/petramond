use super::*;

pub struct StairFamily;

impl ShapeSim for StairFamily {
    fn refines(&self, _p: &ShapeParams) -> bool {
        true
    }

    fn accepts_row_uv_rotation(&self) -> bool {
        true
    }

    fn rotate_y(&self, block: Block, state: ShapeState) -> crate::block::rotation::CellRotation {
        let placed = StairState::from_cell(state);
        let mut bytes = state.bytes().to_vec();
        if bytes.is_empty() {
            bytes.push(0);
        }
        bytes[0] = StairState {
            facing: crate::block::rotation::facing(placed.facing),
            ..placed
        }
        .encode();
        if bytes.len() > 1 {
            let m = bytes[1];
            bytes[1] = ((m & 1) << 1) | ((m & 2) << 2) | ((m & 8) >> 1) | ((m & 4) >> 2);
        }
        crate::block::rotation::CellRotation::unchanged(block, ShapeState::new(&bytes))
    }

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
        // Byte 0 is facing+half, fixed, never refined. Byte 1 is corner shape, joined against
        // neighbour stairs' placed bits.
        // We only look at neighbours' placed bits, not their refined shape. Otherwise refinement
        // cascades through stairs.
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
        let shape = stair_shape_at(nb, pos);
        any_octant(lo, hi, &|ix, iy, iz| {
            crate::stair::shape_half_cell_occupied(shape, ix, iy, iz)
        })
    }

    fn shade_boxes(
        &self,
        _p: &ShapeParams,
        nb: &dyn ShapeNeighborhood,
        pos: IVec3,
        _b: Block,
        out: &mut Vec<Aabb>,
    ) -> bool {
        let shape = stair_shape_at(nb, pos);
        push_octants(out, |ix, iy, iz| {
            crate::stair::shape_half_cell_occupied(shape, ix, iy, iz)
        });
        true
    }
}

impl ShapeRender for StairFamily {
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
            crate::block_state::HeldBlockState::Stair(s) => s,
            _ => StairState::new(crate::facing::Facing::South, Default::default()),
        };
        let turns = b.uv_turns();
        out.extend(
            crate::stair::boxes_for_shape(crate::stair::shape(held))
                .iter()
                .map(|b| {
                    let mut item = crate::block::ItemBox::solid(b.min, b.max);
                    item.uv_turns[2] = turns[0];
                    item.uv_turns[3] = turns[1];
                    item
                }),
        );
    }

    fn boxes(&self, ctx: &ShapeCtx<'_>, out: &mut Vec<ShapeBox>) {
        let tiles = ctx.block.tiles();
        let shape = stair_shape_at(ctx.nb, ctx.pos);
        out.extend(crate::stair::boxes_for_shape(shape).iter().map(|a| {
            ShapeBox::uniform(*a, tiles, ctx.tint_for).with_slot_uv_turns(ctx.block.uv_turns())
        }));
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
        Some(([0.0, 0.0, 0.0], [1.0, 1.0, 1.0]))
    }
    fn item_render(&self, _p: &ShapeParams, block: Block) -> ItemRender {
        ItemRender::BlockForm(block)
    }
}

impl ShapePlacement for StairFamily {
    fn held_rotations(&self) -> u8 {
        2
    }

    fn held_state(
        &self,
        _block: Block,
        rotation: &crate::world::placement::HeldRotation,
        selected: Option<crate::item::ItemType>,
    ) -> Option<crate::block_state::HeldBlockState> {
        Some(crate::block_state::HeldBlockState::Stair(StairState::new(
            crate::block_model::DEFAULT_MODEL_FACING,
            rotation.stair_half(selected),
        )))
    }

    fn authored_plan(
        &self,
        block: Block,
        inputs: &mut crate::world::placement::authored::Inputs<'_>,
    ) -> Result<PlacementPlan, String> {
        let half = match inputs.property("half", "bottom") {
            "bottom" => crate::block_state::StairHalf::Bottom,
            "top" => crate::block_state::StairHalf::Top,
            value => return Err(format!("unknown stair half '{value}'")),
        };
        Ok(PlacementPlan::single(
            inputs.anchor,
            block,
            StairState::new(inputs.facing()?, half).to_cell(),
        ))
    }

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
        let boxes = crate::stair::resolved_boxes_state(p, state, |q| stair_state_at(w, q));
        if !w.placement_cell_open(p) || occupied(p, boxes) {
            return PlacementOutcome::Refused;
        }
        PlacementOutcome::Plan(PlacementPlan::single(p, block, state.to_cell()))
    }
}

pub fn is_stair(block: Block) -> bool {
    block.shape_family() == ShapeFamily::Stair
}
