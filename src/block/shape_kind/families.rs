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
use super::facets::{union_box as union, ItemRender, ShapeCtx, ShapeMount, ShapeRender, ShapeSim};
use super::neighborhood::{ShapeNeighborhood, ShapeState};
use super::{ConnectionParams, ItemForm, ShapeFamily, ShapeParams};
use crate::block_state::{EntityFront, SlabState, StairState};
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
        // A fixed no-neighbour segment: the family's `item_boxes` builds it.
        ItemForm::Segment => ItemRender::BlockForm(block),
        // The item's own flat/extruded sprite.
        ItemForm::Sprite => ItemRender::ItemSprite,
        ItemForm::Cube => ItemRender::BlockForm(block),
    }
}

// --- The families themselves, one file each ------------------------------------
//
// Each family is INDEPENDENT — a stair knows nothing about a fence — so each
// owns a file holding its struct plus all three facet impls. Adding a family
// is a new file plus one `singletons` arm.
mod boxset;
mod cube;
mod custom;
mod door;
mod fence;
mod ladder;
mod model;
mod pane;
mod plant;
mod slab;
mod stair;
mod torch;

use boxset::BoxSetFamily;
use cube::CubeFamily;
use custom::CustomFamily;
use door::DoorFamily;
use fence::FenceFamily;
use ladder::LadderFamily;
use model::ModelFamily;
use pane::PaneFamily;
use plant::{CropFamily, CrossFamily};
use slab::SlabFamily;
use stair::StairFamily;
use torch::TorchFamily;

/// The box list of a box-set kind — a family invariant, so an absence is a
/// loader bug.
#[inline]
fn box_set(p: &ShapeParams) -> &'static super::BoxSetParams {
    p.box_set().expect("a box-set family carries its boxes")
}

/// How far a box-set cell is turned about Y: the placement facing a
/// `directional_view` row stores, as quarter turns from the authored form
/// (whose front is `-Z`, matching [`Facing::North`]). A row with no facing
/// never stores one and always reads `0`.
fn box_set_turns(nb: &dyn ShapeNeighborhood, pos: IVec3, block: Block) -> u8 {
    if !block.directional_view() {
        return 0;
    }
    turns_for(state_of_at::<EntityFront>(nb, pos).0)
}

fn turns_for(facing: Facing) -> u8 {
    match facing {
        Facing::North => 0,
        Facing::East => 1,
        Facing::South => 2,
        Facing::West => 3,
    }
}

/// A box-set cell's stored corner form (byte 1), or straight when the shape
/// does not corner-join. Byte 0 is the placed facing and is never refined —
/// the stair's identity/refined split.
///
/// Reads the form off the params the facet was ALREADY handed rather than
/// re-deriving them from the block: `occupies_pocket` calls this once per AO
/// probe corner and once per light-aperture quadrant, and neither may pay a
/// `def()` load. An out-of-vocabulary byte (an old world's stale state, until
/// the load sweep rewrites it) is clamped by the accessors — see
/// [`BoxSetParams::boxes`](super::BoxSetParams::boxes).
fn box_set_form(p: &ShapeParams, nb: &dyn ShapeNeighborhood, pos: IVec3) -> super::CornerForm {
    if !box_set(p).corner_joins {
        return 0;
    }
    nb.shape_state(pos).byte(1)
}

/// One authored box as drawn geometry.
///
/// A box textures exactly like a cube of the same row — `[top, bottom, side]`
/// plus the row's `front` on the face its placement facing points to — carved
/// to the box's own extent. Only what no row-level tile can express is
/// authored per box: a face the shape draws through a DIFFERENT surface than
/// the cell's outside (a shelf under a counter top), which overrides. Plus the
/// UV turn [`face_uv_turns`](super::face_uv_turns) prescribes.
pub(crate) fn box_set_box(
    d: &super::BoxDef,
    turns: u8,
    block: Block,
    tint_for: &dyn Fn(crate::atlas::Tile) -> [f32; 3],
) -> ShapeBox {
    let mut b = ShapeBox::uniform(d.aabb, block.tiles(), tint_for);
    if !d.occludes {
        b = b.as_face_carrier();
    }
    if d.double_sided {
        b = b.double_sided();
    }
    let front = block.front_tile();
    for (i, face) in b.faces.iter_mut().enumerate() {
        if !d.faces[i] {
            *face = None;
            continue;
        }
        let Some(style) = face else { continue };
        // This face's art lives in a frame `d.art_turns[i]` quarter turns
        // ahead of the shape's own, so both frame-dependent decisions read the
        // TOTAL turn: which face carries the row's `front` (a corner form's
        // wrapped face is a different number of turns from the authored one
        // than its siblings, which is why a single turn-index lookup could not
        // express it) and how far a `±Y` tile must be counter-rotated.
        let art_turns = (turns + d.art_turns[i]) & 3;
        let front = front.filter(|_| i == super::FRONT_AFTER_TURN[art_turns as usize]);
        if let Some(tile) = d.tiles[i].or(front) {
            style.tile = tile;
            style.tint = tint_for(tile);
        }
        style.uv_turns = super::face_uv_turns(i, art_turns);
    }
    b
}

// --- Singletons + binding -------------------------------------------------------

static CUBE: CubeFamily = CubeFamily;
static BOX_SET: BoxSetFamily = BoxSetFamily;
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

pub(super) fn singletons(
    family: ShapeFamily,
) -> (
    &'static dyn ShapeSim,
    &'static dyn ShapeRender,
    &'static dyn ShapePlacement,
) {
    match family {
        ShapeFamily::Cube => (&CUBE, &CUBE, &CUBE),
        ShapeFamily::BoxSet => (&BOX_SET, &BOX_SET, &BOX_SET),
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
        ShapeFamily::BoxSet
            | ShapeFamily::Stair
            | ShapeFamily::Slab
            | ShapeFamily::Pane
            | ShapeFamily::Fence
            | ShapeFamily::Ladder
            | ShapeFamily::Custom
    )
}

/// Whether a family's CELL COLLISION is fully determined by the block id —
/// i.e. it does NOT override [`ShapeSim::collision_boxes`], so the trait
/// default (the row's position-less boxes) is the whole answer. Mirrored onto
/// [`ShapeKindDef::collision_state_free`] so the per-id collision table can be
/// baked once and every cell probe skips the virtual resolve.
///
/// SAFE BY DEFAULT: a family listed here that later grows a per-cell
/// `collision_boxes` override must be removed from this list, and
/// `collision_state_free_kinds_resolve_identically` fails until it is.
pub(super) fn collision_is_state_free(family: ShapeFamily) -> bool {
    matches!(
        family,
        ShapeFamily::Cube | ShapeFamily::Cross | ShapeFamily::Crop | ShapeFamily::Torch
    )
}

/// Whether a family overrides [`ShapeSim::refine_state`] — mirrored onto
/// [`ShapeKindDef::refines`] so the edit cascade's per-cell gate is a field
/// read. Adding a neighbour-refined family means adding it HERE and
/// implementing `refine_state`, nothing in the cascade.
pub(super) fn refines(family: ShapeFamily, params: &ShapeParams) -> bool {
    match family {
        ShapeFamily::Stair | ShapeFamily::Pane | ShapeFamily::Fence => true,
        // Per KIND, not per family: only a box set that actually declares a
        // connect group has anything to refine, so farmland and the snow layer
        // keep the cascade's cheap "nothing shaped nearby" path.
        ShapeFamily::BoxSet => params.box_set().is_some_and(|s| s.corner_joins),
        _ => false,
    }
}
