//! Chunk meshing: per-face culling, opaque + transparent passes, atlas UVs.
//!
//! Lighting is `directional face shade x per-vertex ambient occlusion`: the
//! face-direction `SHADES` factor (top brightest, bottom darkest) is modulated
//! by a "smooth lighting" AO term baked per vertex from the
//! solid neighbours around each corner. The shader interpolates the per-vertex
//! AO across the face, giving the soft contact shadows in nooks and against
//! adjacent blocks.

mod boxset;
mod builder;
pub(crate) mod face;
mod face_emit;
pub(crate) mod fence;
mod greedy;
pub(crate) mod ladder;
pub(crate) mod pane;
pub(crate) mod plane;
#[cfg(test)]
mod skylight;
pub(crate) mod slab;
mod tint;
mod torch;
pub(crate) mod vertex;
mod water;

#[cfg(test)]
pub use builder::build_section_mesh;
pub(crate) use builder::{build_section_mesh_from_pad, SectionMeshPad};
#[cfg(test)]
pub use skylight::{compute_chunk_skylight, compute_chunk_skylight_with_neighbors};
pub(crate) use vertex::{
    pack_cell_uv, UV_MODE_CELL_LOCAL, UV_MODE_SHIFT, UV_MODE_THIN_U, UV_MODE_THIN_V,
};
// The `Vertex::packed` bit layout, re-exported so the dynamic-geometry bakes
// (`render::item_cube`, `render::lighting`) encode it through the SAME
// constants the chunk mesher does instead of their own literals.
pub use vertex::MAX_TILES;
pub(crate) use vertex::{
    pack_overlay, AO_SHIFT, CORNER_SHIFT, OVERLAY_FLAG, SHADE_SHIFT, SKY_SHIFT,
};
pub(crate) use vertex::{pack_tint, retint, unpack_tint, DYED_FLAG2};
pub use vertex::{ChunkMesh, ContactShadowVertex, ModelVertex, TerrainVertex, Vertex, SHADES};

#[cfg(test)]
mod tests;
