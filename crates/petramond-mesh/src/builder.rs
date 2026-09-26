use petramond_world::chunk::SectionPos;
use petramond_world::content::{pin, Content};
use petramond_world::section::Section;
use petramond_world::texture_transition::Rules;

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
mod scratch;
mod transition;

pub use cell_class::MeshRegistry;
pub(super) use cube_face::face_axes;
pub(super) use lighting::{boundary_plane, CornerLight};
pub use pad::SectionMeshPad;
#[cfg(test)]
pub(super) use lighting::corner_cast_probes;

pub use closure_pad::WorldReads;
pub use foliage::FOLIAGE_OVERHANG;
pub use transition::{SamplingHalo, SAMPLING_HALO};

/// What a section mesh is built against: the block dispatch tables and the
/// texture-transition policy. Passed into every build rather than read from
/// process globals, so one process can mesh against several registries.
#[derive(Copy, Clone)]
pub struct MeshContext<'a> {
    /// The content whose block rows and texture rules this build uses.
    pub content: Content,
    pub registry: &'a MeshRegistry,
    pub rules: &'a Rules,
}

impl MeshContext<'static> {
    /// Build a mesh context for a specific content registry.
    pub fn for_content(content: Content) -> Self {
        let _pin = pin(content);
        Self {
            content,
            registry: MeshRegistry::global(),
            rules: petramond_world::texture_transition::rules(),
        }
    }

    /// A context for the registry selected on this thread.
    pub fn global() -> Self {
        Self::for_content(Content::current())
    }
}

/// Build the mesh for one cubic [`Section`] from world-coordinate reads: they
/// are sampled over the section's one-cell pad (see [`SectionMeshPad`]) and
/// the pad is meshed exactly as the live world's is, so reads beyond that pad
/// are never made. Out-of-world / unloaded reads return air / open sky as the
/// reads define. Block-entity state (furnace lit/facing, torch placement,
/// model offset/facing) is read from `section` directly. The renderer culls
/// the resulting mesh by its [`SectionPos`].
pub fn build_section_mesh(
    section: &Section,
    pos: SectionPos,
    ctx: MeshContext<'_>,
    reads: &WorldReads<'_>,
) -> ChunkMesh {
    let _pin = pin(ctx.content);
    let pad = closure_pad::ClosurePad::assemble(pos, reads);
    build_section_mesh_from_pad(section, pos, pad.view(), ctx)
}

/// [`build_section_mesh_cancellable`] that always finishes.
pub fn build_section_mesh_from_pad(
    section: &Section,
    pos: SectionPos,
    pad: SectionMeshPad<'_>,
    ctx: MeshContext<'_>,
) -> ChunkMesh {
    build_section_mesh_cancellable(section, pos, pad, ctx, &|| false).expect("uncancelled mesh")
}

/// Build one section's mesh from its assembled pad. Workers can abandon
/// superseded snapshots between section rows.
pub fn build_section_mesh_cancellable(
    section: &Section,
    pos: SectionPos,
    pad: SectionMeshPad<'_>,
    ctx: MeshContext<'_>,
    cancelled: &dyn Fn() -> bool,
) -> Option<ChunkMesh> {
    let _pin = pin(ctx.content);
    if cancelled() {
        return None;
    }
    let mesh = mesher::mesh_section(section, pos, &pad, ctx, cancelled, true)?;
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
    ctx: MeshContext<'_>,
    reads: &WorldReads<'_>,
    exposure_masks: bool,
) -> ChunkMesh {
    let _pin = pin(ctx.content);
    let pad = closure_pad::ClosurePad::assemble(pos, reads);
    mesher::mesh_section(section, pos, &pad.view(), ctx, &|| false, exposure_masks)
        .expect("uncancelled mesh")
}
