//! The engine shape families: one unit struct per [`ShapeFamily`], each a
//! `&'static` singleton implementing [`ShapeSim`] and [`ShapeRender`] by
//! delegating to the proven shape-math free functions
//! (`crate::{stair,slab,pane,fence,ladder,door}`, `crate::block_model`),
//! reading the world ONLY through the primitive [`ShapeNeighborhood`] seam.
//! A [`ShapeKindDef`](super::ShapeKindDef) row binds the singleton for its
//! family; adding a family is one struct here plus a [`singletons`] arm — not
//! an edit to every consumer.

use crate::mathh::IVec3;
use crate::world::World;

use super::super::{Aabb, Block, ShapeBox};
use super::facets::{ItemRender, ShapeCtx, ShapeRender, ShapeSim};
use super::neighborhood::{ShapeNeighborhood, ShapeState};
use super::{ConnectionParams, ItemForm, ShapeFamily, ShapeParams};
use crate::block_state::{SlabState, StairState};
use crate::facing::Facing;
use crate::torch::TorchPlacement;
use crate::world::placement::{PlaceInputs, PlacementOutcome, PlacementPlan, ShapePlacement};

// --- Reading per-cell state through the primitive seam --------------------
//
// A family decodes the OPAQUE `ShapeState` bytes its own blocks carry
// through the type's own [`CellView`]/[`CellCodec`] impls (living beside the
// type in its owner's module — the engine exports no byte vocabulary). The
// helpers here are thin seam reads: gate on ownership, decode.

use super::neighborhood::{CellCodec, CellView};

/// The typed state at `q`, gated on ownership: a foreign or absent cell
/// decodes as the type's default semantics.
fn state_of_at<T: CellView>(nb: &dyn ShapeNeighborhood, q: IVec3) -> T {
    if T::owns(nb.block(q)) {
        T::from_cell(nb.shape_state(q))
    } else {
        T::from_cell(ShapeState::NONE)
    }
}

/// The stair state a cell carries, or `None` when it is not a stair.
fn stair_state_at(nb: &dyn ShapeNeighborhood, q: IVec3) -> Option<StairState> {
    StairState::owns(nb.block(q)).then(|| StairState::from_cell(nb.shape_state(q)))
}

/// A cell's refined stair corner shape — a stored-state DECODE (resolved at
/// edit time by the refine cascade, never re-derived on a read).
fn stair_shape_at(nb: &dyn ShapeNeighborhood, q: IVec3) -> crate::stair::StairShape {
    if !crate::stair::is_stair(nb.block(q)) {
        // Never observed (consumers gate on the stair family first); answer
        // the default facing's straight shape rather than an empty mask.
        return crate::stair::shape(StairState::default());
    }
    crate::stair::StairShape::from_cell(nb.shape_state(q))
}

/// A cell's RAW slab state (readers normalize with the cell's block); a
/// non-slab cell decodes to the empty stack.
fn slab_state_at(nb: &dyn ShapeNeighborhood, q: IVec3) -> SlabState {
    state_of_at::<SlabState>(nb, q)
}

/// A model cell's footprint offset + facing through the seam.
fn model_state_at(nb: &dyn ShapeNeighborhood, q: IVec3) -> crate::block_model::ModelCellState {
    state_of_at::<crate::block_model::ModelCellState>(nb, q)
}

/// The door state a cell carries, or `None` (no state stored / not a door).
fn door_state_at(nb: &dyn ShapeNeighborhood, q: IVec3) -> Option<crate::door::DoorState> {
    state_of_at::<Option<crate::door::DoorState>>(nb, q)
}

/// Whether the pocket `(lo, hi)` overlaps any half-cell octant the predicate
/// reports occupied — the quantized occupancy test the stair and slab
/// families share.
fn any_octant(lo: [f32; 3], hi: [f32; 3], occ: &dyn Fn(usize, usize, usize) -> bool) -> bool {
    let touches = |a: usize, half: usize| {
        if half == 0 {
            lo[a] < 0.5
        } else {
            hi[a] > 0.5
        }
    };
    (0..8).any(|o| {
        let (ix, iy, iz) = (o & 1, (o >> 1) & 1, (o >> 2) & 1);
        touches(0, ix) && touches(1, iy) && touches(2, iz) && occ(ix, iy, iz)
    })
}

/// Box-overlap against a cell-local AABB.
fn overlaps(lo: [f32; 3], hi: [f32; 3], mn: [f32; 3], mx: [f32; 3]) -> bool {
    (0..3).all(|a| lo[a] < mx[a] && hi[a] > mn[a])
}

