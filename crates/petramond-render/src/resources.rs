//! GPU-side terrain columns: how a column's section meshes are packed into
//! arena layers, patched in place, or repacked around retained siblings.
//! Texture uploads, the shared quad index buffer and the column-origin table
//! live in the sibling modules.

mod column_origins;
mod incremental;
mod patch;
mod quad_index;
mod textures;

use super::geometry_arena::{GeometryArena, LayerAlloc};
pub(crate) use column_origins::{ColumnOriginSlot, ColumnOrigins};
pub(crate) use incremental::TerrainUploadBatch;
use patch::{section_index_hash, try_patch_column_verts};
use petramond_mesh::{ChunkMesh, ContactShadowVertex, ModelVertex, TerrainVertex, Vertex};
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
    /// World-space minimum corner `(x, y, z)` of this section.
    pub origin: (i32, i32, i32),
    pub opaque_vertex_start: u32,
    pub opaque_vertex_count: u32,
    pub far_opaque_vertex_start: u32,
    pub far_opaque_vertex_count: u32,
    pub transparent_vertex_start: u32,
    pub transparent_vertex_count: u32,
    /// The cull-none fluid-top stream (see [`petramond_mesh::ChunkMesh`]).
    pub transparent_ts_vertex_start: u32,
    pub transparent_ts_vertex_count: u32,
    pub translucent_vertex_start: u32,
    pub translucent_vertex_count: u32,
    pub model_index_start: u32,
    pub model_idx_count: u32,
    /// The section's alpha-BLEND model faces: an index range into the same
    /// column index buffer, in the blend region that follows every section's
    /// opaque indices (so the column's whole blend region stays contiguous).
    pub model_blend_index_start: u32,
    pub model_blend_idx_count: u32,
    pub model_vertex_start: u32,
    pub model_vertex_count: u32,
    /// Contact-shadow VERTEX range (the stream is non-indexed). Kept per section
    /// only so `plan_draw_order` can decide column contact visibility from the
    /// VISIBLE sections — the draw itself is whole-column. A section may hold
    /// contact vertices with `model_idx_count == 0` (a multi-cell model whose
    /// cuboids all render from a sibling cell), so contact visibility must NOT
    /// be inferred from the model range.
    pub contact_vertex_start: u32,
    pub contact_vertex_count: u32,
    /// Fingerprint of the section-local index streams (see
    /// `section_index_hash`). Guards the vertex-only patch path: equal layer
    /// counts pin every start offset, but NOT the index topology — plant quads
    /// (12 indices per 4 vertices) interleave with inline cube faces in cell
    /// order, and faces migrate between the inline and deferred-greedy
    /// partitions as their merge keys change, so two meshes with identical
    /// counts can wire vertices differently. Patching vertices under stale
    /// indices collapses such quads to degenerate triangles.
    pub index_hash: u64,
}

pub struct GpuColumnMesh {
    pub opaque_vbuf: Option<Layer>,
    /// Quads in the column's opaque stream — the draw's index count is six per
    /// quad against the shared quad index buffer.
    pub opaque_quads: u32,
    pub far_opaque_vbuf: Option<Layer>,
    pub transparent_vbuf: Option<Layer>,
    pub transparent_ts_vbuf: Option<Layer>,
    pub translucent_vbuf: Option<Layer>,
    pub model_vbuf: Option<Layer>,
    pub model_ibuf: Option<Layer>,
    pub model_idx_count: u32,
    /// The column's whole alpha-BLEND model index region, appended after the
    /// last opaque index (`model_idx_count .. model_idx_count + this`): one
    /// contiguous range so the blend pass can batch per column like the model
    /// pass does.
    pub model_blend_idx_count: u32,
    /// The column's whole contact-shadow stream (non-indexed 16-byte
    /// `ContactShadowVertex`), drawn once per visible contact-bearing column.
    pub contact_vbuf: Option<Layer>,
    pub contact_vertex_count: u32,
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

#[derive(Default)]
pub(super) struct ColumnUploadScratch {
    opaque: Vec<TerrainVertex>,
    far_opaque: Vec<TerrainVertex>,
    transparent: Vec<TerrainVertex>,
    transparent_two_sided: Vec<TerrainVertex>,
    translucent: Vec<TerrainVertex>,
    model: Vec<ModelVertex>,
    model_idx: Vec<u32>,
    model_blend_idx: Vec<u32>,
    contact: Vec<ContactShadowVertex>,
}

impl ColumnUploadScratch {
    fn clear(&mut self) {
        self.opaque.clear();
        self.far_opaque.clear();
        self.transparent.clear();
        self.transparent_two_sided.clear();
        self.translucent.clear();
        self.model.clear();
        self.model_idx.clear();
        self.model_blend_idx.clear();
        self.contact.clear();
    }

