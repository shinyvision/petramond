//! Chunk meshing: per-face culling, opaque + transparent passes, atlas UVs.
//!
//! Lighting is `directional face shade x per-vertex ambient occlusion`: the
//! face-direction `SHADES` factor (top brightest, bottom darkest) is modulated
//! by a "smooth lighting" AO term baked per vertex from the
//! solid neighbours around each corner. The shader interpolates the per-vertex
//! AO across the face, giving the soft contact shadows in nooks and against
//! adjacent blocks.

#![allow(clippy::too_many_arguments)]

mod boxset;
mod builder;
pub mod face;
mod face_emit;
pub use petramond_world::shape_mesh::fence;
mod greedy;
pub use petramond_world::shape_mesh::ladder;
pub use petramond_world::shape_mesh::pane;
pub mod plane;
#[cfg(test)]
mod skylight;
pub use petramond_world::shape_mesh::slab;
mod fluid;
mod tint;
mod torch;
pub mod vertex;

pub use builder::{
    build_section_mesh, build_section_mesh_cancellable, build_section_mesh_from_pad, SectionMeshPad,
};
pub use builder::{SamplingHalo, FOLIAGE_OVERHANG, SAMPLING_HALO};
#[cfg(test)]
pub use skylight::{compute_chunk_skylight, compute_chunk_skylight_with_neighbors};
pub use vertex::transition::UV_MODE_TRANSITION;
pub use vertex::{
    pack_cell_uv, UV_MODE_CELL_LOCAL, UV_MODE_NONE, UV_MODE_SHIFT, UV_MODE_THIN_U, UV_MODE_THIN_V,
};
// The `Vertex::packed` bit layout, re-exported so the dynamic-geometry bakes
// (`render::item_cube`, `render::lighting`) encode it through the SAME
// constants the chunk mesher does instead of their own literals.
pub use vertex::MAX_TILES;
pub use vertex::{pack_overlay, AO_SHIFT, CORNER_SHIFT, OVERLAY_FLAG, SHADE_SHIFT, SKY_SHIFT};
pub use vertex::{pack_tint, retint, unpack_tint, DYED_FLAG2};
pub use vertex::{ChunkMesh, ContactShadowVertex, ModelVertex, TerrainVertex, Vertex, SHADES};
pub use vertex::{FLUID_FLOW_FLAG2, FLUID_MEDIUM_MASK, FLUID_MEDIUM_SHIFT, MAX_FLUID_MEDIA};

#[cfg(test)]
mod tests;
