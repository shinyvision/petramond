use serde::{Deserialize, Serialize};

/// How far a crop plane ([`ShapeFamily::Crop`](super::ShapeFamily::Crop)) sits
/// in from the cell faces it is perpendicular to (2/16 of a block). Shared by
/// the mesher, the targeting ray, and the selection outline so they always
/// trace the same geometry.
pub const CROP_PLANE_INSET: f32 = 2.0 / 16.0;

/// How far a crop plane hangs BELOW its cell (1/16): the art's bottom row sits
/// on the sunken top of the farmland underneath (a lowered cube) instead of
/// floating a texel above it. On a full block (a wild crop on grass) the
/// overhang is buried and invisible.
pub const CROP_PLANE_DROP: f32 = 1.0 / 16.0;

/// How a block participates in light propagation. This is the render/collision-neutral
/// shape category that `world::light` consumes; per-cell state, such as stair facing,
/// still lives in the section and is interpreted by the lighting shape layer.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum BlockLightShape {
    Open,
    OpaqueCube,
    Stair,
    Slab,
    /// A Layer-3 custom shape whose per-cell opacity comes from its SIM bake's
    /// `light_aperture` (gathered into the light snapshot). Coarse: a cell is
    /// opaque or open to light per the baked bit — the same nearer-full rounding
    /// the lowered cube uses, without a per-quadrant mask.
    CustomAperture,
}

/// One axis-aligned box of a block's collision shape, in CELL-LOCAL coordinates
/// (`0.0..1.0` per axis). A block's full shape is a *list* of these (see
/// [`Block::collision_boxes`]) — one for a full cube or the inset chest, several for
/// shapes like stairs. The player collides via a swept-AABB over them, and the
/// selection outline + break overlay derive from their union.
#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Aabb {
    pub min: [f32; 3],
    pub max: [f32; 3],
}
