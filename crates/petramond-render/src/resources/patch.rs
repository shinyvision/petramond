use super::layers::{mesh_bytes, mesh_count};
use super::{GpuColumnMesh, GpuSectionMesh, SectionStream, TerrainArenas};
use petramond_mesh::ChunkMesh;
use petramond_world::chunk::SectionPos;

pub(super) fn section_index_hash(mesh: &ChunkMesh) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut eat = |stream: &[u32]| {
        for &i in stream {
            h ^= i as u64;
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
        h ^= 0xff;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    };
    eat(&mesh.model_idx);
    eat(&mesh.model_blend_idx);
    h
}

fn counts_match(mesh: &ChunkMesh, gpu: &GpuSectionMesh) -> bool {
    SectionStream::ALL
        .iter()
        .all(|&stream| mesh_count(mesh, stream) == gpu.span(stream).count)
}

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