    fn reserve_for(&mut self, meshes: &[(SectionPos, &ChunkMesh)]) {
        self.opaque
            .reserve(meshes.iter().map(|(_, mesh)| mesh.opaque.len()).sum());
        self.far_opaque
            .reserve(meshes.iter().map(|(_, mesh)| mesh.far_opaque.len()).sum());
        self.transparent
            .reserve(meshes.iter().map(|(_, mesh)| mesh.transparent.len()).sum());
        self.transparent_two_sided.reserve(
            meshes
                .iter()
                .map(|(_, mesh)| mesh.transparent_two_sided.len())
                .sum(),
        );
        self.translucent
            .reserve(meshes.iter().map(|(_, mesh)| mesh.translucent.len()).sum());
        self.model
            .reserve(meshes.iter().map(|(_, mesh)| mesh.model.len()).sum());
        self.model_idx
            .reserve(meshes.iter().map(|(_, mesh)| mesh.model_idx.len()).sum());
        self.model_blend_idx.reserve(
            meshes
                .iter()
                .map(|(_, mesh)| mesh.model_blend_idx.len())
                .sum(),
        );
        self.contact
            .reserve(meshes.iter().map(|(_, mesh)| mesh.contact.len()).sum());
    }
}

/// Fresh arena suballocations since process start, counted on every path
/// that claims one ([`fresh_layer_alloc`]). A sizing or reuse policy change
/// trades VRAM against this number, so it is measurable rather than argued.
pub(super) static TERRAIN_SUBALLOCS: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);

/// One column layer's live geometry: where it sits in the arena and how many
/// bytes of it are in use. The allocation's size-class capacity is usually
/// larger, and that rounding IS the growth headroom a remesh writes into
/// (see [`layer_fits`]).
pub struct Layer {
    pub alloc: LayerAlloc,
    pub len: u64,
}

/// Upload `data` into the arena, REUSING `prev`'s suballocation when the data
/// still fits its size class.
///
/// Reuse is the point: a section re-meshes constantly while streaming (a
/// freshly loaded section re-lights its neighbours, each of which remeshes),
/// and re-suballocating for every one of those re-uploads churns the free
/// lists on the render thread. Writing into the allocation it already has
/// avoids that. The class rounding IS the growth headroom; an allocation is
/// released only when the data no longer fits it or has shrunk past a 4×
/// hysteresis, so a dug-out column returns its VRAM but size jitter never
/// churns.
fn upload_layer(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    arena: &mut GeometryArena,
    prev: Option<Layer>,
    data: &[u8],
) -> Option<Layer> {
    let len = data.len() as u64;
    if data.is_empty() {
        return None;
    }
    let alloc = match prev {
        Some(p) if layer_fits(&p, len) => p.alloc,
        _ => fresh_layer_alloc(device, arena, len),
    };
    arena.write(queue, &alloc, 0, data);
    Some(Layer { alloc, len })
}

/// Can `len` bytes be written into `prev`'s allocation in place? Yes while
/// they fit, unless the allocation is now wildly oversized (the player mined
/// out most of the column / far LOD replaced dense foliage) — then it goes
/// back to the arena rather than pinning VRAM.
fn layer_fits(prev: &Layer, len: u64) -> bool {
    let cap = prev.alloc.capacity();
    let oversized = cap > 16 * 1024 && cap / 4 > len;
    cap >= len && !oversized
}

