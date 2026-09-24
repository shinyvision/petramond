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
        // Source allocations must stay live until copies have been submitted:
        // earlier recycling could let a queued CPU write overwrite their data.
        drop(self.retired);
    }
}

struct Span {
    destination: u32,
    count: u32,
    source: Source,
}

enum Source {
    Cpu(usize),
    Gpu(u32),
}

struct LayerPlan<V> {
    data: Vec<V>,
    spans: Vec<Span>,
    count: u32,
}

impl<V: bytemuck::Pod> LayerPlan<V> {
    fn new() -> Self {
        Self {
            data: Vec::new(),
            spans: Vec::new(),
            count: 0,
        }
    }

    fn push(&mut self, src: &[V], previous: Option<(u32, u32)>) -> (u32, u32) {
        let destination = self.count;
        let (count, source) = match previous {
            Some((start, count)) => (count, Source::Gpu(start)),
            None => {
                let offset = self.data.len();
                self.data.extend_from_slice(src);
                (src.len() as u32, Source::Cpu(offset))
            }
        };
        self.count += count;
        if count > 0 {
            self.spans.push(Span {
                destination,
                count,
                source,
            });
        }
        (destination, count)
    }

    /// Every retained span keeps its offset: nothing on the GPU has to move.
    fn unmoved(&self) -> bool {
        self.spans.iter().all(|s| match s.source {
            Source::Cpu(_) => true,
            Source::Gpu(start) => start == s.destination,
        })
    }

    /// Write the plan into GPU memory. The previous layer is written IN PLACE
    /// when it still fits and no retained span moved — the common repack (one
    /// section remeshed at the same size, or a trailing section changed) then
    /// costs only the changed sections' CPU writes. Otherwise a fresh
    /// allocation receives the CPU spans by write and the retained spans by
    /// GPU copy, and the previous layer joins the batch's retired list so its
    /// bytes stay live until those copies are submitted.
    fn upload(
        self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        arena: &mut GeometryArena,
        encoder: &mut wgpu::CommandEncoder,
        previous: Option<Layer>,
        retired: &mut Vec<Layer>,
    ) -> Option<Layer> {
        if self.count == 0 {
            retired.extend(previous);
            return None;
        }
        let stride = std::mem::size_of::<V>() as u64;
        let len = u64::from(self.count) * stride;
        let (alloc, source) = match previous {
            Some(p) if self.unmoved() && layer_fits(&p, len) => (p.alloc, None),
            other => (fresh_layer_alloc(device, arena, len), other),
        };
        for span in self.spans {
            let offset = u64::from(span.destination) * stride;
            match span.source {
                Source::Cpu(start) => {
                    arena.write(
                        queue,
                        &alloc,
                        offset,
                        bytemuck::cast_slice(&self.data[start..start + span.count as usize]),
                    );
                }
                // In place, an unmoved span already holds its bytes.
                Source::Gpu(_) if source.is_none() => {}
                Source::Gpu(start) => {
                    let source = source.as_ref().expect("retained GPU geometry has a layer");
                    arena.copy(
                        device,
                        encoder,
                        &source.alloc,
                        u64::from(start) * stride,
                        &alloc,
                        offset,
                        u64::from(span.count) * stride,
                    );
                }
            }
        }
        retired.extend(source);
        Some(Layer { alloc, len })
    }
}

impl LayerPlan<TerrainVertex> {
    fn terrain(&mut self, src: &[Vertex], previous: Option<(u32, u32)>) -> (u32, u32) {
        if previous.is_some() {
            return self.push(&[], previous);
        }
        let vertices: Vec<_> = src.iter().map(TerrainVertex::from_mesh).collect();
        self.push(&vertices, None)
    }
}