/// RESOLVE a connection mask from the neighbourhood — the refine-time (and
/// pre-placement hypothetical) computation. Reads decode the stored
/// [`crate::connect::ConnectionMask`] instead; nothing hot ends here.
pub(crate) fn resolve_connection_mask(
    nb: &dyn ShapeNeighborhood,
    pos: IVec3,
    rule: super::ConnectionRule,
    family: ShapeFamily,
) -> u8 {
    crate::connect::resolved_mask(
        pos,
        |q| nb.block(q),
        |q, (dx, dz)| super::facets::full_face_at(nb, q, IVec3::new(-dx, 0, -dz)),
        |b, dir, ff| crate::connect::connects(rule, family, b, dir, ff),
    )
}

/// A connection family's STORED resolved boxes at `pos`: the params'
/// precomputed box table indexed by the refined mask — one state read.
fn connection_boxes(
    nb: &dyn ShapeNeighborhood,
    pos: IVec3,
    c: &ConnectionParams,
    _family: ShapeFamily,
) -> &'static [Aabb] {
    crate::connect::boxes_for_mask(
        c.boxes,
        crate::connect::ConnectionMask::from_cell(nb.shape_state(pos)).0,
    )
}

/// A connection family's boxes for a NOT-YET-WRITTEN cell (the placement
/// overlap gate): the mask must be computed, there is nothing stored yet.
fn hypothetical_connection_boxes(
    nb: &dyn ShapeNeighborhood,
    pos: IVec3,
    c: &ConnectionParams,
    family: ShapeFamily,
) -> &'static [Aabb] {
    crate::connect::boxes_for_mask(c.boxes, resolve_connection_mask(nb, pos, c.rule, family))
}

/// The connection params of a fence/pane shape kind — a family invariant, so an
/// absence is a loader bug.
#[inline]
fn conn(p: &ShapeParams) -> &'static ConnectionParams {
    p.connection()
        .expect("a connection family carries connection params")
}

/// The item form a connection shape declares maps to its [`ItemRender`].
#[inline]
fn item_from_form(form: ItemForm, block: Block) -> ItemRender {
    match form {
        // A fixed no-neighbour segment, built from the family's item geometry.
        ItemForm::Segment => ItemRender::Geometry(block),
        // The item's own flat/extruded sprite.
        ItemForm::Sprite => ItemRender::ItemSprite,
        ItemForm::Cube => ItemRender::Cube(block),
    }
}

/// The union AABB of a box list (pane/fence selection outline), or `None` when
/// empty.
fn union(boxes: &[Aabb]) -> Option<([f32; 3], [f32; 3])> {
    if boxes.is_empty() {
        return None;
    }
    let mut mn = [f32::INFINITY; 3];
    let mut mx = [f32::NEG_INFINITY; 3];
    for b in boxes {
        for i in 0..3 {
            mn[i] = mn[i].min(b.min[i]);
            mx[i] = mx[i].max(b.max[i]);
        }
    }
    Some((mn, mx))
}

// --- Cube / lowered cube / plants / torch: sim + render defaults ----------------

/// Plain full cube — every facet is the trait default except the face
/// answer: a cube's faces are geometrically complete (material gates are the
/// ASKER's rule and apply to `Cube` answers only, so air and glass answering
/// here is safe).
pub struct CubeFamily;
impl ShapeSim for CubeFamily {
    fn full_face(
        &self,
        _p: &ShapeParams,
        _nb: &dyn ShapeNeighborhood,
        _pos: IVec3,
        _b: Block,
        _dir: IVec3,
    ) -> Option<super::facets::FullFace> {
        Some(super::facets::FullFace::Cube)
    }
}
impl ShapeRender for CubeFamily {}

/// A lowered cube: its collision box and lowered visual box are row data, so
/// the defaults (`Block::collision_boxes` / `Block::visual_aabb`) are correct.
pub struct LoweredCubeFamily;
impl ShapeSim for LoweredCubeFamily {
    fn light_shape(&self, p: &ShapeParams, b: Block) -> crate::block::BlockLightShape {
        // No partial-cell light shape — the deliberate simplification,
        // rounded to the nearer full case: a mostly-full cube (farmland,
        // 15/16) blocks like a full cube, a thin cover (the snow layer,
        // 1/16) blocks nothing. Anything else would darken the cell an
        // entity standing ON a thin cover occupies.
        let _ = p;
        if b.lowered_height().unwrap_or(0) >= 8 {
            crate::block::BlockLightShape::OpaqueCube
        } else {
            crate::block::BlockLightShape::Open
        }
    }
}
impl ShapeRender for LoweredCubeFamily {}

