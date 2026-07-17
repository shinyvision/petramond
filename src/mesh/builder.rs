use crate::block::Block;
#[cfg(test)]
use crate::block_state::{SlabState, StairState};
use crate::chunk::SectionPos;
use crate::section::Section;

use super::tint;
use super::vertex::ChunkMesh;
use super::{ladder, pane, slab, stair, torch};

mod cube_face;
mod exposed_masks;
mod geometry;
mod model_block;
mod pad;
mod plant;

pub(super) use cube_face::{cube_face_lighting, face_axes};
pub(super) use pad::mesh_pad_idx;
pub(crate) use pad::SectionMeshPad;

use geometry::section_geometry;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum LeafMeshMode {
    Detailed,
    Simplified,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct MeshOptions {
    pub leaf_mesh_mode: LeafMeshMode,
}

impl MeshOptions {
    pub const DETAILED: Self = Self {
        leaf_mesh_mode: LeafMeshMode::Detailed,
    };

    pub const FAR_LEAVES: Self = Self {
        leaf_mesh_mode: LeafMeshMode::Simplified,
    };
}

/// Build the mesh for one cubic [`Section`]. All neighbour lookups are by WORLD
/// coordinate and route to the owning section (including this one), so the same
/// closure handles in-section and cross-section reads; out-of-world / unloaded
/// reads return air / open-sky as the closures define. Block-entity state (furnace
/// lit/facing, torch placement, model offset/facing) is read from `section`
/// directly. The renderer culls the resulting mesh by its [`SectionPos`].
#[allow(clippy::too_many_arguments)]
#[cfg(test)]
pub fn build_section_mesh(
    section: &Section,
    pos: SectionPos,
    neighbour_block: impl Fn(i32, i32, i32) -> u8,
    neighbour_stair_state: impl Fn(i32, i32, i32) -> StairState,
    neighbour_slab_state: impl Fn(i32, i32, i32) -> SlabState,
    neighbour_water: impl Fn(i32, i32, i32) -> u8,
    neighbour_biome: impl Fn(i32, i32) -> u8,
    neighbour_light: impl Fn(i32, i32, i32) -> u8,
    neighbour_blocklight: impl Fn(i32, i32, i32) -> u8,
    neighbour_loaded: impl Fn(i32, i32, i32) -> bool,
) -> ChunkMesh {
    let tints = section.has_biome_tint_blocks().then(|| {
        let (ox, _, oz) = pos.origin_world();
        tint::biome_window(ox, oz, &neighbour_biome)
    });
    let mut mesh = section_geometry(
        section,
        pos,
        &neighbour_block,
        &neighbour_stair_state,
        &neighbour_slab_state,
        &neighbour_water,
        &neighbour_light,
        &neighbour_blocklight,
        &neighbour_loaded,
        tints.as_ref(),
        MeshOptions::DETAILED,
        None,
    );
    if !section.blocks_slice().contains(&Block::OakLeaves.id()) {
        return mesh;
    }
    let far = section_geometry(
        section,
        pos,
        &neighbour_block,
        &neighbour_stair_state,
        &neighbour_slab_state,
        &neighbour_water,
        &neighbour_light,
        &neighbour_blocklight,
        &neighbour_loaded,
        tints.as_ref(),
        MeshOptions::FAR_LEAVES,
        None,
    );
    if far.opaque_idx.len() < mesh.opaque_idx.len() {
        mesh.far_opaque = far.opaque;
        mesh.far_opaque_idx = far.opaque_idx;
    }
    mesh
}

pub(crate) fn build_section_mesh_from_pad(
    section: &Section,
    pos: SectionPos,
    pad: SectionMeshPad<'_>,
) -> ChunkMesh {
    let (ox, oy, oz) = pos.origin_world();
    let nb_block = |wx, wy, wz| pad.block_world(ox, oy, oz, wx, wy, wz);
    let nb_stair_state = |wx, wy, wz| pad.stair_world(ox, oy, oz, wx, wy, wz);
    let nb_slab_state = |wx, wy, wz| pad.slab_world(ox, oy, oz, wx, wy, wz);
    let nb_water = |wx, wy, wz| pad.water_world(ox, oy, oz, wx, wy, wz);
    let nb_biome = |wx, wz| pad.biome_world(ox, oz, wx, wz);
    let nb_skylight = |wx, wy, wz| pad.skylight_world(ox, oy, oz, wx, wy, wz);
    let nb_blocklight = |wx, wy, wz| pad.blocklight_world(ox, oy, oz, wx, wy, wz);
    let nb_loaded = |wx, wy, wz| pad.loaded_world(ox, oy, oz, wx, wy, wz);
    let tints = section
        .has_biome_tint_blocks()
        .then(|| tint::biome_window(ox, oz, nb_biome));
    let mut mesh = section_geometry(
        section,
        pos,
        nb_block,
        nb_stair_state,
        nb_slab_state,
        nb_water,
        nb_skylight,
        nb_blocklight,
        nb_loaded,
        tints.as_ref(),
        MeshOptions::DETAILED,
        Some(&pad),
    );
    if !section.blocks_slice().contains(&Block::OakLeaves.id()) {
        return mesh;
    }
    let far = section_geometry(
        section,
        pos,
        nb_block,
        nb_stair_state,
        nb_slab_state,
        nb_water,
        nb_skylight,
        nb_blocklight,
        nb_loaded,
        tints.as_ref(),
        MeshOptions::FAR_LEAVES,
        None,
    );
    if far.opaque_idx.len() < mesh.opaque_idx.len() {
        mesh.far_opaque = far.opaque;
        mesh.far_opaque_idx = far.opaque_idx;
    }
    mesh
}
