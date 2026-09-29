use super::*;
use crate::block::shape_kind::RunRoot;

pub struct BoxSetFamily;

impl ShapeSim for BoxSetFamily {
    fn refines(&self, p: &ShapeParams) -> bool {
        box_set(p).refine != super::super::BoxSetRefine::None
    }

    fn nav_follows_row(&self) -> bool {
        true
    }

    fn validate_row(&self, p: &ShapeParams, row: &RowFacts) -> Result<(), String> {
        if row.corners && !row.flags.is_directional_view() {
            return Err("'corners' requires the 'directional_view' flag".into());
        }
        if box_set(p).run().is_some() && row.flags.is_directional_view() {
            return Err("a 'run' row cannot carry the 'directional_view' flag".into());
        }
        if row.flags.is_opaque() {
            return Err("a 'boxes' row must not carry the 'opaque' flag".into());
        }
        if row.flags.occludes_ao() {
            return Err(
                "a 'boxes' row must not carry the 'ao_occluder' flag — its shape answers \
                        occlusion per box"
                    .into(),
            );
        }
        if row.authored_collision {
            return Err(
                "a 'boxes' row derives its collision from the shape; author \
                        \"collision\": [] and set \"collides\": false on any box that should be \
                        walked through"
                    .into(),
            );
        }
        Ok(())
    }

    fn collision_boxes(
        &self,
        p: &ShapeParams,
        nb: &dyn ShapeNeighborhood,
        pos: IVec3,
        block: Block,
    ) -> &'static [Aabb] {
        box_set(p).collision(box_set_turns(nb, pos, block), box_set_form(p, nb, pos))
    }

    fn default_boxes(&self, p: &ShapeParams, _b: Block) -> &'static [Aabb] {
        box_set(p).collision(0, 0)
    }

    fn target_boxes(
        &self,
        p: &ShapeParams,
        nb: &dyn ShapeNeighborhood,
        pos: IVec3,
        block: Block,
        out: &mut Vec<crate::block::PosedBox>,
    ) {
        out.extend_from_slice(
            box_set(p).targets(box_set_turns(nb, pos, block), box_set_form(p, nb, pos)),
        );
    }

    fn occupies_pocket(
        &self,
        p: &ShapeParams,
        nb: &dyn ShapeNeighborhood,
        pos: IVec3,
        b: Block,
        lo: [f32; 3],
        hi: [f32; 3],
    ) -> bool {
        box_set(p)
            .boxes(box_set_turns(nb, pos, b), box_set_form(p, nb, pos))
            .iter()
            .filter(|d| d.occludes)
            .any(|d| match d.pose {
                Some(pose) => pose.overlaps_aabb(d.aabb.min, d.aabb.max, lo, hi),
                None => overlaps(lo, hi, d.aabb.min, d.aabb.max),
            })
    }

    fn shades_pocket(
        &self,
        p: &ShapeParams,
        nb: &dyn ShapeNeighborhood,
        pos: IVec3,
        b: Block,
        lo: [f32; 3],
        hi: [f32; 3],
    ) -> bool {
        box_set(p)
            .boxes(box_set_turns(nb, pos, b), box_set_form(p, nb, pos))
            .iter()
            .filter(|d| d.occludes && d.casts_ao)
            .any(|d| match d.pose {
                Some(pose) => pose.overlaps_aabb(d.aabb.min, d.aabb.max, lo, hi),
                None => overlaps(lo, hi, d.aabb.min, d.aabb.max),
            })
    }

    fn shade_boxes(
        &self,
        p: &ShapeParams,
        nb: &dyn ShapeNeighborhood,
        pos: IVec3,
        b: Block,
        out: &mut Vec<Aabb>,
    ) -> bool {
        for d in box_set(p)
            .boxes(box_set_turns(nb, pos, b), box_set_form(p, nb, pos))
            .iter()
            .filter(|d| d.occludes && d.casts_ao)
        {
            if d.pose.is_some() {
                return false;
            }
            out.push(d.aabb);
        }
        true
    }

    fn light_shape(&self, _p: &ShapeParams, _b: Block) -> crate::block::BlockLightShape {
        crate::block::BlockLightShape::Shaped
    }

    fn refine_state(
        &self,
        p: &ShapeParams,
        nb: &dyn ShapeNeighborhood,
        pos: IVec3,
        block: Block,
        state: ShapeState,
    ) -> ShapeState {
        match box_set(p).refine {
            super::super::BoxSetRefine::None => return state,
            super::super::BoxSetRefine::Run(run) => {
                let form = resolve_run_form(nb, pos, block.shape_kind(), run.root);
                return ShapeState::new(&[state.byte(0), form]);
            }
            super::super::BoxSetRefine::Corners => {}
        }
        // Byte 0 is the placed facing and never gets refined. Byte 1 is the corner form, from the
        // same neighbour rule as stairs (`crate::stair::resolved_shape`). Only neighbours' placed
        // facings are read, so the cascade stays acyclic.
        let facing = state_of_at::<EntityFront>(nb, pos).0;
        let own_kind = block.shape_kind();
        let neighbour_facing = |q: IVec3| -> Option<Facing> {
            let nb_block = nb.block(q);
            (nb_block.shape_kind() == own_kind)
                .then(|| state_of_at::<EntityFront>(nb, q).0)
                .filter(|g| g.dir().dot(facing.dir()) == 0)
        };
        let side = |g: Facing| -> u8 {
            if turns_for(g) == (turns_for(facing) + 1) & 3 {
                0
            } else {
                1
            }
        };
        let behind = -facing.dir();
        let form = if let Some(g) = neighbour_facing(pos + behind) {
            1 + side(g)
        } else if let Some(g) = neighbour_facing(pos - behind) {
            3 + side(g)
        } else {
            0
        };
        ShapeState::new(&[state.byte(0), form])
    }
    fn full_face(
        &self,
        _p: &ShapeParams,
        nb: &dyn ShapeNeighborhood,
        pos: IVec3,
        _b: Block,
        dir: IVec3,
    ) -> Option<crate::block::shape_kind::facets::FullFace> {
        crate::block::shape_kind::facets::face_is_solid(nb, pos, dir)
            .then_some(crate::block::shape_kind::facets::FullFace::Shaped)
    }
}