/// The cross billboard plant (grass/fern/flower). No collision; its item is a
/// flat sprite of the top tile.
pub struct CrossFamily;
impl ShapeSim for CrossFamily {}
impl ShapeRender for CrossFamily {
    fn item_render(&self, _p: &ShapeParams, block: Block) -> ItemRender {
        ItemRender::Tile(block.tiles()[0])
    }
}

/// The planted-crop lattice — like [`CrossFamily`] for item purposes.
pub struct CropFamily;
impl ShapeSim for CropFamily {}
impl ShapeRender for CropFamily {
    fn item_render(&self, _p: &ShapeParams, block: Block) -> ItemRender {
        ItemRender::Tile(block.tiles()[0])
    }
}

/// A torch: no collision (selectable by its pole in `player::interaction`); its
/// item is a flat sprite.
pub struct TorchFamily;
impl ShapeSim for TorchFamily {}
impl ShapeRender for TorchFamily {
    fn item_render(&self, _p: &ShapeParams, _block: Block) -> ItemRender {
        ItemRender::ItemSprite
    }
}

// --- Stateful chunk-meshed shapes ----------------------------------------------

/// A directional stair; boxes resolve corner shape from neighbours.
pub struct StairFamily;
impl ShapeSim for StairFamily {
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
        // Byte 0 (the PLACED facing + half) is identity and never refined;
        // byte 1 is the corner shape joined against the neighbour stairs'
        // placed bits. Corner resolution reads neighbours' PLACED state only,
        // so stair refinement can never cascade through other stairs.
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
    ) -> Option<super::facets::FullFace> {
        crate::stair::face_full(
            crate::stair::StairShape::from_cell(nb.shape_state(pos)),
            dir,
        )
        .then_some(super::facets::FullFace::Shaped)
    }

    fn light_shape(&self, _p: &ShapeParams, _b: Block) -> crate::block::BlockLightShape {
        crate::block::BlockLightShape::Shaped
    }

    fn light_apertures(&self, _p: &ShapeParams, _b: Block, state: ShapeState) -> u32 {
        // The PLACED bits drive light (the historical coarse choice — corner
        // joins don't re-light).
        let placed = StairState::from_cell(state);
        super::facets::pack_light_apertures(|(dx, dy, dz)| {
            crate::stair::light_side_mask(placed, dx, dy, dz)
        })
    }
}
impl ShapeRender for StairFamily {
    fn occupies_pocket(&self, ctx: &ShapeCtx<'_>, lo: [f32; 3], hi: [f32; 3]) -> bool {
        // The UNRESOLVED shape on purpose: corner-join resolution reads
        // neighbours the pad mesher cannot see, and both meshers must agree.
        let shape = crate::stair::shape(StairState::decode(ctx.nb.shape_state(ctx.pos).byte(0)));
        any_octant(lo, hi, &|ix, iy, iz| {
            crate::stair::shape_half_cell_occupied(shape, ix, iy, iz)
        })
    }

    fn boxes(&self, ctx: &ShapeCtx<'_>, out: &mut Vec<ShapeBox>) {
        let tiles = ctx.block.tiles();
        let shape = stair_shape_at(ctx.nb, ctx.pos);
        out.extend(
            crate::stair::boxes_for_shape(shape)
                .iter()
                .map(|a| ShapeBox::uniform(*a, tiles, ctx.tint_for)),
        );
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
        // A stair targets the whole cell (targeting is the whole cube).
        Some(([0.0, 0.0, 0.0], [1.0, 1.0, 1.0]))
    }
    fn item_render(&self, _p: &ShapeParams, block: Block) -> ItemRender {
        ItemRender::Geometry(block)
    }
}

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
    ) -> Option<super::facets::FullFace> {
        let state = crate::slab::normalize_state(b, slab_state_at(nb, pos));
        crate::slab::face_full(state, dir).then_some(super::facets::FullFace::Shaped)
    }

    fn light_shape(&self, _p: &ShapeParams, _b: Block) -> crate::block::BlockLightShape {
        crate::block::BlockLightShape::Shaped
    }

    fn light_apertures(&self, _p: &ShapeParams, b: Block, state: ShapeState) -> u32 {
        let st = crate::slab::normalize_state(b, SlabState::from_cell(state));
        super::facets::pack_light_apertures(|(dx, dy, dz)| {
            crate::slab::light_side_mask(st, dx, dy, dz)
        })
    }
}
impl ShapeRender for SlabFamily {
    fn occupies_pocket(&self, ctx: &ShapeCtx<'_>, lo: [f32; 3], hi: [f32; 3]) -> bool {
        let state = crate::slab::normalize_state(ctx.block, slab_state_at(ctx.nb, ctx.pos));
        state.is_full()
            || any_octant(lo, hi, &|ix, iy, iz| {
                crate::slab::half_cell_occupied(state, ix, iy, iz)
            })
    }

