use crate::mathh::IVec3;
use crate::world::data::WorldData;

use super::super::{Aabb, Block, ShapeBox};
use super::facets::{
    union_box as union, ItemRender, MeshEmitter, PlantPlanes, RowFacts, ShapeCtx, ShapeMount,
    ShapeRender, ShapeSim,
};
use super::neighborhood::{ShapeNeighborhood, ShapeState};
use super::{BlockShapeKind, ConnectionParams, ItemForm, ShapeFamily, ShapeParams};
use crate::block_state::{EntityFront, SlabState, StairState};
use crate::facing::Facing;
use crate::torch::TorchPlacement;
use crate::world::placement::{PlaceInputs, PlacementOutcome, PlacementPlan, ShapePlacement};

use super::neighborhood::{CellCodec, CellView};

fn state_of_at<T: CellView>(nb: &dyn ShapeNeighborhood, q: IVec3) -> T {
    if T::owns(nb.block(q)) {
        T::from_cell(nb.shape_state(q))
    } else {
        T::from_cell(ShapeState::NONE)
    }
}

fn stair_state_at(nb: &dyn ShapeNeighborhood, q: IVec3) -> Option<StairState> {
    StairState::owns(nb.block(q)).then(|| StairState::from_cell(nb.shape_state(q)))
}

fn stair_shape_at(nb: &dyn ShapeNeighborhood, q: IVec3) -> crate::stair::StairShape {
    if !crate::stair::is_stair(nb.block(q)) {
        return crate::stair::shape(StairState::default());
    }
    crate::stair::StairShape::from_cell(nb.shape_state(q))
}

fn slab_state_at(nb: &dyn ShapeNeighborhood, q: IVec3) -> SlabState {
    state_of_at::<SlabState>(nb, q)
}

fn model_state_at(nb: &dyn ShapeNeighborhood, q: IVec3) -> crate::block_model::ModelCellState {
    state_of_at::<crate::block_model::ModelCellState>(nb, q)
}

fn door_state_at(nb: &dyn ShapeNeighborhood, q: IVec3) -> Option<crate::door::DoorState> {
    state_of_at::<Option<crate::door::DoorState>>(nb, q)
}

fn trapdoor_state_at(
    nb: &dyn ShapeNeighborhood,
    q: IVec3,
) -> Option<crate::trapdoor::TrapdoorState> {
    state_of_at::<Option<crate::trapdoor::TrapdoorState>>(nb, q)
}

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

fn overlaps(lo: [f32; 3], hi: [f32; 3], mn: [f32; 3], mx: [f32; 3]) -> bool {
    (0..3).all(|a| lo[a] < mx[a] && hi[a] > mn[a])
}

pub fn resolve_connection_mask(
    nb: &dyn ShapeNeighborhood,
    pos: IVec3,
    rule: super::ConnectionRule,
    kind: BlockShapeKind,
) -> u8 {
    crate::connect::resolved_mask(
        pos,
        |q| nb.block(q),
        |q, (dx, dz)| super::facets::full_face_at(nb, q, IVec3::new(-dx, 0, -dz)),
        |b, dir, ff| crate::connect::connects(rule, kind, b, dir, ff),
    )
}

fn connection_boxes(
    nb: &dyn ShapeNeighborhood,
    pos: IVec3,
    c: &ConnectionParams,
) -> &'static [Aabb] {
    crate::connect::boxes_for_mask(
        c.boxes,
        crate::connect::ConnectionMask::from_cell(nb.shape_state(pos)).0,
    )
}

fn hypothetical_connection_boxes(
    nb: &dyn ShapeNeighborhood,
    pos: IVec3,
    c: &ConnectionParams,
    kind: BlockShapeKind,
) -> &'static [Aabb] {
    crate::connect::boxes_for_mask(c.boxes, resolve_connection_mask(nb, pos, c.rule, kind))
}

#[inline]
fn conn(p: &ShapeParams) -> &'static ConnectionParams {
    p.connection()
        .expect("a connection family carries connection params")
}

#[inline]
fn item_from_form(form: ItemForm, block: Block) -> ItemRender {
    match form {
        ItemForm::Segment => ItemRender::BlockForm(block),
        ItemForm::Sprite => ItemRender::ItemSprite,
        ItemForm::Cube => ItemRender::BlockForm(block),
    }
}

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
mod trapdoor;

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
use trapdoor::TrapdoorFamily;

