//! Data-driven Blockbench (`.bbmodel`) blocks: the chunk-meshed, world-placed kind, counterpart
//! to the legacy atlas-cube blocks rather than to mobs.
//!
//! A bbmodel block is authored like a mob (cubes, per-face UVs, an embedded texture) but behaves
//! like a block. It is baked into the chunk mesh, lit at mesh time, and broken and collided per
//! cell like a legacy block. Only the texturing differs: its faces carry arbitrary sub-rectangle
//! UVs that the tile-packed vertex can't express, so model geometry uses a second, explicit-UV
//! vertex stream and samples a combined `ModelAtlas`.
//!
//! # Three layers
//!
//! 1. [`BlockModel`] is the cached parse: cube geometry plus the decoded texture. Parsing is
//!    expensive, so it is compiled once into a `.llblock` (see [`crate::asset_cache`]).
//! 2. `ModelAtlas` packs every kind's texture into one sheet with a per-kind UV transform. The
//!    mesher uses it for UV remap and the renderer uploads it.
//! 3. [`ModelInstance`] is the runtime bake from the cached model and its data row: the cell
//!    footprint, the cubes split per occupied cell, and each cell's collision and selection box.
//!    It is cheap, so it lives outside the cache and footprint tweaks need no cache bump.
//!
//! # Multi-block
//!
//! A model larger than one cell (the workbench is 2×2×1) declares a `cells` footprint in its data
//! row. The bake scales the model uniformly into that box, centred on X/Z and resting on the
//! floor, and gives each cube to the cell holding its centre. Every footprint cell holds the
//! block id; the per-chunk `model_cells` map records authored offsets and `model_facings` the
//! placed orientation. Placement needs the whole footprint clear, and breaking any cell breaks
//! the group. Each cell meshes and collides with only its own cubes and boxes.

use crate::facing::Facing;

mod ao;
mod atlas;
mod compiled;
mod defs;
mod display;
mod geometry;
pub mod instance;
mod material;
mod placement;
mod query;
#[cfg(test)]
mod tests;

pub use ao::{bake_box_ao, AoBox};
pub use atlas::{atlas, particle_patch};
pub use compiled::*;
pub use defs::*;
pub use display::*;
pub use instance::*;
pub use material::{FaceAppearance, FrameStrip, SurfaceMaterial, TextureAnimation};
pub use placement::{
    base_from_cell, base_from_centered_anchor, base_from_front_left_anchor,
    oriented_footprint_cells, placement_transform,
};
pub use query::{
    collision_boxes, collision_boxes_oriented, model_render_boxes, outline_bounds, ray_vs_model,
    ray_vs_model_within, selection_aabb, selection_aabb_oriented,
};

pub use geometry::cube_is_flat_plane;
pub use geometry::render_face_bias;
pub use placement::placement_transform_fp;

use compiled::models;
use geometry::{box_corners, cell_of, clip_to_cell, posed_cube_bounds, union_clip_to_cell};
use placement::oriented_cell_instance;

pub const DEFAULT_MODEL_FACING: Facing = Facing::North;

pub const PARTS_KV_KEY: &str = "petramond:parts";

pub const MAX_MODEL_PARTS: usize = 32;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct ModelCellState {
    pub offset: [u8; 3],
    pub facing: Facing,
}

impl Default for ModelCellState {
    fn default() -> Self {
        Self {
            offset: [0, 0, 0],
            facing: DEFAULT_MODEL_FACING,
        }
    }
}

impl crate::block::CellView for ModelCellState {
    fn owns(block: crate::block::Block) -> bool {
        block.model_kind().is_some()
    }
    fn from_cell(s: crate::block::ShapeState) -> Self {
        Self {
            offset: [s.byte(0), s.byte(1), s.byte(2)],
            facing: Facing::from_u8(s.byte(3)),
        }
    }
}
impl crate::block::CellCodec for ModelCellState {
    fn to_cell(&self) -> crate::block::ShapeState {
        crate::block::ShapeState::new(&[
            self.offset[0],
            self.offset[1],
            self.offset[2],
            self.facing.to_u8(),
        ])
    }
}