    fn meshes_as_cube(&self, ctx: &ShapeCtx<'_>) -> bool {
        // A same-material full stack IS the material's full cube: it falls to
        // the cube path so it culls, lights, and GREEDY-MERGES like one (the
        // merge is load-bearing for streaming perf). A mixed-material stack
        // keeps the per-layer boxes so each layer shows its own texture.
        let state = crate::slab::normalize_state(ctx.block, slab_state_at(ctx.nb, ctx.pos));
        crate::slab::is_uniform_full_stack(state)
    }

    fn boxes(&self, ctx: &ShapeCtx<'_>, out: &mut Vec<ShapeBox>) {
        let state = crate::slab::normalize_state(ctx.block, slab_state_at(ctx.nb, ctx.pos));
        for (slot, layer_block) in crate::slab::layer_slots(state) {
            let (min, max) = crate::mesh::slab::slot_box(slot);
            out.push(ShapeBox::uniform(
                Aabb { min, max },
                layer_block.tiles(),
                ctx.tint_for,
            ));
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
        ItemRender::Geometry(block)
    }
}

/// A glass pane (or a Layer-2 bar): post + arms resolved from neighbours, all
/// dimensions/rule/item-form from the connection params.
pub struct PaneFamily;
impl ShapeSim for PaneFamily {
    fn default_boxes(&self, p: &ShapeParams, _b: Block) -> &'static [Aabb] {
        // The bare no-neighbour post.
        crate::connect::boxes_for_mask(conn(p).boxes, 0)
    }

    fn collision_boxes(
        &self,
        p: &ShapeParams,
        nb: &dyn ShapeNeighborhood,
        pos: IVec3,
        _b: Block,
    ) -> &'static [Aabb] {
        connection_boxes(nb, pos, conn(p), ShapeFamily::Pane)
    }

    fn refine_state(
        &self,
        p: &ShapeParams,
        nb: &dyn ShapeNeighborhood,
        pos: IVec3,
        _b: Block,
        _state: ShapeState,
    ) -> ShapeState {
        crate::connect::ConnectionMask(resolve_connection_mask(
            nb,
            pos,
            conn(p).rule,
            ShapeFamily::Pane,
        ))
        .to_cell()
    }
}
impl ShapeRender for PaneFamily {
    fn occupies_pocket(&self, ctx: &ShapeCtx<'_>, lo: [f32; 3], hi: [f32; 3]) -> bool {
        // The mask-free POST: mask resolution is out of the pad's reach, and
        // rails are thin and corner-distant anyway.
        let c = conn(ctx.params);
        lo[0] < c.post_hi && hi[0] > c.post_lo && lo[2] < c.post_hi && hi[2] > c.post_lo
    }

    fn boxes(&self, ctx: &ShapeCtx<'_>, out: &mut Vec<ShapeBox>) {
        // [top, bottom, side] tiles = [edge, edge, glass].
        let [edge_tile, _bottom, glass_tile] = ctx.block.tiles();
        let c = conn(ctx.params);
        let mask = crate::connect::ConnectionMask::from_cell(ctx.nb.shape_state(ctx.pos)).0;
        crate::mesh::pane::push_mesh_boxes(
            out,
            c.post_lo,
            c.post_hi,
            mask,
            glass_tile,
            edge_tile,
            (ctx.tint_for)(glass_tile),
        );
    }

    fn picks_by_boxes(&self, _p: &ShapeParams) -> bool {
        true
    }
    fn selection_box(
        &self,
        p: &ShapeParams,
        nb: &dyn ShapeNeighborhood,
        pos: IVec3,
        _b: Block,
    ) -> Option<([f32; 3], [f32; 3])> {
        union(connection_boxes(nb, pos, conn(p), ShapeFamily::Pane))
    }
    fn item_render(&self, p: &ShapeParams, block: Block) -> ItemRender {
        item_from_form(conn(p).item_form, block)
    }
}

