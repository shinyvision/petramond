//! The vertex-only patch: rewriting a column's vertex streams in place when
//! every section kept its stream counts and index topology (light/AO remeshes).

use super::layers::{mesh_bytes, mesh_count};
use super::{GpuColumnMesh, GpuSectionMesh, SectionStream, TerrainArenas};
use petramond_mesh::ChunkMesh;
use petramond_world::chunk::SectionPos;

/// FNV-1a over a section's SECTION-LOCAL index streams, all indexed layers in a
/// fixed order. With per-stream counts already matched, equal hashes mean the
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

/// Every stream keeps its element count — which pins every start offset in
/// the column, the opaque far/tail split included.
fn counts_match(mesh: &ChunkMesh, gpu: &GpuSectionMesh) -> bool {
    SectionStream::ALL
        .iter()
        .all(|&stream| mesh_count(mesh, stream) == gpu.span(stream).count)
}

/// When every section keeps the same stream counts as the installed GPU
/// column, rewrite only the dirty sections' vertex streams in place, straight
/// from their sealed meshes. Indices and sibling packing are skipped entirely.
pub(super) fn try_patch_column(
    queue: &wgpu::Queue,
    arenas: &TerrainArenas,
    meshes: &[(SectionPos, &ChunkMesh)],
    prev: &GpuColumnMesh,
) -> bool {
    if meshes.len() != prev.sections.len() {
        return false;
    }
    for (&(sp, mesh), (psp, gpu)) in meshes.iter().zip(&prev.sections) {
        // The record is reused as-is, so everything it caches beyond the
        // vertex bytes must still hold: counts, index topology, connectivity.
        if sp != *psp
            || (mesh.mesh_dirty
                && (!counts_match(mesh, gpu)
                    || section_index_hash(mesh) != gpu.index_hash
                    || mesh.visibility != gpu.visibility))
        {
            return false;
        }
    }
    for (&(_, mesh), (_, gpu)) in meshes.iter().zip(&prev.sections) {
        if !mesh.mesh_dirty {
            continue;
        }
        for stream in SectionStream::ALL {
            let span = gpu.span(stream);
            // Equal index topology was checked above; only vertices changed.
            if stream.is_index() || span.is_empty() {
                continue;
            }
            let buffer = stream.buffer();
            let Some(layer) = prev.buffer(buffer) else {
                return false;
            };
            if !arenas.get(buffer).write(
                queue,
                &layer.alloc,
                u64::from(span.start) * buffer.stride(),
                mesh_bytes(mesh, stream),
            ) {
                return false;
            }
        }
    }
    true
}
