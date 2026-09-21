use petramond_world::block::Block;
use petramond_world::block::ShapeState;
use petramond_world::chunk::SectionPos;
use petramond_world::section::Section;

use super::tint;
use super::torch;
use super::vertex::ChunkMesh;

mod cell_class;
mod cube_face;
mod exposed_masks;
mod fluid_faces;
mod foliage;
mod geometry;
mod model_block;
mod pad;
mod plant;
mod transition;

pub(super) use cube_face::{
    boundary_plane, corner_cast_probes, cube_face_lighting, face_axes, probe_worthy,
};
pub use pad::SectionMeshPad;
pub(super) use pad::{mesh_pad_idx, MESH_PAD_SIDE};

pub use foliage::FOLIAGE_OVERHANG;
use geometry::section_geometry;
pub use transition::{SamplingHalo, SAMPLING_HALO};

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

/// Build the mesh for one cubic [`Section`] from world-coordinate closures. All
/// neighbour lookups route to the owning section (including this one), so the
/// same closure handles in-section and cross-section reads; out-of-world /
/// unloaded reads return air / open-sky as the closures define. Block-entity
/// state (furnace lit/facing, torch placement, model offset/facing) is read
/// from `section` directly. `neighbour_dyed` answers whether a cell carries a
/// dye tint (a transition exclusion) — for neighbour sections too, exactly as
/// the pad mesher scatters their tint maps. The renderer culls the resulting
/// mesh by its [`SectionPos`].
#[allow(clippy::too_many_arguments)]
pub fn build_section_mesh(
    section: &Section,
    pos: SectionPos,
    rules: &petramond_world::texture_transition::Rules,
    neighbour_block: impl Fn(i32, i32, i32) -> u16,
    neighbour_cell_state: impl Fn(i32, i32, i32) -> ShapeState,
    neighbour_fluid_meta: impl Fn(i32, i32, i32) -> u8,
    neighbour_biome: impl Fn(i32, i32) -> u8,
    neighbour_light: impl Fn(i32, i32, i32) -> u8,
    neighbour_blocklight: impl Fn(i32, i32, i32) -> petramond_world::light::LightRgb,
    neighbour_loaded: impl Fn(i32, i32, i32) -> bool,
    neighbour_dyed: impl Fn(i32, i32, i32) -> bool,
) -> ChunkMesh {
    let tints = transition::needs_tint(section, rules).then(|| {
        let (ox, _, oz) = pos.origin_world();
        tint::biome_window(ox, oz, &neighbour_biome)
    });
    let blocked = |x: i32, y: i32, z: i32| {
        neighbour_dyed(x, y, z)
            || petramond_world::block::snow_cover_at(
                glam::IVec3::new(x, y + petramond_world::block::SNOW_COVER_REACH, z),
                |p| Block::from_id(neighbour_block(p.x, p.y, p.z)),
            )
            .is_some()
    };
    let mut mesh = section_geometry(
        section,
        pos,
        &neighbour_block,
        &neighbour_cell_state,
        &neighbour_fluid_meta,
        &neighbour_light,
        &neighbour_blocklight,
        &neighbour_loaded,
        &blocked,
        rules,
        tints.as_ref(),
        MeshOptions::DETAILED,
        None,
        &|| false,
    );
    if !section.blocks_iter().any(|id| Block(id).is_leaves()) {
        return mesh;
    }
    let far = section_geometry(
        section,
        pos,
        &neighbour_block,
        &neighbour_cell_state,
        &neighbour_fluid_meta,
        &neighbour_light,
        &neighbour_blocklight,
        &neighbour_loaded,
        &blocked,
        rules,
        tints.as_ref(),
        MeshOptions::FAR_LEAVES,
        None,
        &|| false,
    );
    if far.opaque.len() < mesh.opaque.len() {
        mesh.far_opaque = far.opaque;
    }
    mesh
}

/// [`build_section_mesh_cancellable`] that always finishes.
pub fn build_section_mesh_from_pad(
    section: &Section,
    pos: SectionPos,
    pad: SectionMeshPad<'_>,
    rules: &petramond_world::texture_transition::Rules,
) -> ChunkMesh {
    build_section_mesh_cancellable(section, pos, pad, rules, &|| false).expect("uncancelled mesh")
}

/// Build one section's mesh from its assembled pad under a transition policy.
/// Workers can abandon superseded snapshots between section rows and LOD passes.
pub fn build_section_mesh_cancellable(
    section: &Section,
    pos: SectionPos,
    pad: SectionMeshPad<'_>,
    rules: &petramond_world::texture_transition::Rules,
    cancelled: &dyn Fn() -> bool,
) -> Option<ChunkMesh> {
    if cancelled() {
        return None;
    }
    let (ox, oy, oz) = pos.origin_world();
    let nb_block = |wx, wy, wz| pad.block_world(ox, oy, oz, wx, wy, wz);
    let nb_cell_state = |wx, wy, wz| pad.cell_state_world(ox, oy, oz, wx, wy, wz);
    let nb_fluid_meta = |wx, wy, wz| pad.fluid_meta_world(ox, oy, oz, wx, wy, wz);
    let nb_biome = |wx, wz| pad.biome_world(ox, oz, wx, wz);
    let nb_skylight = |wx, wy, wz| pad.skylight_world(ox, oy, oz, wx, wy, wz);
    let nb_blocklight = |wx, wy, wz| pad.blocklight_world(ox, oy, oz, wx, wy, wz);
    let blocked = |wx, wy, wz| pad.transition_blocked_world(ox, oy, oz, wx, wy, wz);
    let nb_loaded = |wx, wy, wz| pad.loaded_world(ox, oy, oz, wx, wy, wz);
    let tints =
        transition::needs_tint(section, rules).then(|| tint::biome_window(ox, oz, nb_biome));
    let mut mesh = section_geometry(
        section,
        pos,
        nb_block,
        nb_cell_state,
        nb_fluid_meta,
        nb_skylight,
        nb_blocklight,
        nb_loaded,
        &blocked,
        rules,
        tints.as_ref(),
        MeshOptions::DETAILED,
        Some(&pad),
        cancelled,
    );
    if cancelled() {
        return None;
    }
    if !section.blocks_iter().any(|id| Block(id).is_leaves()) {
        return (!cancelled()).then_some(mesh);
    }
    let far = section_geometry(
        section,
        pos,
        nb_block,
        nb_cell_state,
        nb_fluid_meta,
        nb_skylight,
        nb_blocklight,
        nb_loaded,
        &blocked,
        rules,
        tints.as_ref(),
        MeshOptions::FAR_LEAVES,
        None,
        cancelled,
    );
    if far.opaque.len() < mesh.opaque.len() {
        mesh.far_opaque = far.opaque;
    }
    (!cancelled()).then_some(mesh)
}
