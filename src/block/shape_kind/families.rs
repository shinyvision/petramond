//! The engine shape families: one unit struct per [`ShapeFamily`], each a
//! `&'static` singleton implementing [`ShapeSim`] and [`ShapeRender`] by
//! delegating to the proven shape-math free functions
//! (`crate::{stair,slab,pane,fence,ladder,door}`, `crate::block_model`) and the
//! `World` box accessors. A [`ShapeKindDef`](super::ShapeKindDef) row binds the
//! singleton for its family; adding a family is one struct here plus a
//! [`singletons`] arm — not an edit to every consumer.

use crate::mathh::IVec3;
use crate::world::World;

use super::super::{Aabb, Block};
use super::facets::{ItemRender, ShapeRender, ShapeSim};
use super::{ConnectionParams, ItemForm, ShapeFamily, ShapeParams};

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

/// Plain full cube — every facet is the trait default.
pub struct CubeFamily;
impl ShapeSim for CubeFamily {}
impl ShapeRender for CubeFamily {}

/// A lowered cube: its collision box and lowered visual box are row data, so
/// the defaults (`Block::collision_boxes` / `Block::visual_aabb`) are correct.
pub struct LoweredCubeFamily;
impl ShapeSim for LoweredCubeFamily {}
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
    fn collision_boxes(&self, _p: &ShapeParams, w: &World, pos: IVec3, _b: Block) -> &'static [Aabb] {
        w.stair_boxes_at(pos.x, pos.y, pos.z)
    }
}
impl ShapeRender for StairFamily {
    fn selection_box(
        &self,
        _p: &ShapeParams,
        _w: &World,
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
    fn collision_boxes(&self, _p: &ShapeParams, w: &World, pos: IVec3, _b: Block) -> &'static [Aabb] {
        w.slab_boxes_at(pos.x, pos.y, pos.z)
    }
}
impl ShapeRender for SlabFamily {
    fn selection_box(
        &self,
        _p: &ShapeParams,
        w: &World,
        pos: IVec3,
        _b: Block,
    ) -> Option<([f32; 3], [f32; 3])> {
        w.slab_visual_aabb_at(pos.x, pos.y, pos.z)
    }
    fn item_render(&self, _p: &ShapeParams, block: Block) -> ItemRender {
        ItemRender::Geometry(block)
    }
}

/// A glass pane (or a Layer-2 bar): post + arms resolved from neighbours, all
/// dimensions/rule/item-form from the connection params.
pub struct PaneFamily;
impl ShapeSim for PaneFamily {
    fn collision_boxes(&self, p: &ShapeParams, w: &World, pos: IVec3, _b: Block) -> &'static [Aabb] {
        w.connection_boxes_at(pos, conn(p), ShapeFamily::Pane)
    }
}
impl ShapeRender for PaneFamily {
    fn selection_box(
        &self,
        p: &ShapeParams,
        w: &World,
        pos: IVec3,
        _b: Block,
    ) -> Option<([f32; 3], [f32; 3])> {
        union(w.connection_boxes_at(pos, conn(p), ShapeFamily::Pane))
    }
    fn item_render(&self, p: &ShapeParams, block: Block) -> ItemRender {
        item_from_form(conn(p).item_form, block)
    }
}

/// A fence (or a Layer-2 wall/hedge): post + arms resolved from neighbours, read
/// solid by nav, all dimensions/rule/item-form from the connection params.
pub struct FenceFamily;
impl ShapeSim for FenceFamily {
    fn collision_boxes(&self, p: &ShapeParams, w: &World, pos: IVec3, _b: Block) -> &'static [Aabb] {
        w.connection_boxes_at(pos, conn(p), ShapeFamily::Fence)
    }
    fn nav_reads_solid(&self, _p: &ShapeParams) -> bool {
        true
    }
}
impl ShapeRender for FenceFamily {
    fn selection_box(
        &self,
        p: &ShapeParams,
        w: &World,
        pos: IVec3,
        _b: Block,
    ) -> Option<([f32; 3], [f32; 3])> {
        union(w.connection_boxes_at(pos, conn(p), ShapeFamily::Fence))
    }
    fn item_render(&self, p: &ShapeParams, block: Block) -> ItemRender {
        item_from_form(conn(p).item_form, block)
    }
}