/// The ONE path that claims arena space for a column layer, so
/// [`TERRAIN_SUBALLOCS`] counts every allocation whichever upload took it.
fn fresh_layer_alloc(device: &wgpu::Device, arena: &mut GeometryArena, len: u64) -> LayerAlloc {
    TERRAIN_SUBALLOCS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    arena.alloc(device, len)
}

fn append_indexed_layer<V: Copy>(
    verts: &mut Vec<V>,
    indices: &mut Vec<u32>,
    src_verts: &[V],
    src_indices: &[u32],
) -> (u32, u32, u32, u32) {
    let index_start = indices.len() as u32;
    let vertex_start = verts.len() as u32;
    verts.extend_from_slice(src_verts);
    if vertex_start == 0 {
        indices.extend_from_slice(src_indices);
    } else {
        indices.extend(src_indices.iter().map(|&i| i + vertex_start));
    }
    (
        index_start,
        src_indices.len() as u32,
        vertex_start,
        src_verts.len() as u32,
    )
}

/// Append an IMPLIED-triangulation layer (see [`petramond_mesh::QuadIdx`]): only
/// the vertices travel, and the draw reads the shared quad index buffer with
/// the section's vertex start as `base_vertex`. Returns
/// `(vertex_start, vertex_count)`.
fn append_quad_layer(
    verts: &mut Vec<TerrainVertex>,
    src_verts: &[Vertex],
    col_ox: i32,
    col_oz: i32,
) -> (u32, u32) {
    let vertex_start = verts.len() as u32;
    verts.extend(
        src_verts
            .iter()
            .map(|v| TerrainVertex::from_world(v, col_ox, col_oz)),
    );
    (vertex_start, src_verts.len() as u32)
}

/// `(min_cy, max_cy)` over a column's installed sections, inverted when empty.
fn cy_span(sections: &[(SectionPos, GpuSectionMesh)]) -> (i32, i32) {
    sections
        .iter()
        .fold((i32::MAX, i32::MIN), |(lo, hi), (sp, _)| {
            (lo.min(sp.cy), hi.max(sp.cy))
        })
}

