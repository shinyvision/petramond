//! Chunk meshing: per-face culling, opaque + transparent passes, atlas UVs.
//!
//! Lighting is `directional face shade x per-vertex ambient occlusion`: the
//! face-direction `SHADES` factor (top brightest, bottom darkest) is modulated
//! by a "smooth lighting" AO term baked per vertex from the
//! solid neighbours around each corner. The shader interpolates the per-vertex
//! AO across the face, giving the soft contact shadows in nooks and against
//! adjacent blocks.

mod builder;
pub(crate) mod face;
#[cfg(test)]
mod skylight;
mod tint;
mod torch;
mod vertex;
mod water;

pub use builder::build_section_mesh;
#[cfg(test)]
pub use builder::{build_mesh, build_mesh_lods_with_loaded_neighbors};
#[cfg(test)]
pub use builder::{build_mesh_with_options, MeshOptions};
#[cfg(test)]
pub use skylight::{compute_chunk_skylight, compute_chunk_skylight_with_neighbors};
pub use vertex::{ChunkMesh, ModelVertex, Vertex, SHADES};

#[cfg(test)]
mod tests;
