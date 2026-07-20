use serde::{Deserialize, Serialize};

use crate::block_model::BlockModelKind;

/// How a block's geometry is meshed. `Cube` is the standard 6-face box; `Cross`
/// is an X of two diagonal billboard quads (grass, ferns, flowers, mushrooms);
/// `Torch` is a thin pole (a small box) standing on the floor or tilted against a
/// wall, with its orientation read from the chunk's torch map (see `mesh::torch`);
/// `Model` is a data-driven Blockbench block ([`BlockModelKind`]) — NOT chunk-meshed
/// (like the chest it is drawn each frame as a placed model, see
/// `render::placed_model`), with its own texture, collision and selection baked from
/// the `.bbmodel` (see [`crate::block_model`]).
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RenderShape {
    Cube,
    /// A cube whose VISIBLE box stops `16 - n` texels short of the cell top
    /// (`{"lowered_cube": 15}` = 1 texel shorter — farmland, future dirt
    /// paths). Meshes through the ordinary cube face loop with the top
    /// lowered; rows must NOT carry the `opaque` flag (the sunken top means
    /// neighbours keep their faces — see the mesher notes) but the block
    /// still blocks light like a full cube ([`Block::light_shape`]). Offers
    /// no complete torch-support face; collision follows the row's boxes.
    LoweredCube(u8),
    Cross,
    /// A planted-crop lattice: four axis-aligned billboard quads, one pair
    /// perpendicular to each horizontal axis, inset [`CROP_PLANE_INSET`] from
    /// the cell faces and spanning edge to edge along their long axis (a `#`
    /// seen from above). Same cutout/flat-lit treatment as [`Cross`]
    /// (see [`RenderShape::Cross`]); reads as a row crop instead of a tuft.
    Crop,
    Torch,
    /// A chunk-meshed directional stair, with the low side facing the player when
    /// placed. Its per-cell facing lives in the section's stair-facing map; collision,
    /// selection, and meshing resolve straight/corner boxes through `crate::stair`.
    Stair,
    /// A chunk-meshed half-cell slab. Its per-cell state stores the split axis and up
    /// to two material-bearing layers, so a cell can hold mixed slabs without adding a
    /// registry row for every material pair.
    Slab,
    /// A chunk-meshed glass pane: a thin full-height post that grows arms toward
    /// the horizontal neighbours it connects to. The connection mask is NOT
    /// stored state — it is resolved from the current neighbours wherever the
    /// shape is needed (collision, selection, meshing), like stair corners. See
    /// `crate::pane` for the connection rules and boxes.
    Pane,
    /// A chunk-meshed wooden fence: a 4/16-thick full-height post growing a pair
    /// of horizontal rails (2/16 from the top and bottom, 3/16 thick) toward each
    /// connected side. Like the pane it stores NO per-cell state — the 4-bit
    /// connection mask is resolved from the current neighbours at every query.
    /// See `crate::fence` for the connection rules and boxes.
    Fence,
    /// A chunk-meshed climbable wall panel: a 1/16-thick alpha-cutout slice flush
    /// against the vertical wall face it hangs on. Its facing (the direction the
    /// panel front points, away from the wall) lives in the section's shared
    /// entity-facing map; the one panel box in `crate::ladder` drives targeting,
    /// the outline, the crack overlay, the mesh (see `mesh::ladder`), and the
    /// facing-resolved collision (a body bumps the thin panel and can stand on
    /// top of a column — resolved position-aware, so the ROW's collision stays
    /// empty). Climbing itself is a player-physics rule keyed off
    /// [`BlockTag::CLIMBABLE`], not the collision shape.
    Ladder,
    Model(BlockModelKind),
    /// A wooden door: a 2-tall thin slab on a cell edge. Like the chest it is NOT
    /// chunk-meshed — it is drawn each frame as a dynamic hinged model (see
    /// `render::door_model`) so the leaf can swing smoothly. Its facing + open +
    /// which-half state lives in the chunk door map; the per-cell collision and
    /// selection boxes are resolved position-aware in `world::door` from that state
    /// (see [`crate::door`]). The mesher skips a door cell, exactly like a chest.
    Door,
}

/// How far a [`RenderShape::Crop`] plane sits in from the cell faces it is
/// perpendicular to (2/16 of a block). Shared by the mesher, the targeting
/// ray, and the selection outline so they always trace the same geometry.
pub const CROP_PLANE_INSET: f32 = 2.0 / 16.0;

/// How far a [`RenderShape::Crop`] plane hangs BELOW its cell (1/16): the
/// art's bottom row sits on the sunken top of the farmland underneath
/// ([`RenderShape::LoweredCube`]) instead of floating a texel above it. On a
/// full block (a wild crop on grass) the overhang is buried and invisible.
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