pub(super) fn upload_column_mesh(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    meshes: &[(SectionPos, &ChunkMesh)],
    prev: Option<GpuColumnMesh>,
    scratch: &mut ColumnUploadScratch,
    origins: &mut ColumnOrigins,
    arena: &mut GeometryArena,
    quad_index: &mut QuadIndexBuffer,
    batch: &mut TerrainUploadBatch,
) -> GpuColumnMesh {
    let col_ox = meshes.first().map(|(sp, _)| sp.cx * 16).unwrap_or(0);
    let col_oz = meshes.first().map(|(sp, _)| sp.cz * 16).unwrap_or(0);

    if let Some(g) = prev {
        if try_patch_column_verts(queue, arena, meshes, &g) {
            return g;
        }
        return incremental::repack(device, queue, arena, quad_index, meshes, g, batch);
    }

    scratch.clear();
    scratch.reserve_for(meshes);
    let mut sections = Vec::with_capacity(meshes.len());

    // The column index buffer lays out every section's OPAQUE model indices
    // first, then every section's BLEND indices, so both regions batch as one
    // contiguous range per column. The total is needed up front to place each
    // section's blend range.
    let model_opaque_total: u32 = meshes.iter().map(|(_, m)| m.model_idx.len() as u32).sum();

    for &(sp, mesh) in meshes {
        let (opaque_vertex_start, opaque_vertex_count) =
            append_quad_layer(&mut scratch.opaque, &mesh.opaque, col_ox, col_oz);
        let (far_opaque_vertex_start, far_opaque_vertex_count) =
            append_quad_layer(&mut scratch.far_opaque, &mesh.far_opaque, col_ox, col_oz);
        let (transparent_vertex_start, transparent_vertex_count) =
            append_quad_layer(&mut scratch.transparent, &mesh.transparent, col_ox, col_oz);
        let (transparent_ts_vertex_start, transparent_ts_vertex_count) = append_quad_layer(
            &mut scratch.transparent_two_sided,
            &mesh.transparent_two_sided,
            col_ox,
            col_oz,
        );
        let (translucent_vertex_start, translucent_vertex_count) =
            append_quad_layer(&mut scratch.translucent, &mesh.translucent, col_ox, col_oz);
        let (model_index_start, model_idx_count, model_vertex_start, model_vertex_count) =
            append_indexed_layer(
                &mut scratch.model,
                &mut scratch.model_idx,
                &mesh.model,
                &mesh.model_idx,
            );
        // Same vertex buffer, blend index region: rebase onto this section's
        // vertex start, positioned in the column-wide blend tail.
        let model_blend_index_start = model_opaque_total + scratch.model_blend_idx.len() as u32;
        let model_blend_idx_count = mesh.model_blend_idx.len() as u32;
        scratch
            .model_blend_idx
            .extend(mesh.model_blend_idx.iter().map(|&i| i + model_vertex_start));
        let contact_vertex_start = scratch.contact.len() as u32;
        let contact_vertex_count = mesh.contact.len() as u32;
        scratch.contact.extend_from_slice(&mesh.contact);
        sections.push((
            sp,
            GpuSectionMesh {
                model_local_indices: mesh.model_idx.clone().into(),
                model_local_blend_indices: mesh.model_blend_idx.clone().into(),
                origin: (sp.cx * 16, sp.cy * 16, sp.cz * 16),
                opaque_vertex_start,
                opaque_vertex_count,
                far_opaque_vertex_start,
                far_opaque_vertex_count,
                transparent_vertex_start,
                transparent_vertex_count,
                transparent_ts_vertex_start,
                transparent_ts_vertex_count,
                translucent_vertex_start,
                translucent_vertex_count,
                model_index_start,
                model_idx_count,
                model_blend_index_start,
                model_blend_idx_count,
                model_vertex_start,
                model_vertex_count,
                contact_vertex_start,
                contact_vertex_count,
                index_hash: section_index_hash(mesh),
            },
        ));
    }

    // The largest implied-triangulation draw this column can submit: the whole
    // column's opaque stream, or one section's far-LOD stream.
    quad_index.ensure(
        device,
        queue,
        (scratch
            .opaque
            .len()
            .max(scratch.far_opaque.len())
            .max(scratch.transparent.len())
            .max(scratch.transparent_two_sided.len())
            .max(scratch.translucent.len())
            / 4) as u32,
    );

    // Fold the blend region into the column index buffer's tail (per-section
    // blend ranges were already placed at `model_opaque_total + …`).
    let model_blend_idx_count = scratch.model_blend_idx.len() as u32;
    scratch.model_idx.append(&mut scratch.model_blend_idx);

    GpuColumnMesh {
        opaque_vbuf: upload_layer(
            device,
            queue,
            arena,
            None,
            bytemuck::cast_slice(&scratch.opaque),
        ),
        opaque_quads: (scratch.opaque.len() / 4) as u32,
        far_opaque_vbuf: upload_layer(
            device,
            queue,
            arena,
            None,
            bytemuck::cast_slice(&scratch.far_opaque),
        ),
        transparent_vbuf: upload_layer(
            device,
            queue,
            arena,
            None,
            bytemuck::cast_slice(&scratch.transparent),
        ),
        transparent_ts_vbuf: upload_layer(
            device,
            queue,
            arena,
            None,
            bytemuck::cast_slice(&scratch.transparent_two_sided),
        ),
        translucent_vbuf: upload_layer(
            device,
            queue,
            arena,
            None,
            bytemuck::cast_slice(&scratch.translucent),
        ),
        model_vbuf: upload_layer(
            device,
            queue,
            arena,
            None,
            bytemuck::cast_slice(&scratch.model),
        ),
        model_ibuf: upload_layer(
            device,
            queue,
            arena,
            None,
            bytemuck::cast_slice(&scratch.model_idx),
        ),
        model_idx_count: model_opaque_total,
        model_blend_idx_count,
        contact_vbuf: upload_layer(
            device,
            queue,
            arena,
            None,
            bytemuck::cast_slice(&scratch.contact),
        ),
        contact_vertex_count: scratch.contact.len() as u32,
        origin_slot: origins.slot(device, queue, None, col_ox, col_oz),
        col_ox,
        col_oz,
        cy_span: cy_span(&sections),
        sections,
    }
}

#[cfg(test)]
mod tests;
