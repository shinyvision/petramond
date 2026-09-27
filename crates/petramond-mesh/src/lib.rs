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
pub mod visibility;

pub use builder::{
    build_section_mesh, build_section_mesh_cancellable, build_section_mesh_from_pad, MeshContext,
    MeshRegistry, SectionMeshPad, WorldReads,
};
pub use builder::{SamplingHalo, FOLIAGE_OVERHANG, SAMPLING_HALO};
#[cfg(test)]
pub use skylight::{compute_chunk_skylight, compute_chunk_skylight_with_neighbors};
pub use vertex::transition::UV_MODE_TRANSITION;
pub use vertex::MAX_TILES;
pub use vertex::{
    pack_cell_uv, UV_MODE_CELL_LOCAL, UV_MODE_NONE, UV_MODE_SHIFT, UV_MODE_THIN_U, UV_MODE_THIN_V,
};
pub use vertex::{pack_overlay, AO_SHIFT, CORNER_SHIFT, OVERLAY_FLAG, SHADE_SHIFT, SKY_SHIFT};
pub use vertex::{pack_tint, retint, unpack_tint, DYED_FLAG2};
pub use vertex::{
    ChunkMesh, ContactShadowVertex, ModelVertex, QuadLayer, TerrainVertex, Vertex, SHADES,
};
pub use vertex::{FLUID_FLOW_FLAG2, FLUID_MEDIUM_MASK, FLUID_MEDIUM_SHIFT, MAX_FLUID_MEDIA};
pub use visibility::SectionVisibility;

#[cfg(test)]
mod tests;