/// A fence (or a Layer-2 wall/hedge): post + arms resolved from neighbours, read
/// solid by nav, all dimensions/rule/item-form from the connection params.
pub struct FenceFamily;
impl ShapeSim for FenceFamily {
    fn default_boxes(&self, p: &ShapeParams, _b: Block) -> &'static [Aabb] {
        crate::connect::boxes_for_mask(conn(p).boxes, 0)
    }

    fn collision_boxes(
        &self,
        p: &ShapeParams,
        nb: &dyn ShapeNeighborhood,
        pos: IVec3,
        _b: Block,
    ) -> &'static [Aabb] {
        connection_boxes(nb, pos, conn(p), ShapeFamily::Fence)
    }

    fn refine_state(
        &self,
        p: &ShapeParams,
        nb: &dyn ShapeNeighborhood,
        pos: IVec3,
        _b: Block,
        _state: ShapeState,
    ) -> ShapeState {
        crate::connect::ConnectionMask(resolve_connection_mask(
            nb,
            pos,
            conn(p).rule,
            ShapeFamily::Fence,
        ))
        .to_cell()
    }

    fn full_face(
        &self,
        _p: &ShapeParams,
        _nb: &dyn ShapeNeighborhood,
        _pos: IVec3,
        _b: Block,
        dir: IVec3,
    ) -> Option<super::facets::FullFace> {
        // The post's flat top holds a floor mount (a torch on a fence post);
        // the sides are never a complete wall face.
        (dir.y > 0).then_some(super::facets::FullFace::Shaped)
    }

    fn nav_reads_solid(&self, _p: &ShapeParams) -> bool {
        true
    }
}
impl ShapeRender for FenceFamily {
    fn occupies_pocket(&self, ctx: &ShapeCtx<'_>, lo: [f32; 3], hi: [f32; 3]) -> bool {
        let c = conn(ctx.params);
        lo[0] < c.post_hi && hi[0] > c.post_lo && lo[2] < c.post_hi && hi[2] > c.post_lo
    }

    fn boxes(&self, ctx: &ShapeCtx<'_>, out: &mut Vec<ShapeBox>) {
        let tiles = ctx.block.tiles();
        let c = conn(ctx.params);
        let mask = crate::connect::ConnectionMask::from_cell(ctx.nb.shape_state(ctx.pos)).0;
        crate::mesh::fence::push_mesh_boxes(
            out,
            c.post_lo,
            c.post_hi,
            mask,
            tiles,
            (ctx.tint_for)(tiles[2]),
        );
    }

    fn picks_by_boxes(&self, _p: &ShapeParams) -> bool {
        true
    }
    fn selection_box(
        &self,
        p: &ShapeParams,
        nb: &dyn ShapeNeighborhood,
        pos: IVec3,
        _b: Block,
    ) -> Option<([f32; 3], [f32; 3])> {
        union(connection_boxes(nb, pos, conn(p), ShapeFamily::Fence))
    }
    fn item_render(&self, p: &ShapeParams, block: Block) -> ItemRender {
        item_from_form(conn(p).item_form, block)
    }
}

/// A climbable wall panel (the ladder); facing is block identity.
pub struct LadderFamily;
impl ShapeSim for LadderFamily {
    fn collision_boxes(
        &self,
        _p: &ShapeParams,
        _nb: &dyn ShapeNeighborhood,
        _pos: IVec3,
        b: Block,
    ) -> &'static [Aabb] {
        let (t, h) = b.ladder_dims();
        crate::ladder::collision_boxes_dim(b.panel_facing(), t, h)
    }
}
impl ShapeRender for LadderFamily {
    fn occupies_pocket(&self, ctx: &ShapeCtx<'_>, lo: [f32; 3], hi: [f32; 3]) -> bool {
        let (thickness, height) = ctx.block.ladder_dims();
        let (mn, mx) = crate::ladder::panel_aabb_dim(ctx.block.panel_facing(), thickness, height);
        overlaps(lo, hi, mn, mx)
    }

    fn boxes(&self, ctx: &ShapeCtx<'_>, out: &mut Vec<ShapeBox>) {
        let tile = ctx.block.tiles()[0];
        let (thickness, height) = ctx.block.ladder_dims();
        crate::mesh::ladder::push_mesh_box(
            out,
            ctx.block.panel_facing(),
            thickness,
            height,
            tile,
            (ctx.tint_for)(tile),
        );
    }

    fn selection_box(
        &self,
        _p: &ShapeParams,
        _nb: &dyn ShapeNeighborhood,
        _pos: IVec3,
        b: Block,
    ) -> Option<([f32; 3], [f32; 3])> {
        let (t, h) = b.ladder_dims();
        Some(crate::ladder::panel_aabb_dim(b.panel_facing(), t, h))
    }
    fn item_render(&self, _p: &ShapeParams, _block: Block) -> ItemRender {
        ItemRender::ItemSprite
    }
}

