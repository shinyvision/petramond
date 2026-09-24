//! The vertex-only patch: rewriting a column's vertex attributes in place when
//! every section kept its counts and index topology (light/AO remeshes).

use super::{GeometryArena, GpuColumnMesh, GpuSectionMesh, Layer};
use petramond_mesh::{ChunkMesh, TerrainVertex, Vertex};
use petramond_world::chunk::SectionPos;

fn patch_terrain_verts(
    queue: &wgpu::Queue,
    arena: &GeometryArena,
    buf: &Option<Layer>,
    vertex_start: u32,
    src: &[Vertex],
) -> bool {
    if src.is_empty() {
        return true;
    }
    let quantized: Vec<TerrainVertex> = src.iter().map(TerrainVertex::from_mesh).collect();
    patch_verts(queue, arena, buf, vertex_start, &quantized)
}

/// FNV-1a over a section's SECTION-LOCAL index streams, all indexed layers in a
/// fixed order. With per-layer counts already matched, equal hashes mean the
/// column-buffer indices retained on the GPU (section-local + a vertex-start
/// offset that count equality pins) are still valid for the new vertex data.
pub(super) fn section_index_hash(mesh: &ChunkMesh) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut eat = |stream: &[u32]| {
        for &i in stream {
            h ^= i as u64;
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
        // Layer separator: streams of different layers must not concatenate
        // into the same digest position.
        h ^= 0xff;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    };
    eat(&mesh.model_idx);
    eat(&mesh.model_blend_idx);
    h
}

fn layer_sizes_match(mesh: &ChunkMesh, gpu: &GpuSectionMesh) -> bool {
    // The opaque stream is packed as two regions (far, then leaf tail), so the
    // split must land in the same place as well as the total.
    super::far_len(mesh) == gpu.opaque_vertex_count
        && mesh.opaque.len() as u32 - super::far_len(mesh) == gpu.opaque_tail_count
        && mesh.transparent.len() as u32 == gpu.transparent_vertex_count
        && mesh.transparent_two_sided.len() as u32 == gpu.transparent_ts_vertex_count
        && mesh.translucent.len() as u32 == gpu.translucent_vertex_count
        && mesh.model.len() as u32 == gpu.model_vertex_count
        && mesh.model_idx.len() as u32 == gpu.model_idx_count
        && mesh.model_blend_idx.len() as u32 == gpu.model_blend_idx_count
        && mesh.contact.len() as u32 == gpu.contact_vertex_count
}

fn patch_verts<V: bytemuck::Pod>(
    queue: &wgpu::Queue,
    arena: &GeometryArena,
    buf: &Option<Layer>,
    vertex_start: u32,
    src: &[V],
) -> bool {
    if src.is_empty() {
        return true;
    }
    let Some(buf) = buf else {
        return false;
    };
    let offset = vertex_start as u64 * std::mem::size_of::<V>() as u64;
    let bytes = bytemuck::cast_slice(src);
    arena.write(queue, &buf.alloc, offset, bytes)
}

/// When every section keeps the same vertex/index counts as the installed GPU
/// column, rewrite only vertex attributes in place (light/AO remeshes). Indices
/// and sibling CPU packing are skipped entirely.
pub(super) fn try_patch_column_verts(
    queue: &wgpu::Queue,
    arena: &GeometryArena,
    meshes: &[(SectionPos, &ChunkMesh)],
    prev: &GpuColumnMesh,
) -> bool {
    if meshes.len() != prev.sections.len() {
        return false;
    }
    for (&(sp, mesh), &(psp, ref gpu)) in meshes.iter().zip(&prev.sections) {
        if sp != psp
            || (mesh.mesh_dirty
                && (!layer_sizes_match(mesh, gpu) || section_index_hash(mesh) != gpu.index_hash))
        {
            return false;
        }
    }
    for (&(_, mesh), (_, gpu)) in meshes.iter().zip(&prev.sections) {
        if !mesh.mesh_dirty {
            continue;
        }
        let far = super::far_len(mesh) as usize;
        if !patch_terrain_verts(
            queue,
            arena,
            &prev.opaque_vbuf,
            gpu.opaque_vertex_start,
            &mesh.opaque[..far],
        ) || !patch_terrain_verts(
            queue,
            arena,
            &prev.opaque_vbuf,
            gpu.opaque_tail_start,
            &mesh.opaque[far..],
        ) || !patch_terrain_verts(
            queue,
            arena,
            &prev.transparent_vbuf,
            gpu.transparent_vertex_start,
            &mesh.transparent,
        ) || !patch_terrain_verts(
            queue,
            arena,
            &prev.transparent_ts_vbuf,
            gpu.transparent_ts_vertex_start,
            &mesh.transparent_two_sided,
        ) || !patch_terrain_verts(
            queue,
            arena,
            &prev.translucent_vbuf,
            gpu.translucent_vertex_start,
            &mesh.translucent,
        ) || !patch_verts(
            queue,
            arena,
            &prev.model_vbuf,
            gpu.model_vertex_start,
            &mesh.model,
        ) || !patch_verts(
            queue,
            arena,
            &prev.contact_vbuf,
            gpu.contact_vertex_start,
            &mesh.contact,
        ) {
            return false;
        }
    }
    true
}
