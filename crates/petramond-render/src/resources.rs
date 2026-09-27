mod arenas;
mod column_origins;
mod layers;
mod pack;
mod patch;
mod quad_index;
mod textures;

use super::geometry_arena::{GeometryArena, LayerAlloc};
pub(crate) use arenas::TerrainArenas;
pub(crate) use column_origins::{ColumnOriginSlot, ColumnOrigins, COLUMN_ORIGIN_LAYOUT};
pub(crate) use layers::{ColumnBuffer, SectionStream, Span};
pub(crate) use pack::TerrainUploadBatch;
use patch::try_patch_column;
use petramond_mesh::ChunkMesh;
use petramond_world::chunk::SectionPos;
pub(crate) use quad_index::QuadIndexBuffer;
pub(crate) use textures::{
    create_atlas, create_atlas_array, create_depth, create_depth_sampled, create_gui_panel,
    create_model_texture, create_rgba_nearest, create_scene_color, create_scene_color_sampled,
    create_sky_texture, create_solid_rgba_texture,
};

#[derive(Clone, Default)]
pub struct GpuSectionMesh {
    model_local_indices: std::sync::Arc<[u32]>,
    model_local_blend_indices: std::sync::Arc<[u32]>,
    pub origin: (i32, i32, i32),
    /// Every range this section owns, by [`SectionStream`]. Each stream's
    /// ranges form one per-column region, so a column drawn wholly at far LOD
    /// (every section's [`SectionStream::OpaqueFar`]) is ONE contiguous range,
    /// exactly as a column drawn wholly detailed is — without that, one
    /// distant leafy section forced its whole column onto per-section draws.
    pub spans: [Span; SectionStream::COUNT],
    pub has_far_lod: bool,
    pub far_lod_active: bool,
    pub index_hash: u64,
    pub visibility: petramond_mesh::SectionVisibility,
}

impl GpuSectionMesh {
    #[inline]
    pub fn span(&self, stream: SectionStream) -> Span {
        self.spans[stream.index()]
    }

    #[inline]
    pub fn has_model(&self) -> bool {
        !self.span(SectionStream::ModelIndices).is_empty()
            || !self.span(SectionStream::ModelBlendIndices).is_empty()
    }

    #[inline]
    pub fn has_alpha(&self) -> bool {
        [
            SectionStream::Transparent,
            SectionStream::TransparentTwoSided,
            SectionStream::Translucent,
        ]
        .iter()
        .any(|&stream| !self.span(stream).is_empty())
    }

    fn local_indices(&self, stream: SectionStream) -> &std::sync::Arc<[u32]> {
        match stream {
            SectionStream::ModelBlendIndices => &self.model_local_blend_indices,
            _ => &self.model_local_indices,
        }
    }
}

pub struct GpuColumnMesh {
    buffers: [Option<Layer>; ColumnBuffer::COUNT],
    regions: [u32; SectionStream::COUNT],
    pub origin_slot: ColumnOriginSlot,
    pub col_ox: i32,
    pub col_oz: i32,
    pub sections: Vec<(SectionPos, GpuSectionMesh)>,
    pub cy_span: (i32, i32),
}

impl GpuColumnMesh {
    pub fn column_pos(&self) -> petramond_world::chunk::ChunkPos {
        petramond_world::chunk::ChunkPos::new(self.col_ox >> 4, self.col_oz >> 4)
    }

    #[inline]
    pub fn buffer(&self, buffer: ColumnBuffer) -> Option<&Layer> {
        self.buffers[buffer.index()].as_ref()
    }

    pub fn layers(&self) -> impl Iterator<Item = (ColumnBuffer, &Layer)> {
        ColumnBuffer::ALL
            .into_iter()
            .filter_map(|buffer| self.buffer(buffer).map(|layer| (buffer, layer)))
    }

    pub fn region(&self, stream: SectionStream) -> Span {
        let start = SectionStream::ALL
            .iter()
            .take_while(|&&s| s != stream)
            .filter(|s| s.buffer() == stream.buffer())
            .map(|s| self.regions[s.index()])
            .sum();
        Span {
            start,
            count: self.regions[stream.index()],
        }
    }

    pub fn opaque_quads(&self) -> u32 {
        (self.regions[SectionStream::OpaqueFar.index()]
            + self.regions[SectionStream::OpaqueTail.index()])
            / 4
    }

    pub fn opaque_far_quads(&self) -> u32 {
        self.regions[SectionStream::OpaqueFar.index()] / 4
    }
}

pub(super) static TERRAIN_SUBALLOCS: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);

pub struct Layer {
    pub alloc: LayerAlloc,
    pub len: u64,
}

fn layer_fits(prev: &Layer, len: u64) -> bool {
    let cap = prev.alloc.capacity();
    let oversized = cap > 16 * 1024 && cap / 4 > len;
    cap >= len && !oversized
}

fn fresh_layer_alloc(device: &wgpu::Device, arena: &mut GeometryArena, len: u64) -> LayerAlloc {
    TERRAIN_SUBALLOCS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    arena.alloc(device, len)
}

fn cy_span(sections: &[(SectionPos, GpuSectionMesh)]) -> (i32, i32) {
    sections
        .iter()
        .fold((i32::MAX, i32::MIN), |(lo, hi), (sp, _)| {
            (lo.min(sp.cy), hi.max(sp.cy))
        })
}

/// Install a column's section meshes on the GPU. A column whose sections all
/// kept their stream counts and index topology (a light/AO remesh) is patched
/// in place; anything else is packed around the retained siblings of `prev`
/// (a fresh column has none). Either way every byte written is a copy of a
/// sealed mesh stream.
#[allow(clippy::too_many_arguments)]
pub(super) fn upload_column_mesh(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    meshes: &[(SectionPos, &ChunkMesh)],
    prev: Option<GpuColumnMesh>,
    origins: &mut ColumnOrigins,
    arenas: &mut TerrainArenas,
    quad_index: &mut QuadIndexBuffer,
    batch: &mut TerrainUploadBatch,
) -> GpuColumnMesh {
    if let Some(g) = &prev {
        if try_patch_column(queue, arenas, meshes, g) {
            return prev.expect("checked above");
        }
    }
    pack::pack_column(
        device, queue, arenas, quad_index, origins, meshes, prev, batch,
    )
}

#[cfg(test)]
mod tests;