/// A bbmodel block; geometry/collision baked from the model, oriented per cell.
pub struct ModelFamily;
impl ShapeSim for ModelFamily {
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
    fn item_render(&self, p: &ShapeParams, _block: Block) -> ItemRender {
        ItemRender::Model(p.model_kind().expect("model family carries a model kind"))
    }
}

/// A wooden door; a thin slab on a cell edge, per-cell facing/open/half state.
pub struct DoorFamily;
impl ShapeSim for DoorFamily {
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

/// A mod-defined procedural shape (Layer 3). Collision/geometry come from the
/// WASM bake cache; on a cache miss or a trapped bake it falls back to the row's
/// static collision/visual boxes (the failure policy). Nav solidity is a
/// declared property of the shape.
pub struct CustomFamily;
impl ShapeSim for CustomFamily {
    fn light_shape(&self, p: &ShapeParams, _b: Block) -> crate::block::BlockLightShape {
        // The declared light tier: the simple open/opaque_cube declarations,
        // or the per-cell aperture whose opacity the SIM bake supplies
        // through the light snapshot.
        match p.custom() {
            Some(c) if c.light_shape == super::CustomLight::OpaqueCube => {
                crate::block::BlockLightShape::OpaqueCube
            }
            Some(c) if c.light_shape == super::CustomLight::CustomAperture => {
                crate::block::BlockLightShape::Shaped
            }
            _ => crate::block::BlockLightShape::Open,
        }
    }

    fn collision_boxes(
        &self,
        _p: &ShapeParams,
        nb: &dyn ShapeNeighborhood,
        pos: IVec3,
        block: Block,
    ) -> &'static [Aabb] {
        // The sim bake cache, or the row's static boxes on a miss / trapped bake.
        nb.baked_collision(pos)
            .unwrap_or_else(|| block.collision_boxes())
    }
    fn nav_reads_solid(&self, p: &ShapeParams) -> bool {
        p.custom().is_some_and(|c| c.nav_solid)
    }
}
impl ShapeRender for CustomFamily {
    fn occupies_pocket(&self, ctx: &ShapeCtx<'_>, lo: [f32; 3], hi: [f32; 3]) -> bool {
        ctx.nb.baked(ctx.pos).is_some_and(|boxes| {
            boxes
                .iter()
                .any(|bx| overlaps(lo, hi, bx.aabb.min, bx.aabb.max))
        })
    }

    fn boxes(&self, ctx: &ShapeCtx<'_>, out: &mut Vec<ShapeBox>) {
        // A Layer-3 shape draws what its WASM bake produced. No bake reachable
        // (never baked, trapped, or outside the caller's window) emits nothing,
        // and the caller falls back to the row's static form.
        let Some(baked) = ctx.nb.baked(ctx.pos) else {
            return;
        };
        let tiles = ctx.block.tiles();
        out.extend(baked.iter().map(|b| {
            // The bake's per-box tint multiplies the world tint; untinted
            // boxes carry [1.0; 3] and cost nothing.
            let tint_for = |tile: crate::atlas::Tile| {
                let world = (ctx.tint_for)(tile);
                [
                    world[0] * b.tint[0],
                    world[1] * b.tint[1],
                    world[2] * b.tint[2],
                ]
            };
            let mut mb = ShapeBox::uniform(b.aabb, tiles, tint_for).with_ao_strength(b.ao_strength);
            // The bake DECLARES whether its tint is a dye (sample the
            // dye-base twin) or a plain hue-preserving multiply over the
            // authored texels — both are expressible, per the wire field.
            mb.dyed = b.dyed;
            mb
        }));
    }

    fn picks_by_boxes(&self, _p: &ShapeParams) -> bool {
        true
    }
    fn item_render(&self, _p: &ShapeParams, block: Block) -> ItemRender {
        // The item KIND is true baked geometry: `render::item_cube`'s custom
        // branch draws the shape's `BakeShapeItem` boxes from the item cache
        // (cube fallback on a miss). Still a `BlockCube` render kind, so item
        // entities / in-hand / icon all route through the cube item renderer.
        ItemRender::Geometry(block)
    }
}

// --- Singletons + binding -------------------------------------------------------