impl ShapeRender for BoxSetFamily {
    fn mesh_emitter(&self, _p: &ShapeParams) -> MeshEmitter {
        MeshEmitter::Boxes
    }

    fn item_boxes(
        &self,
        p: &ShapeParams,
        b: Block,
        _state: crate::block_state::HeldBlockState,
        out: &mut Vec<crate::block::ItemBox>,
    ) {
        let Some(set) = p.box_set() else { return };
        // Shapes with a front are drawn half-turned. Their authored front is `-Z` but the iso
        // icon shows `+Y`/`-X`/`+Z`, so otherwise you'd only see the back. `.bbmodel` packs do
        // the same with a 180° yaw in their `gui` display transform.
        let turns = if b.directional_view() { 2 } else { 0 };
        for box_def in set.boxes(turns, 0) {
            out.push(crate::block::ItemBox {
                aabb: box_def.aabb,
                faces: box_def.faces,
                material: None,
                tiles: box_def.tiles,
                uv_turns: std::array::from_fn(|i| {
                    (crate::block::face_uv_turns(i, box_def.face_frame_turns(turns, i))
                        + box_def.uv_turns[i])
                        & 3
                }),
                uv_rects: box_def.uv,
                pose: box_def.pose,
            });
        }
    }

    fn boxes(&self, ctx: &ShapeCtx<'_>, out: &mut Vec<ShapeBox>) {
        let turns = box_set_turns(ctx.nb, ctx.pos, ctx.block);
        let form = box_set_form(ctx.params, ctx.nb, ctx.pos);
        out.extend(
            box_set(ctx.params)
                .boxes(turns, form)
                .iter()
                .map(|d| box_set_box(d, turns, ctx.block, ctx.tint_for)),
        )
    }

    fn selection_box(
        &self,
        p: &ShapeParams,
        nb: &dyn ShapeNeighborhood,
        pos: IVec3,
        block: Block,
    ) -> Option<([f32; 3], [f32; 3])> {
        let b = box_set(p).bounds(box_set_turns(nb, pos, block), box_set_form(p, nb, pos));
        Some((b.min, b.max))
    }

    fn default_selection_box(
        &self,
        p: &ShapeParams,
        _block: Block,
    ) -> Option<([f32; 3], [f32; 3])> {
        let b = box_set(p).bounds(0, 0);
        Some((b.min, b.max))
    }

    fn item_render(&self, _p: &ShapeParams, block: Block) -> ItemRender {
        ItemRender::BlockForm(block)
    }
}

impl ShapePlacement for BoxSetFamily {
    /// A run picks its ROOT from the click and its row from the root. A
    /// ceiling or floor click names the root outright (the clicked side);
    /// a wall click tries standing first, then hanging. A root holds when
    /// the cell that way is a segment of the same run or presents a
    /// complete face (the wall-torch test). Resolving the OTHER root writes
    /// the row's `flipped` sibling — orientation as block identity, the
    /// ladder-row pattern — and the form is pre-resolved from the
    /// neighbours so the write lands already refined. Every other box set
    /// keeps the generic single-cell path.
    fn placement_plan(
        &self,
        w: &WorldData,
        block: Block,
        inputs: &PlaceInputs,
        occupied: &mut dyn FnMut(IVec3, &[Aabb]) -> bool,
    ) -> PlacementOutcome {
        let Some(own) = box_set(&block.shape_kind_def().params).run() else {
            return PlacementOutcome::General;
        };
        let p = inputs.place_pos;
        let roots = match inputs.normal.y {
            0 => [RunRoot::Down, RunRoot::Up],
            y if y < 0 => [RunRoot::Up, RunRoot::Down],
            _ => [RunRoot::Down, RunRoot::Up],
        };
        for root in roots {
            let row = if root == own.root {
                block
            } else {
                match block.flipped_row() {
                    Some(row) => row,
                    None => continue,
                }
            };
            let kind = row.shape_kind();
            let nb: &dyn ShapeNeighborhood = w;
            let anchor = p + root.dir();
            let held = run_segment(nb, anchor, kind) || w.mount_face_complete(anchor, root.tip());
            if !held {
                continue;
            }
            let form = resolve_run_form(nb, p, kind, root);
            let boxes = box_set(kind.params()).collision(0, form);
            return match w.finish_single_cell_placement(
                row,
                p,
                ShapeState::new(&[0, form]),
                boxes,
                occupied,
            ) {
                Some(plan) => PlacementOutcome::Plan(plan),
                None => PlacementOutcome::Refused,
            };
        }
        PlacementOutcome::Refused
    }
}