/// A climbable wall panel (the ladder); facing is block identity.
pub struct LadderFamily;
impl ShapeSim for LadderFamily {
    fn collision_boxes(&self, _p: &ShapeParams, _w: &World, _pos: IVec3, b: Block) -> &'static [Aabb] {
        let (t, h) = b.ladder_dims();
        crate::ladder::collision_boxes_dim(b.panel_facing(), t, h)
    }
}
impl ShapeRender for LadderFamily {
    fn selection_box(
        &self,
        _p: &ShapeParams,
        _w: &World,
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
    fn collision_boxes(&self, p: &ShapeParams, w: &World, pos: IVec3, _b: Block) -> &'static [Aabb] {
        let kind = p.model_kind().expect("model family carries a model kind");
        crate::block_model::collision_boxes_oriented(
            kind,
            w.model_offset_at(pos.x, pos.y, pos.z),
            w.model_facing_at(pos.x, pos.y, pos.z),
        )
    }
}
impl ShapeRender for ModelFamily {
    fn selection_box(
        &self,
        p: &ShapeParams,
        w: &World,
        pos: IVec3,
        _b: Block,
    ) -> Option<([f32; 3], [f32; 3])> {
        let kind = p.model_kind().expect("model family carries a model kind");
        crate::block_model::selection_aabb_oriented(
            kind,
            w.model_offset_at(pos.x, pos.y, pos.z),
            w.model_facing_at(pos.x, pos.y, pos.z),
        )
    }
    fn item_render(&self, p: &ShapeParams, _block: Block) -> ItemRender {
        ItemRender::Model(p.model_kind().expect("model family carries a model kind"))
    }
}

/// A wooden door; a thin slab on a cell edge, per-cell facing/open/half state.
pub struct DoorFamily;
impl ShapeSim for DoorFamily {
    fn collision_boxes(&self, _p: &ShapeParams, w: &World, pos: IVec3, b: Block) -> &'static [Aabb] {
        match w.door_state_at(pos.x, pos.y, pos.z) {
            Some(state) => crate::door::collision_boxes(state),
            None => b.collision_boxes(),
        }
    }
}
impl ShapeRender for DoorFamily {
    fn selection_box(
        &self,
        _p: &ShapeParams,
        w: &World,
        pos: IVec3,
        b: Block,
    ) -> Option<([f32; 3], [f32; 3])> {
        match w.door_state_at(pos.x, pos.y, pos.z) {
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
    fn collision_boxes(&self, _p: &ShapeParams, w: &World, pos: IVec3, block: Block) -> &'static [Aabb] {
        // The sim bake cache, or the row's static boxes on a miss / trapped bake.
        w.custom_shape_boxes(pos)
            .unwrap_or_else(|| block.collision_boxes())
    }
    fn nav_reads_solid(&self, p: &ShapeParams) -> bool {
        p.custom().is_some_and(|c| c.nav_solid)
    }
}
impl ShapeRender for CustomFamily {
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
pub(super) fn singletons(
    family: ShapeFamily,
) -> (&'static dyn ShapeSim, &'static dyn ShapeRender) {
    match family {
        ShapeFamily::Cube => (&CUBE, &CUBE),
        ShapeFamily::LoweredCube => (&LOWERED_CUBE, &LOWERED_CUBE),
        ShapeFamily::Cross => (&CROSS, &CROSS),
        ShapeFamily::Crop => (&CROP, &CROP),
        ShapeFamily::Torch => (&TORCH, &TORCH),
        ShapeFamily::Stair => (&STAIR, &STAIR),
        ShapeFamily::Slab => (&SLAB, &SLAB),
        ShapeFamily::Pane => (&PANE, &PANE),
        ShapeFamily::Fence => (&FENCE, &FENCE),
        ShapeFamily::Ladder => (&LADDER, &LADDER),
        ShapeFamily::Model => (&MODEL, &MODEL),
        ShapeFamily::Door => (&DOOR, &DOOR),
        ShapeFamily::Custom => (&CUSTOM, &CUSTOM),
    }
}