/// Retain column draw batching while copying unchanged siblings on the GPU.
/// Only model index streams remain on the CPU, so they can be rebased when a
/// preceding section changes size; released vertex streams never need remeshing.
/// `prev` is consumed: each layer is written in place, or replaced and retired
/// through `batch` (see [`LayerPlan::upload`]).
pub(super) fn repack(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    arena: &mut GeometryArena,
    quad_index: &mut QuadIndexBuffer,
    meshes: &[(SectionPos, &ChunkMesh)],
    prev: GpuColumnMesh,
    batch: &mut TerrainUploadBatch,
) -> GpuColumnMesh {
    let (ox, oz) = (prev.col_ox, prev.col_oz);
    let mut opaque = LayerPlan::new();
    let mut transparent = LayerPlan::new();
    let mut transparent_ts = LayerPlan::new();
    let mut translucent = LayerPlan::new();
    let mut model = LayerPlan::new();
    let mut contact = LayerPlan::new();
    let mut sections = Vec::with_capacity(meshes.len());
    // The opaque plan is filled in TWO passes — every section's far region,
    // then every section's leaf tail — so the packed column keeps the
    // far-region-first layout a whole-column far draw needs. The retained
    // tail ranges are collected here because the second pass no longer has
    // the first's `retained`.
    let mut retained_tails: Vec<Option<(u32, u32)>> = Vec::with_capacity(meshes.len());
    let mut indices = Vec::new();
    let mut blend = Vec::new();
    for &(pos, mesh) in meshes {
        let old = prev
            .sections
            .iter()
            .find(|(p, _)| *p == pos)
            .map(|(_, s)| s);
        let retained = old.filter(|_| !mesh.mesh_dirty);
        assert!(
            !mesh.is_released() || retained.is_some(),
            "released mesh requires its GPU copy"
        );
        let mut section = old.cloned().unwrap_or_default();
        section.origin = pos.origin_world();
        macro_rules! terrain {
            ($plan:ident, $src:ident, $start:ident, $count:ident) => {
                (section.$start, section.$count) =
                    $plan.terrain(&mesh.$src, retained.map(|s| (s.$start, s.$count)));
            };
        }
        let far = far_len(mesh) as usize;
        (section.opaque_vertex_start, section.opaque_vertex_count) = opaque.terrain(
            &mesh.opaque[..far],
            retained.map(|s| (s.opaque_vertex_start, s.opaque_vertex_count)),
        );
        // A retained section keeps the flag its record already carries: its
        // mesh may have been released, and a released mesh reports no far LOD.
        if retained.is_none() {
            section.has_far_lod = mesh.far_opaque_len > 0;
        }
        retained_tails.push(retained.map(|s| (s.opaque_tail_start, s.opaque_tail_count)));
        terrain!(
            transparent,
            transparent,
            transparent_vertex_start,
            transparent_vertex_count
        );
        terrain!(
            transparent_ts,
            transparent_two_sided,
            transparent_ts_vertex_start,
            transparent_ts_vertex_count
        );
        terrain!(
            translucent,
            translucent,
            translucent_vertex_start,
            translucent_vertex_count
        );
        (section.model_vertex_start, section.model_vertex_count) = model.push(
            &mesh.model,
            retained.map(|s| (s.model_vertex_start, s.model_vertex_count)),
        );
        (section.contact_vertex_start, section.contact_vertex_count) = contact.push(
            &mesh.contact,
            retained.map(|s| (s.contact_vertex_start, s.contact_vertex_count)),
        );
        if retained.is_none() {
            section.model_local_indices = mesh.model_idx.clone().into();
            section.model_local_blend_indices = mesh.model_blend_idx.clone().into();
            section.index_hash = section_index_hash(mesh);
        }
        section.model_index_start = indices.len() as u32;
        section.model_idx_count = section.model_local_indices.len() as u32;
        section.model_blend_index_start = blend.len() as u32;
        section.model_blend_idx_count = section.model_local_blend_indices.len() as u32;
        indices.extend(
            section
                .model_local_indices
                .iter()
                .map(|i| i + section.model_vertex_start),
        );
        blend.extend(
            section
                .model_local_blend_indices
                .iter()
                .map(|i| i + section.model_vertex_start),
        );
        sections.push((pos, section));
    }
    // Second opaque pass: the leaf tails, after every far region.
    let opaque_far_quads = opaque.count / 4;
    for (i, &(_, mesh)) in meshes.iter().enumerate() {
        let far = far_len(mesh) as usize;
        let (start, count) = opaque.terrain(&mesh.opaque[far..], retained_tails[i]);
        let section = &mut sections[i].1;
        section.opaque_tail_start = start;
        section.opaque_tail_count = count;
    }
    let model_idx_count = indices.len() as u32;
    let model_blend_idx_count = blend.len() as u32;
    for (_, s) in &mut sections {
        s.model_blend_index_start += model_idx_count;
    }
    indices.extend(blend);
    quad_index.ensure(
        device,
        queue,
        opaque
            .count
            .max(transparent.count)
            .max(transparent_ts.count)
            .max(translucent.count)
            / 4,
    );
    let opaque_quads = opaque.count / 4;
    let contact_vertex_count = contact.count;
    let TerrainUploadBatch { encoder, retired } = batch;
    let encoder = encoder.get_or_insert_with(|| {
        device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("retain unchanged terrain sections"),
        })
    });
    let opaque_vbuf = opaque.upload(device, queue, arena, encoder, prev.opaque_vbuf, retired);
    let transparent_vbuf = transparent.upload(
        device,
        queue,
        arena,
        encoder,
        prev.transparent_vbuf,
        retired,
    );
    let transparent_ts_vbuf = transparent_ts.upload(
        device,
        queue,
        arena,
        encoder,
        prev.transparent_ts_vbuf,
        retired,
    );
    let translucent_vbuf = translucent.upload(
        device,
        queue,
        arena,
        encoder,
        prev.translucent_vbuf,
        retired,
    );
    let model_vbuf = model.upload(device, queue, arena, encoder, prev.model_vbuf, retired);
    let contact_vbuf = contact.upload(device, queue, arena, encoder, prev.contact_vbuf, retired);
    // The index stream is rebuilt on the CPU whole, so its previous layer is
    // reused by the ordinary write-in-place rule (no GPU copy reads it).
    let model_ibuf = upload_layer(
        device,
        queue,
        arena,
        prev.model_ibuf,
        bytemuck::cast_slice(&indices),
    );
    GpuColumnMesh {
        opaque_vbuf,
        opaque_quads,
        opaque_far_quads,
        transparent_vbuf,
        transparent_ts_vbuf,
        translucent_vbuf,
        model_vbuf,
        model_ibuf,
        model_idx_count,
        model_blend_idx_count,
        contact_vbuf,
        contact_vertex_count,
        origin_slot: prev.origin_slot,
        col_ox: ox,
        col_oz: oz,
        cy_span: cy_span(&sections),
        sections,
    }
}

#[cfg(test)]
mod tests;