static CUBE: CubeFamily = CubeFamily;
static LOWERED_CUBE: LoweredCubeFamily = LoweredCubeFamily;
static CROSS: CrossFamily = CrossFamily;
static CROP: CropFamily = CropFamily;
static TORCH: TorchFamily = TorchFamily;
static STAIR: StairFamily = StairFamily;
static SLAB: SlabFamily = SlabFamily;
static PANE: PaneFamily = PaneFamily;
static FENCE: FenceFamily = FenceFamily;
static LADDER: LadderFamily = LadderFamily;
static MODEL: ModelFamily = ModelFamily;
static DOOR: DoorFamily = DoorFamily;
static CUSTOM: CustomFamily = CustomFamily;

/// The `(sim, render)` facet singletons for `family` — the binding
/// `shape_kind::build` stamps onto every [`ShapeKindDef`](super::ShapeKindDef).
// --- Placement: each family owns its own, dispatched by the registry --------
//
// The engine's former per-family placement match now lives here as
// `ShapePlacement` impls. Cube / lowered cube / cross / crop / custom keep the
// trait default (`General` — the generic single-cell path); the shapes with
// bespoke placement override.

impl ShapePlacement for CubeFamily {}
impl ShapePlacement for LoweredCubeFamily {}
impl ShapePlacement for CrossFamily {}
impl ShapePlacement for CropFamily {}
impl ShapePlacement for CustomFamily {}

impl ShapePlacement for StairFamily {
    fn placement_plan(
        &self,
        w: &World,
        block: Block,
        inputs: &PlaceInputs,
        occupied: &mut dyn FnMut(IVec3, &[Aabb]) -> bool,
    ) -> PlacementOutcome {
        let p = inputs.place_pos;
        let half = inputs.held_rotation.stair_half(inputs.held);
        let state = StairState::new(inputs.player_facing, half);
        // The boxes the stair WOULD have: its hypothetical own state (the cell
        // is still empty) corner-resolved against the placed neighbours,
        // through the same seam the placed shape will read.
        let boxes = crate::stair::resolved_boxes_state(p, state, |q| stair_state_at(w, q));
        if !w.placement_cell_open(p) || occupied(p, boxes) {
            return PlacementOutcome::Refused;
        }
        // The placed bits only; the refine cascade appends the corner byte.
        PlacementOutcome::Plan(PlacementPlan::single(p, block, state.to_cell()))
    }
}

impl ShapePlacement for SlabFamily {
    fn placement_plan(
        &self,
        w: &World,
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
        PlacementOutcome::Plan(PlacementPlan::single(
            target,
            crate::slab::representative_block(next),
            next.to_cell(),
        ))
    }
}

impl ShapePlacement for DoorFamily {
    fn placement_plan(
        &self,
        w: &World,
        block: Block,
        inputs: &PlaceInputs,
        occupied: &mut dyn FnMut(IVec3, &[Aabb]) -> bool,
    ) -> PlacementOutcome {
        // A door is a 2-tall thin block: both cells must be loaded +
        // replaceable with a floor to stand on, and the closed slab must not
        // trap a body. It sits on the edge nearest the placer.
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
            writes: vec![(p, block, half(false)), (upper, block, half(true))],
        })
    }
}

