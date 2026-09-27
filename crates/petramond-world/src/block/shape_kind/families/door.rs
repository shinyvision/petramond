use super::*;

pub struct DoorFamily;

impl ShapeSim for DoorFamily {
    fn compound_members(
        &self,
        _p: &ShapeParams,
        _block: Block,
        pos: IVec3,
        state: ShapeState,
    ) -> Option<Vec<(IVec3, ShapeState)>> {
        let door = Option::<crate::door::DoorState>::from_cell(state)?;
        let lower = if door.top { pos - IVec3::Y } else { pos };
        let half = |top| crate::door::DoorState { top, ..door }.to_cell();
        Some(vec![(lower, half(false)), (lower + IVec3::Y, half(true))])
    }

    fn rotate_y(&self, block: Block, state: ShapeState) -> crate::block::rotation::CellRotation {
        let mut door = crate::door::DoorState::from_cell(state);
        door.facing = crate::block::rotation::facing(door.facing);
        crate::block::rotation::CellRotation::unchanged(block, door.to_cell())
    }

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
    fn mesh_emitter(&self, _p: &ShapeParams) -> MeshEmitter {
        MeshEmitter::Nothing
    }

    fn animated_pose(
        &self,
        _p: &ShapeParams,
        _block: Block,
        state: ShapeState,
    ) -> Option<crate::animated_model::AnimatedPose> {
        let door = crate::door::DoorState::from_cell(state);
        (!door.top).then_some(crate::animated_model::AnimatedPose {
            facing: door.facing,
            variant: 0,
            open: door.open,
        })
    }

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
    fn authored_state(&self, _block: Block, state: ShapeState) -> ShapeState {
        crate::door::DoorState {
            open: false,
            ..crate::door::DoorState::from_cell(state)
        }
        .to_cell()
    }

    fn construction_writes(
        &self,
        block: Block,
        state: ShapeState,
        pos: IVec3,
    ) -> crate::world::placement::ConstructionWrites {
        let door = crate::door::DoorState::from_cell(state);
        if door.top {
            return crate::world::placement::ConstructionWrites::Member(pos - IVec3::Y);
        }
        crate::world::placement::ConstructionWrites::Anchor(PlacementPlan {
            anchor: pos,
            writes: [false, true]
                .into_iter()
                .map(|top| {
                    PlacementPlan::whole(
                        pos + IVec3::Y * i32::from(top),
                        block,
                        crate::door::DoorState {
                            open: false,
                            top,
                            ..door
                        }
                        .to_cell(),
                    )
                })
                .collect(),
        })
    }

    fn authored_plan(
        &self,
        block: Block,
        inputs: &mut crate::world::placement::authored::Inputs<'_>,
    ) -> Result<PlacementPlan, String> {
        let facing = inputs.facing()?;
        let open = match inputs.property("open", "false") {
            "true" => true,
            "false" => false,
            value => return Err(format!("unknown door open value '{value}'")),
        };
        Ok(PlacementPlan {
            anchor: inputs.anchor,
            writes: [false, true]
                .into_iter()
                .map(|top| {
                    PlacementPlan::whole(
                        inputs.anchor + IVec3::Y * i32::from(top),
                        block,
                        crate::door::DoorState { facing, open, top }.to_cell(),
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

pub fn is_door(block: Block) -> bool {
    block.shape_family() == ShapeFamily::Door
}
