use petramond_world::block::ShapeState;
use petramond_world::chunk::SectionPos;
use petramond_world::section::Section;

use super::torch;
use super::vertex::ChunkMesh;

mod cell_class;
mod cell_tint;
mod closure_pad;
mod cube;
mod cube_face;
mod exposed_masks;
mod families;
mod fluid_faces;
mod foliage;
mod lighting;
mod mesher;
mod model_block;
mod neighbourhood;
mod pad;
mod plant;
mod transition;

pub(super) use cube_face::face_axes;
pub(super) use lighting::{boundary_plane, CornerLight};
pub use pad::SectionMeshPad;
#[cfg(test)]
pub(super) use lighting::corner_cast_probes;

pub(crate) use closure_pad::WorldReads;
pub use foliage::FOLIAGE_OVERHANG;
pub use transition::{SamplingHalo, SAMPLING_HALO};

/// Build the mesh for one cubic [`Section`] from world-coordinate closures:
/// they are sampled over the section's one-cell pad (see
/// [`SectionMeshPad`]) and the pad is meshed exactly as the live world's is,
/// so reads beyond that pad are never made. Out-of-world / unloaded reads
/// return air / open sky as the closures define. Block-entity state (furnace
/// lit/facing, torch placement, model offset/facing) is read from `section`
/// directly. `neighbour_dyed` answers whether a cell carries a dye tint (a
/// transition exclusion) — for neighbour sections too, exactly as the live
/// pad scatters their tint maps. The renderer culls the resulting mesh by its
/// [`SectionPos`].
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
    let pad = closure_pad::ClosurePad::assemble(
        pos,
        &WorldReads {
            block: &neighbour_block,
            cell_state: &neighbour_cell_state,
            fluid_meta: &neighbour_fluid_meta,
            biome: &neighbour_biome,
            skylight: &neighbour_light,
            blocklight: &neighbour_blocklight,
            loaded: &neighbour_loaded,
            dyed: &neighbour_dyed,
        },
    );
    build_section_mesh_from_pad(section, pos, pad.view(), rules)
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
/// Workers can abandon superseded snapshots between section rows.
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
    let mesh = mesher::mesh_section(section, pos, &pad, rules, cancelled, true)?;
    (!cancelled()).then_some(mesh)
}

/// Mesh the section at `pos` of the world `reads` describe, with the
/// exposure-mask fast path on (the production build) or off (`false`: every
/// cube face culled by asking its front cell). The two must match byte for
/// byte — the reference the parity tests hold the fast path to.
#[cfg(test)]
pub(super) fn build_section_mesh_with(
    section: &Section,
    pos: SectionPos,
    rules: &petramond_world::texture_transition::Rules,
    reads: &WorldReads<'_>,
    exposure_masks: bool,
) -> ChunkMesh {
    let pad = closure_pad::ClosurePad::assemble(pos, reads);
    mesher::mesh_section(section, pos, &pad.view(), rules, &|| false, exposure_masks)
        .expect("uncancelled mesh")
}