pub use door::is_door;
pub use stair::is_stair;
pub use torch::is_torch;
pub use trapdoor::is_trapdoor;

#[inline]
fn box_set(p: &ShapeParams) -> &'static super::BoxSetParams {
    p.box_set().expect("a box-set family carries its boxes")
}

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

fn box_set_form(p: &ShapeParams, nb: &dyn ShapeNeighborhood, pos: IVec3) -> super::CornerForm {
    if box_set(p).refine == super::BoxSetRefine::None {
        return 0;
    }
    nb.shape_state(pos).byte(1)
}

fn run_segment(nb: &dyn ShapeNeighborhood, q: IVec3, kind: super::BlockShapeKind) -> bool {
    nb.block(q).shape_kind() == kind
}

fn opposing_run(nb: &dyn ShapeNeighborhood, q: IVec3, root: super::RunRoot) -> bool {
    nb.block(q)
        .shape_kind()
        .params()
        .box_set()
        .and_then(|b| b.run())
        .is_some_and(|r| r.root == root.opposite())
}

pub fn resolve_run_form(
    nb: &dyn ShapeNeighborhood,
    pos: IVec3,
    kind: super::BlockShapeKind,
    root: super::RunRoot,
) -> super::RunForm {
    super::run_form::run_form(
        pos,
        root,
        |q| run_segment(nb, q, kind),
        |q| opposing_run(nb, q, root),
    )
}

/// One authored box as drawn geometry.
///
/// A box textures like a cube of the same row, `[top, bottom, side]` plus the row's `front` on the
/// face its placement facing points to, carved to the box's own extent. Per box you only author
/// what row tiles can't say: a face drawn through a different surface than the cell's outside (a
/// shelf under a counter top), which overrides. The UV turn comes from
/// [`face_uv_turns`](super::face_uv_turns).
pub fn box_set_box(
    d: &super::BoxDef,
    turns: u8,
    block: Block,
    tint_for: &dyn Fn(crate::tile::Tile) -> [f32; 3],
) -> ShapeBox {
    let mut b = ShapeBox::uniform(d.aabb, block.tiles(), tint_for);
    b.pose = d.pose;
    b.casts_ao = d.casts_ao;
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
        // This face's art sits some quarter turns ahead of the cell's: the shape's turn plus the
        // face's own `art_turns`, or none for a posed box, whose pose carries the turn. That total
        // picks which face carries the row's `front` and how far a `±Y` tile is counter-rotated.
        // Corner forms wrap one face by a different turn count than its siblings, so a plain
        // turn-index lookup can't work.
        let art_turns = d.face_frame_turns(turns, i);
        let front = front.filter(|_| i == super::FRONT_AFTER_TURN[art_turns as usize]);
        if let Some(tile) = d.tiles[i].or(front) {
            style.tile = tile;
            style.tint = tint_for(tile);
        }
        style.uv_turns = (super::face_uv_turns(i, art_turns) + d.uv_turns[i]) & 3;
        style.uv_rect = d.uv[i];
    }
    b
}

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
static TRAPDOOR: TrapdoorFamily = TrapdoorFamily;
static CUSTOM: CustomFamily = CustomFamily;

fn connection_placement(
    w: &WorldData,
    block: Block,
    p: IVec3,
    occupied: &mut dyn FnMut(IVec3, &[Aabb]) -> bool,
) -> PlacementOutcome {
    if !w.placement_cell_open(p) {
        return PlacementOutcome::Refused;
    }
    let c = conn(&block.shape_kind_def().params);
    if occupied(
        p,
        hypothetical_connection_boxes(w, p, c, block.shape_kind()),
    ) {
        return PlacementOutcome::Refused;
    }
    PlacementOutcome::Plan(PlacementPlan::single(p, block, ShapeState::NONE))
}

pub fn singletons(
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
        ShapeFamily::Trapdoor => (&TRAPDOOR, &TRAPDOOR, &TRAPDOOR),
        ShapeFamily::Custom => (&CUSTOM, &CUSTOM, &CUSTOM),
    }
}