impl ShapePlacement for ModelFamily {
    fn placement_plan(
        &self,
        w: &World,
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
        let oriented = !centered
            && (block.directional_view() || crate::block_model::instance(kind).cells.len() > 1);
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
                    (
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

impl ShapePlacement for PaneFamily {
    fn placement_plan(
        &self,
        w: &World,
        block: Block,
        inputs: &PlaceInputs,
        occupied: &mut dyn FnMut(IVec3, &[Aabb]) -> bool,
    ) -> PlacementOutcome {
        connection_placement(w, block, inputs.place_pos, ShapeFamily::Pane, occupied)
    }
}

impl ShapePlacement for FenceFamily {
    fn placement_plan(
        &self,
        w: &World,
        block: Block,
        inputs: &PlaceInputs,
        occupied: &mut dyn FnMut(IVec3, &[Aabb]) -> bool,
    ) -> PlacementOutcome {
        connection_placement(w, block, inputs.place_pos, ShapeFamily::Fence, occupied)
    }
}

/// A connection shape (fence / pane / wall) occupies only its resolved post +
/// arms, tested from the BLOCK's own params since the target cell is still
/// empty. No stored state — connections re-resolve from neighbours wherever
/// the shape is read.
fn connection_placement(
    w: &World,
    block: Block,
    p: IVec3,
    family: ShapeFamily,
    occupied: &mut dyn FnMut(IVec3, &[Aabb]) -> bool,
) -> PlacementOutcome {
    if !w.placement_cell_open(p) {
        return PlacementOutcome::Refused;
    }
    let c = conn(&block.shape_kind_def().params);
    if occupied(p, hypothetical_connection_boxes(w, p, c, family)) {
        return PlacementOutcome::Refused;
    }
    // A plain block write: the refine cascade stores the resolved mask.
    PlacementOutcome::Plan(PlacementPlan::single(p, block, ShapeState::NONE))
}

impl ShapePlacement for TorchFamily {
    fn placement_plan(
        &self,
        w: &World,
        block: Block,
        inputs: &PlaceInputs,
        occupied: &mut dyn FnMut(IVec3, &[Aabb]) -> bool,
    ) -> PlacementOutcome {
        // A torch-shaped block mounts on a floor or wall (never a ceiling) and
        // needs a usable support face. Replacing a plant drops it to the FLOOR.
        // Then the shared single-cell tail applies (substrate/replaceable/body).
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

impl ShapePlacement for LadderFamily {
    fn placement_plan(
        &self,
        w: &World,
        block: Block,
        inputs: &PlaceInputs,
        occupied: &mut dyn FnMut(IVec3, &[Aabb]) -> bool,
    ) -> PlacementOutcome {
        // A ladder-shaped block mounts on a vertical wall face with a complete
        // face behind its panel; the clicked normal names the panel front. The
        // panel is real collision, so the shared tail's body gate applies.
        let p = inputs.place_pos;
        let Some(facing) = Facing::from_horizontal_normal(inputs.normal) else {
            return PlacementOutcome::Refused;
        };
        if !w.ladder_supported_at(p, facing) {
            return PlacementOutcome::Refused;
        }
        let (t, h) = block.ladder_dims();
        let boxes = crate::ladder::collision_boxes_dim(facing, t, h);
        // The facing IS the block row (the sapling-stage pattern): the plan
        // writes the sibling row whose panel fronts the clicked normal — no
        // per-cell state, and no engine write vocabulary.
        let row = block.wall_panel_row(facing);
        match w.finish_single_cell_placement(row, p, ShapeState::NONE, boxes, occupied) {
            Some(plan) => PlacementOutcome::Plan(plan),
            None => PlacementOutcome::Refused,
        }
    }
}

pub(super) fn singletons(
    family: ShapeFamily,
) -> (
    &'static dyn ShapeSim,
    &'static dyn ShapeRender,
    &'static dyn ShapePlacement,
) {
    match family {
        ShapeFamily::Cube => (&CUBE, &CUBE, &CUBE),
        ShapeFamily::LoweredCube => (&LOWERED_CUBE, &LOWERED_CUBE, &LOWERED_CUBE),
        ShapeFamily::Cross => (&CROSS, &CROSS, &CROSS),
        ShapeFamily::Crop => (&CROP, &CROP, &CROP),
        ShapeFamily::Torch => (&TORCH, &TORCH, &TORCH),
        ShapeFamily::Stair => (&STAIR, &STAIR, &STAIR),
        ShapeFamily::Slab => (&SLAB, &SLAB, &SLAB),
        ShapeFamily::Pane => (&PANE, &PANE, &PANE),
        ShapeFamily::Fence => (&FENCE, &FENCE, &FENCE),
        ShapeFamily::Ladder => (&LADDER, &LADDER, &LADDER),
        ShapeFamily::Model => (&MODEL, &MODEL, &MODEL),
        ShapeFamily::Door => (&DOOR, &DOOR, &DOOR),
        ShapeFamily::Custom => (&CUSTOM, &CUSTOM, &CUSTOM),
    }
}

/// Whether a family answers [`ShapeRender::boxes`] — the mesher's cheap
/// per-cell gate, mirrored onto [`ShapeKindDef::resolves_to_boxes`] so the hot
/// loop is a field read. Adding a box family means adding it HERE and
/// implementing `boxes`, nothing else in the mesher.
pub(super) fn resolves_to_boxes(family: ShapeFamily) -> bool {
    matches!(
        family,
        ShapeFamily::Stair
            | ShapeFamily::Slab
            | ShapeFamily::Pane
            | ShapeFamily::Fence
            | ShapeFamily::Ladder
            | ShapeFamily::Custom
    )
}

/// Whether a family overrides [`ShapeSim::refine_state`] — mirrored onto
/// [`ShapeKindDef::refines`] so the edit cascade's per-cell gate is a field
/// read. Adding a neighbour-refined family means adding it HERE and
/// implementing `refine_state`, nothing in the cascade.
pub(super) fn refines(family: ShapeFamily) -> bool {
    matches!(
        family,
        ShapeFamily::Stair | ShapeFamily::Pane | ShapeFamily::Fence
    )
}
