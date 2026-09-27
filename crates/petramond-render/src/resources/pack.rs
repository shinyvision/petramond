use super::layers::{mesh_bytes, mesh_count, mesh_indices};
use super::patch::section_index_hash;
use super::*;

#[derive(Default)]
pub(crate) struct TerrainUploadBatch {
    encoder: Option<wgpu::CommandEncoder>,
    retired: Vec<Layer>,
}

impl TerrainUploadBatch {
    pub(crate) fn submit(self, queue: &wgpu::Queue) {
        if let Some(encoder) = self.encoder {
            queue.submit([encoder.finish()]);
        }
        drop(self.retired);
    }
}

enum Source<'a> {
    Bytes(&'a [u8]),
    Indices(&'a [u32], u32),
    Retained(u32),
}

impl Source<'_> {
    fn is_cpu(&self) -> bool {
        !matches!(self, Source::Retained(_))
    }
}

struct Planned<'a> {
    destination: u32,
    count: u32,
    source: Source<'a>,
}

struct Entry<'a> {
    pos: SectionPos,
    mesh: &'a ChunkMesh,
    retained: Option<&'a GpuSectionMesh>,
    record: GpuSectionMesh,
}

/// Pack `meshes` into a column, reusing `prev`'s allocations and origin slot
/// when there is one. Each buffer is written in place when it still fits and
/// no retained span moved — the common repack (one section remeshed at the
/// same size, or a trailing section changed) then costs only the changed
/// sections' writes. Otherwise a fresh allocation receives the new spans by
/// write and the retained spans by GPU copy, and the previous layer joins the
/// batch's retired list so its bytes stay live until those copies submit.
#[allow(clippy::too_many_arguments)]
pub(super) fn pack_column(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    arenas: &mut TerrainArenas,
    quad_index: &mut QuadIndexBuffer,
    origins: &mut ColumnOrigins,
    meshes: &[(SectionPos, &ChunkMesh)],
    prev: Option<GpuColumnMesh>,
    batch: &mut TerrainUploadBatch,
) -> GpuColumnMesh {
    let (col_ox, col_oz) = meshes
        .first()
        .map_or((0, 0), |(sp, _)| (sp.cx * 16, sp.cz * 16));
    let (mut prev_buffers, prev_sections, prev_slot) = match prev {
        Some(p) => (p.buffers, p.sections, Some(p.origin_slot)),
        None => (Default::default(), Vec::new(), None),
    };

    let mut entries = section_entries(meshes, &prev_sections);
    let (regions, buffer_len) = place_spans(&mut entries);
    let plans = plan_buffers(&entries);

    let largest_quad_draw = ColumnBuffer::ALL
        .iter()
        .filter(|buffer| matches!(buffer, ColumnBuffer::Quads(_)))
        .map(|buffer| buffer_len[buffer.index()] / 4)
        .max()
        .unwrap_or(0);
    quad_index.ensure(device, queue, largest_quad_draw);

    let TerrainUploadBatch { encoder, retired } = batch;
    let mut buffers: [Option<Layer>; ColumnBuffer::COUNT] = Default::default();
    for buffer in ColumnBuffer::ALL {
        let i = buffer.index();
        buffers[i] = upload_buffer(
            device,
            queue,
            arenas.get_mut(buffer),
            encoder,
            buffer.stride(),
            buffer_len[i],
            &plans[i],
            prev_buffers[i].take(),
            retired,
        );
    }

    let sections: Vec<_> = entries
        .into_iter()
        .map(|entry| (entry.pos, entry.record))
        .collect();
    GpuColumnMesh {
        buffers,
        regions,
        origin_slot: prev_slot.unwrap_or_else(|| origins.slot(device, queue, None, col_ox, col_oz)),
        col_ox,
        col_oz,
        cy_span: cy_span(&sections),
        sections,
    }
}

fn section_entries<'a>(
    meshes: &[(SectionPos, &'a ChunkMesh)],
    prev_sections: &'a [(SectionPos, GpuSectionMesh)],
) -> Vec<Entry<'a>> {
    meshes
        .iter()
        .map(|&(pos, mesh)| {
            let old = prev_sections
                .iter()
                .find(|(p, _)| *p == pos)
                .map(|(_, s)| s);
            let retained = old.filter(|_| !mesh.mesh_dirty);
            assert!(
                !mesh.is_released() || retained.is_some(),
                "released mesh requires its GPU copy"
            );
            let mut record = old.cloned().unwrap_or_default();
            record.origin = pos.origin_world();
            match retained {
                Some(old) => {
                    for stream in SectionStream::ALL {
                        record.spans[stream.index()].count = old.span(stream).count;
                    }
                }
                None => {
                    record.has_far_lod = mesh.far_opaque_len > 0;
                    record.model_local_indices = mesh.model_idx.clone().into();
                    record.model_local_blend_indices = mesh.model_blend_idx.clone().into();
                    record.index_hash = section_index_hash(mesh);
                    record.visibility = mesh.visibility;
                    for stream in SectionStream::ALL {
                        record.spans[stream.index()].count = mesh_count(mesh, stream);
                    }
                }
            }
            Entry {
                pos,
                mesh,
                retained,
                record,
            }
        })
        .collect()
}

