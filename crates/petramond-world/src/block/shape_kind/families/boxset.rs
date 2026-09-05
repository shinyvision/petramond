//! The STATIC BOX SET — any block whose form is a fixed authored box list
//! (farmland, the snow layer, the cactus, a pack's dirt path). The list is the
//! whole geometry source: mesh, collision, outline, targeting, AO, apertures.
//!
//! Sim, render, and placement for this family live together here; the shared
//! seam helpers and the singleton table stay in the parent.

use super::*;

/// A STATIC BOX SET — the one family for any block whose form is a fixed list
/// of axis-aligned boxes authored as data (`{"boxes": [...]}`): farmland and
/// the snow layer (one box), the cactus (an inset trunk plus its two cap
/// plates), a mod's dirt path or pressure plate. The authored list is the
/// WHOLE geometry source: mesh, collision, outline, targeting, AO and light
/// apertures all resolve from it, so a row can neither restate nor contradict
/// its own shape.
pub struct BoxSetFamily;

impl ShapeSim for BoxSetFamily {
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
        // Per BOX: drawn matter always occludes light and AO, but a
        // decorative plate is walked through (mob spawning and the surface
        // probes deliberately skip non-colliding cover).
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
        // Every DRAWN box, colliding or not: a walk-through cover and a
        // tilted plane are what the player sees and points at.
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
        // The MATTER boxes, collide-or-not: a snow cover shadows without
        // obstructing, and a face plane spanning the cell carries a full-width
        // face without being a body.
        box_set(p)
            .boxes(box_set_turns(nb, pos, b), box_set_form(p, nb, pos))
            .iter()
            .filter(|d| d.occludes)
            .any(|d| match d.pose {
                Some(pose) => pose.overlaps_aabb(d.aabb.min, d.aabb.max, lo, hi),
                None => overlaps(lo, hi, d.aabb.min, d.aabb.max),
            })
    }

    fn light_shape(&self, _p: &ShapeParams, _b: Block) -> crate::block::BlockLightShape {
        // Always shaped: the apertures fall out of the boxes (the trait
        // default derives them from `occupies_pocket`), so a full-cell box set
        // blocks light and a thin cover only shadows what it covers.
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
        if !box_set(p).corner_joins {
            return state;
        }
        // Byte 0 (the placed facing) is IDENTITY and never refined; byte 1 is
        // the corner form — the stair's identity/refined split, resolved by
        // the SAME neighbour rule stairs use (`crate::stair::resolved_shape`):
        // a perpendicular same-kind neighbour BEHIND makes an outer corner, IN
        // FRONT an inner corner, else straight. Reading only neighbours'
        // PLACED facings keeps the cascade acyclic, exactly like stairs.
        let facing = state_of_at::<EntityFront>(nb, pos).0;
        let own_kind = block.shape_kind();
        let neighbour_facing = |q: IVec3| -> Option<Facing> {
            let nb_block = nb.block(q);
            (nb_block.shape_kind() == own_kind)
                .then(|| state_of_at::<EntityFront>(nb, q).0)
                .filter(|g| g.dir().dot(facing.dir()) == 0)
        };
        // Odd forms = the neighbour faces one quarter turn CLOCKWISE of us.
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
    /// Derived from the BOXES, so any arrangement of matter answers for
    /// itself: a counter whose worktop spans the cell holds a wall/floor mount,
    /// an L of trim does not, and neither is named anywhere. `Shaped`, never
    /// `Cube` — the material rules that bind a cube face (opaque-only joins)
    /// must not bind a shape that merely happens to be complete.
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
    fn item_boxes(
        &self,
        p: &ShapeParams,
        b: Block,
        _state: crate::block_state::HeldBlockState,
        out: &mut Vec<crate::block::ItemBox>,
    ) {
        let Some(set) = p.box_set() else { return };
        // A shape with a FRONT draws half-turned: the authored front is `-Z`
        // and the iso icon presents `+Y`/`-X`/`+Z`, so the authored form would
        // show the viewer nothing but its back. Same correction a `.bbmodel`
        // pack writes as a 180° yaw in its `gui` display transform.
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
        // The DRAWN extent, whether or not it collides — a walk-through cover
        // stays aimable, like a no-collision model block.
        let b = box_set(p).bounds(0, 0);
        Some((b.min, b.max))
    }

    fn item_render(&self, _p: &ShapeParams, block: Block) -> ItemRender {
        // The icon / dropped / in-hand forms draw the shape's own boxes, so
        // the item reads as the block it will place.
        ItemRender::BlockForm(block)
    }
}

impl ShapePlacement for BoxSetFamily {}
