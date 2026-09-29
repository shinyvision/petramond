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
pub(crate) mod scratch;
mod transition;

pub use cell_class::MeshRegistry;
pub(super) use cube_face::face_axes;
#[cfg(test)]
pub(super) use lighting::corner_cast_probes;
pub(super) use lighting::{boundary_plane, CornerLight};
pub use pad::SectionMeshPad;

pub use closure_pad::WorldReads;
pub use foliage::FOLIAGE_OVERHANG;
pub use transition::{SamplingHalo, SAMPLING_HALO};

#[derive(Copy, Clone)]
pub struct MeshContext<'a> {
    pub content: Content,
    pub registry: &'a MeshRegistry,
    pub rules: &'a Rules,
}

impl MeshContext<'static> {
    pub fn for_content(content: Content) -> Self {
        let _pin = pin(content);
        Self {
            content,
            registry: MeshRegistry::global(),
            rules: petramond_world::texture_transition::rules(),
        }
    }

    pub fn global() -> Self {
        Self::for_content(Content::current())
    }
}

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

pub fn build_section_mesh_from_pad(
    section: &Section,
    pos: SectionPos,
    pad: SectionMeshPad<'_>,
    ctx: MeshContext<'_>,
) -> ChunkMesh {
    build_section_mesh_cancellable(section, pos, pad, ctx, &|| false).expect("uncancelled mesh")
}

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
