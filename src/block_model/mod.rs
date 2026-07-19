//! Data-driven Blockbench (`.bbmodel`) BLOCKS — the chunk-meshed, world-placed kind,
//! the counterpart to the legacy atlas-cube blocks rather than to mobs.
//!
//! A bbmodel block is authored like a mob (cube elements + per-face UVs + an embedded
//! texture) but is a *block*: baked into the chunk mesh at remesh, lit at mesh-time, and
//! broken/collided with per cell exactly like a legacy block. The only thing it can't
//! share with the legacy packed path is its texturing — bbmodel faces carry arbitrary
//! sub-rectangle UVs, which the tile-packed vertex + fixed atlas can't express — so model
//! geometry rides a second, explicit-UV vertex stream in the chunk mesh and samples a
//! combined [`ModelAtlas`] instead of the block atlas.
//!
//! # Three layers
//!
//! 1. [`BlockModel`] — the CACHED parse: cube geometry (model space) + the decoded
//!    texture. This is the expensive step (`serde_json` + base64 + PNG decode), compiled
//!    once into a `.llblock` (see [`crate::asset_cache`]) and reused.
//! 2. [`ModelAtlas`] — all kinds' textures stacked into one sheet, with a per-kind UV
//!    transform, built once from the cached models. Shared by the off-thread mesher (UV
//!    remap) and the renderer (GPU upload).
//! 3. [`ModelInstance`] — the runtime bake derived from the cached model + its data row:
//!    the cell footprint, the cubes mapped into footprint space (with atlas UVs) and
//!    SPLIT per occupied cell, and each cell's collision + selection box. Cheap, so it
//!    lives outside the cache — tweaking the footprint or collision needs no cache bump.
//!
//! # Multi-block
//!
//! A model larger than one cell (the workbench is 2×2×1) declares its `cells` footprint
//! in its data row; the bake fits the model into that cell box (uniform scale, X/Z
//! centred, resting on the floor) and assigns each cube to the cell containing its
//! centre. In the world every footprint cell holds the block id; the per-chunk
//! `model_cells` map records authored cell offsets, and `model_facings` records placed
//! orientation. Placement gates the whole footprint clear, breaking any cell breaks the
//! group, and each cell meshes only its own cubes + collides with its own boxes.

use crate::facing::Facing;

mod ao;
mod atlas;
mod compiled;
mod defs;
mod display;
mod geometry;
mod instance;
mod placement;
mod query;
#[cfg(test)]
mod tests;

pub use atlas::{atlas, particle_patch};
pub use compiled::*;
pub use defs::*;
pub use display::*;
pub use instance::*;
pub use placement::{
    base_from_cell, base_from_front_left_anchor, oriented_footprint_cells, placement_transform,
};
pub use query::{
    collision_boxes, collision_boxes_oriented, model_render_boxes, outline_bounds, ray_vs_model,
    selection_aabb, selection_aabb_oriented,
};

pub(crate) use geometry::render_face_bias;

use compiled::MODELS;
use geometry::{box_corners, cell_of, clip_to_cell, posed_cube_bounds, union_clip_to_cell};
use placement::{oriented_cell_instance, placement_transform_fp};

/// Canonical bbmodel orientation: Blockbench model fronts face `-Z` (North).
/// Old model placements that predate per-cell facing read as this unrotated orientation.
pub const DEFAULT_MODEL_FACING: Facing = Facing::North;
