//! GPU-side terrain columns: how a column's section meshes are packed into
//! arena buffers, patched in place, or repacked around retained siblings.
//! Every step iterates the layer table in [`layers`]; the mesh worker has
//! already sealed each section into the GPU vertex format, so the render
//! thread only copies bytes. Texture uploads, the shared quad index buffer and
//! the column-origin table live in the sibling modules.

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
    /// The section-local index streams, kept so a repack can rebase them
    /// after the CPU mesh is released (see [`SectionStream::is_index`]).
    model_local_indices: std::sync::Arc<[u32]>,
    model_local_blend_indices: std::sync::Arc<[u32]>,
    /// World-space minimum corner `(x, y, z)` of this section.
    pub origin: (i32, i32, i32),
    /// Every range this section owns, by [`SectionStream`]. Each stream's
    /// ranges form one per-column region, so a column drawn wholly at far LOD
    /// (every section's [`SectionStream::OpaqueFar`]) is ONE contiguous range,
    /// exactly as a column drawn wholly detailed is — without that, one
    /// distant leafy section forced its whole column onto per-section draws.
    pub spans: [Span; SectionStream::COUNT],
    /// Whether this section HAS a far LOD (i.e. its tail is non-empty). Kept
    /// as its own bit so the planner need not re-derive it.
    pub has_far_lod: bool,
    /// Whether this section drew its far (simplified-canopy) LOD on the last
    /// planned frame — the hysteresis input of [`far_leaf_lod_active`]. It
    /// lives on the section rather than in a side map because the planner asks
    /// it for every visible far-capable section every frame, and because a
    /// section that goes away must take its LOD state with it. An incremental
    /// repack clones the record, so the state survives one.
    ///
    /// [`far_leaf_lod_active`]: crate::renderer::far_leaf_lod_active
    pub far_lod_active: bool,
    /// Fingerprint of the section-local index streams (see
    /// `section_index_hash`). Guards the vertex-only patch path: equal stream
    /// counts pin every start offset, but NOT the index topology — faces can
    /// migrate between the inline and deferred-greedy partitions as their
    /// merge keys change, so two meshes with identical counts can wire
    /// vertices differently. Patching vertices under stale indices collapses
    /// such triangles.
    pub index_hash: u64,
    /// Which of the section's faces see each other through it (from the
    /// mesh worker): the occlusion flood's graph edge data.
    pub visibility: petramond_mesh::SectionVisibility,
}

impl GpuSectionMesh {
    #[inline]
    pub fn span(&self, stream: SectionStream) -> Span {
        self.spans[stream.index()]
    }

    /// Whether the section owns any geometry drawn by the model passes.
    #[inline]
    pub fn has_model(&self) -> bool {
        !self.span(SectionStream::ModelIndices).is_empty()
            || !self.span(SectionStream::ModelBlendIndices).is_empty()
    }

    /// Whether the section owns any geometry of the alpha passes (fluids and
    /// translucent blocks).
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

    /// The section-local indices behind an index stream.
    fn local_indices(&self, stream: SectionStream) -> &std::sync::Arc<[u32]> {
        match stream {
            SectionStream::ModelBlendIndices => &self.model_local_blend_indices,
            _ => &self.model_local_indices,
        }
    }
}

pub struct GpuColumnMesh {
    /// The column's arena buffers, by [`ColumnBuffer`].
    buffers: [Option<Layer>; ColumnBuffer::COUNT],
    /// Elements in each stream's column-wide region, by [`SectionStream`].
    regions: [u32; SectionStream::COUNT],
    /// This column's slot in the shared instance-step origin table
    /// ([`ColumnOrigins`]); `vs_terrain` reads `[ox, 0, oz, 0]` from it via the
    /// draw's `first_instance`.
    pub origin_slot: ColumnOriginSlot,
    pub col_ox: i32,
    pub col_oz: i32,
    pub sections: Vec<(SectionPos, GpuSectionMesh)>,
    /// `(min_cy, max_cy)` over `sections`, or `(i32::MAX, i32::MIN)` when the
    /// column holds none. The whole-column cull AABB derives from it, so it is
    /// stored rather than re-folded over the section list every frame.
    pub cy_span: (i32, i32),
}

impl GpuColumnMesh {
    /// The column's own chunk position, from the world origin it was packed
    /// with — so a column can name itself without the map that stores it.
    pub fn column_pos(&self) -> petramond_world::chunk::ChunkPos {
        petramond_world::chunk::ChunkPos::new(self.col_ox >> 4, self.col_oz >> 4)
    }

    #[inline]
    pub fn buffer(&self, buffer: ColumnBuffer) -> Option<&Layer> {
        self.buffers[buffer.index()].as_ref()
    }

    /// Every live buffer, for the memory census.
    pub fn layers(&self) -> impl Iterator<Item = (ColumnBuffer, &Layer)> {
        ColumnBuffer::ALL
            .into_iter()
            .filter_map(|buffer| self.buffer(buffer).map(|layer| (buffer, layer)))
    }

    /// A stream's whole column-wide region inside its buffer: regions of one
    /// buffer follow each other in [`SectionStream::ALL`] order.
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

    /// Quads in the column's whole opaque stream (far region + leaf tails):
    /// the detailed whole-column draw.
    pub fn opaque_quads(&self) -> u32 {
        (self.regions[SectionStream::OpaqueFar.index()]
            + self.regions[SectionStream::OpaqueTail.index()])
            / 4
    }

    /// Quads of the column's leading FAR region (every section's far LOD, in
    /// section order): the whole-column far-LOD draw.
    pub fn opaque_far_quads(&self) -> u32 {
        self.regions[SectionStream::OpaqueFar.index()] / 4
    }
}

/// Fresh arena suballocations since process start, counted on every path
/// that claims one ([`fresh_layer_alloc`]). A sizing or reuse policy change
/// trades VRAM against this number, so it is measurable rather than argued.
pub(super) static TERRAIN_SUBALLOCS: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);

/// One column buffer's live geometry: where it sits in the arena and how many
/// bytes of it are in use. The allocation's size-class capacity is usually
/// larger, and that rounding IS the growth headroom a remesh writes into
/// (see [`layer_fits`]).
pub struct Layer {
    pub alloc: LayerAlloc,
    pub len: u64,
}

/// Can `len` bytes be written into `prev`'s allocation in place? Yes while
/// they fit, unless the allocation is now wildly oversized (the player mined
/// out most of the column / far LOD replaced dense foliage) — then it goes
/// back to the arena rather than pinning VRAM. The class rounding IS the
/// growth headroom; releasing only past a 4× hysteresis means a dug-out column
/// returns its VRAM but size jitter never churns the free lists.
fn layer_fits(prev: &Layer, len: u64) -> bool {
    let cap = prev.alloc.capacity();
    let oversized = cap > 16 * 1024 && cap / 4 > len;
    cap >= len && !oversized
}

/// The ONE path that claims arena space for a column buffer, so
/// [`TERRAIN_SUBALLOCS`] counts every allocation whichever upload took it.
fn fresh_layer_alloc(device: &wgpu::Device, arena: &mut GeometryArena, len: u64) -> LayerAlloc {
    TERRAIN_SUBALLOCS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    arena.alloc(device, len)
}

/// `(min_cy, max_cy)` over a column's installed sections, inverted when empty.
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
