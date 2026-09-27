use super::*;

pub struct TorchFamily;

impl ShapeSim for TorchFamily {
    fn collision_state_free(&self) -> bool {
        true
    }

    fn nav_follows_row(&self) -> bool {
        true
    }

    fn rotate_y(&self, block: Block, state: ShapeState) -> crate::block::rotation::CellRotation {
        let mount = crate::torch::TorchPlacement::from_cell(state);
        let turned = match mount {
            crate::torch::TorchPlacement::Floor => mount,
            crate::torch::TorchPlacement::North => crate::torch::TorchPlacement::East,
            crate::torch::TorchPlacement::East => crate::torch::TorchPlacement::South,
            crate::torch::TorchPlacement::South => crate::torch::TorchPlacement::West,
            crate::torch::TorchPlacement::West => crate::torch::TorchPlacement::North,
        };
        crate::block::rotation::CellRotation::unchanged(block, turned.to_cell())
    }

    fn mount(
        &self,
        _p: &ShapeParams,
        nb: &dyn ShapeNeighborhood,
        pos: IVec3,
        _b: Block,
    ) -> Option<ShapeMount> {
        let placement = TorchPlacement::from_cell(nb.shape_state(pos));
        Some(ShapeMount {
            cell: placement.support_cell(pos),
            normal: placement.support_normal(),
        })
    }
}

impl ShapeRender for TorchFamily {
    fn mesh_emitter(&self, _p: &ShapeParams) -> MeshEmitter {
        MeshEmitter::Pole
    }

    fn precise_pick(&self, _p: &ShapeParams) -> bool {
        true
    }

    fn item_render(&self, _p: &ShapeParams, _block: Block) -> ItemRender {
        ItemRender::ItemSprite
    }
}

impl ShapePlacement for TorchFamily {
    fn authored_plan(
        &self,
        block: Block,
        inputs: &mut crate::world::placement::authored::Inputs<'_>,
    ) -> Result<PlacementPlan, String> {
        let mount = match inputs.property("mount", "floor") {
            "floor" => TorchPlacement::Floor,
            "wall" => TorchPlacement::from_place_normal(inputs.facing()?.dir()).unwrap(),
            value => return Err(format!("unknown torch mount '{value}'")),
        };
        Ok(PlacementPlan::single(inputs.anchor, block, mount.to_cell()))
    }

    fn placement_plan(
        &self,
        w: &WorldData,
        block: Block,
        inputs: &PlaceInputs,
        occupied: &mut dyn FnMut(IVec3, &[Aabb]) -> bool,
    ) -> PlacementOutcome {
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

pub fn is_torch(block: Block) -> bool {
    block.shape_family() == ShapeFamily::Torch
}
