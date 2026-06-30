//! Chunk meshing: per-face culling, opaque + transparent passes, atlas UVs.
//!
//! Lighting is `directional face shade x per-vertex ambient occlusion`: the
//! face-direction `SHADES` factor (top brightest, bottom darkest) is modulated
//! by a "smooth lighting" AO term baked per vertex from the
//! solid neighbours around each corner. The shader interpolates the per-vertex
//! AO across the face, giving the soft contact shadows in nooks and against
//! adjacent blocks.

mod blocklight;
mod builder;
pub(crate) mod face;
mod skylight;
mod tint;
mod torch;
mod vertex;
mod water;

// TODO(S5): the column-era block-light flood, kept (and tested) for the cubic
// cross-section block-light rewrite; not wired into `build_section_mesh` yet.
#[allow(unused_imports)]
pub use blocklight::compute_chunk_blocklight_with_neighbors;
pub use builder::build_section_mesh;
#[allow(unused_imports)]
// build_mesh* are now used only by tests during the cubic migration (S10 cleanup)
pub use builder::{build_mesh, build_mesh_lods_with_loaded_neighbors};
#[cfg(test)]
pub use builder::{build_mesh_with_options, MeshOptions};
#[cfg(test)]
pub use skylight::compute_chunk_skylight;
pub use skylight::compute_chunk_skylight_with_neighbors;
pub use vertex::{ChunkMesh, ModelVertex, Vertex, SHADES};

#[cfg(test)]
mod tests;