fn place_spans(
    entries: &mut [Entry<'_>],
) -> ([u32; SectionStream::COUNT], [u32; ColumnBuffer::COUNT]) {
    let mut regions = [0u32; SectionStream::COUNT];
    for entry in entries.iter() {
        for stream in SectionStream::ALL {
            regions[stream.index()] += entry.record.span(stream).count;
        }
    }
    let mut cursor = [0u32; SectionStream::COUNT];
    let mut buffer_len = [0u32; ColumnBuffer::COUNT];
    for stream in SectionStream::ALL {
        let len = &mut buffer_len[stream.buffer().index()];
        cursor[stream.index()] = *len;
        *len += regions[stream.index()];
    }
    for entry in entries.iter_mut() {
        for stream in SectionStream::ALL {
            let span = &mut entry.record.spans[stream.index()];
            span.start = cursor[stream.index()];
            cursor[stream.index()] += span.count;
        }
    }
    (regions, buffer_len)
}

fn plan_buffers<'a>(entries: &[Entry<'a>]) -> [Vec<Planned<'a>>; ColumnBuffer::COUNT] {
    let mut plans: [Vec<Planned<'a>>; ColumnBuffer::COUNT] = Default::default();
    for entry in entries {
        let (mesh, retained) = (entry.mesh, entry.retained);
        let vertex_base = entry.record.span(SectionStream::ModelVertices).start;
        for stream in SectionStream::ALL {
            let span = entry.record.span(stream);
            if span.is_empty() {
                continue;
            }
            let source = match (stream.is_index(), retained) {
                (true, Some(old)) => Source::Indices(old.local_indices(stream), vertex_base),
                (true, None) => Source::Indices(mesh_indices(mesh, stream), vertex_base),
                (false, Some(old)) => Source::Retained(old.span(stream).start),
                (false, None) => Source::Bytes(mesh_bytes(mesh, stream)),
            };
            plans[stream.buffer().index()].push(Planned {
                destination: span.start,
                count: span.count,
                source,
            });
        }
    }
    for plan in &mut plans {
        plan.sort_unstable_by_key(|p| p.destination);
    }
    plans
}

#[allow(clippy::too_many_arguments)]
fn upload_buffer(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    arena: &mut GeometryArena,
    encoder: &mut Option<wgpu::CommandEncoder>,
    stride: u64,
    count: u32,
    plan: &[Planned<'_>],
    previous: Option<Layer>,
    retired: &mut Vec<Layer>,
) -> Option<Layer> {
    if count == 0 {
        retired.extend(previous);
        return None;
    }
    let len = u64::from(count) * stride;
    let unmoved = plan.iter().all(|p| match p.source {
        Source::Retained(start) => start == p.destination,
        _ => true,
    });
    let (alloc, source) = match previous {
        Some(p) if unmoved && layer_fits(&p, len) => (p.alloc, None),
        other => (fresh_layer_alloc(device, arena, len), other),
    };
    let mut at = 0;
    while at < plan.len() {
        if !plan[at].source.is_cpu() {
            at += 1;
            continue;
        }
        let first = plan[at].destination;
        let mut end = at;
        let mut next = first;
        while end < plan.len() && plan[end].source.is_cpu() && plan[end].destination == next {
            next += plan[end].count;
            end += 1;
        }
        let run = &plan[at..end];
        let written = arena.write_with(
            queue,
            &alloc,
            u64::from(first) * stride,
            u64::from(next - first) * stride,
            |dst| fill_run(dst, run, stride),
        );
        debug_assert!(written, "a planned span left its allocation");
        at = end;
    }
    if let Some(previous) = &source {
        let encoder = encoder.get_or_insert_with(|| {
            device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("retain unchanged terrain sections"),
            })
        });
        for p in plan {
            if let Source::Retained(start) = p.source {
                arena.copy(
                    device,
                    encoder,
                    &previous.alloc,
                    u64::from(start) * stride,
                    &alloc,
                    u64::from(p.destination) * stride,
                    u64::from(p.count) * stride,
                );
            }
        }
    }
    retired.extend(source);
    Some(Layer { alloc, len })
}

fn fill_run(dst: &mut [u8], run: &[Planned<'_>], stride: u64) {
    let mut at = 0;
    for p in run {
        let bytes = p.count as usize * stride as usize;
        let out = &mut dst[at..at + bytes];
        match p.source {
            Source::Bytes(src) => out.copy_from_slice(src),
            Source::Indices(local, base) => {
                for (word, &index) in out.chunks_exact_mut(4).zip(local) {
                    word.copy_from_slice(&(index + base).to_ne_bytes());
                }
            }
            Source::Retained(_) => unreachable!("a staging run holds only new spans"),
        }
        at += bytes;
    }
}

#[cfg(test)]
mod tests;
