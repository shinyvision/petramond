//! `DynamicDraw`: a per-frame-rewritten vertex(+index) buffer pair for one draw
//! subsystem, collapsing the field group every dynamic subsystem used to spell
//! out by hand (pipeline + vbuf + ibuf + a CPU staging `Vec` + an uploaded
//! count). One `bake` does the shape that was repeated ~7× inline: clear the
//! count, build the geometry, GROW the buffers to fit, upload, store the
//! count; `draw` binds + issues the indexed draw.
//!
//! The buffers GROW. There is no cap: a frame that bakes more than the buffer
//! holds gets a bigger buffer, never a blank subsystem. Growth carries 25%
//! headroom at one-page granularity so a slowly rising count reallocates a
//! handful of times, and a buffer never shrinks — the next crowd is coming.
//! Every subsystem starts at one small page, so a quiet scene holds almost no
//! VRAM for the dynamic streams. Every growable buffer in the renderer — the
//! subsystems here and the hand-managed hand/outline/icon streams alike —
//! starts through [`new_buffer`] and grows through [`upload`].

/// Bytes every growable buffer starts at, and the granule growth rounds to.
const INITIAL_BYTES: u64 = 4096;

/// A fresh growable buffer at its initial size.
pub(crate) fn new_buffer(
    device: &wgpu::Device,
    usage: wgpu::BufferUsages,
    label: &str,
) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size: INITIAL_BYTES,
        usage: usage | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

/// The size a buffer grows to when it must hold `needed` bytes: 25% headroom,
/// rounded up to whole pages.
fn grown_size(needed: u64) -> u64 {
    (needed + needed / 4).div_ceil(INITIAL_BYTES) * INITIAL_BYTES
}

/// Make `buffer` hold at least `needed` bytes: recreated with headroom when it
/// does not (its contents are gone — callers upload after this), untouched
/// when it does. Never shrinks.
pub(super) fn ensure_capacity(
    device: &wgpu::Device,
    buffer: &mut wgpu::Buffer,
    needed: u64,
    usage: wgpu::BufferUsages,
    label: &str,
) {
    if buffer.size() >= needed {
        return;
    }
    *buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size: grown_size(needed),
        usage: usage | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
}

/// Upload `data` to `buffer`, growing it first when it is too small.
pub(super) fn upload<T: bytemuck::Pod>(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    buffer: &mut wgpu::Buffer,
    data: &[T],
    usage: wgpu::BufferUsages,
    label: &str,
) {
    let bytes: &[u8] = bytemuck::cast_slice(data);
    ensure_capacity(device, buffer, bytes.len() as u64, usage, label);
    queue.write_buffer(buffer, 0, bytes);
}

/// The vertex-buffer and index-buffer labels a subsystem's `label` expands to.
fn buffer_labels(label: &str) -> (String, String) {
    (format!("{label} vbuf"), format!("{label} ibuf"))
}

/// An indexed dynamic draw: an owned `{ pipeline, vbuf, ibuf }` plus the index
/// count uploaded this frame (`0` = nothing to draw). The CPU staging vectors
/// are supplied to [`DynamicDraw::bake`] by the caller — several subsystems
/// (item-entity, chest, break) deliberately SHARE one scratch pair because they
/// bake sequentially, so the scratch lives on the renderer, not here, to
/// preserve that exact reuse.
pub(super) struct DynamicDraw {
    pub pipeline: crate::pipeline::SampledPipeline,
    pub vbuf: wgpu::Buffer,
    pub ibuf: wgpu::Buffer,
    vbuf_label: String,
    ibuf_label: String,
    /// Index count uploaded this frame (`0` = nothing baked).
    pub index_count: u32,
}

impl DynamicDraw {
    pub(super) fn new(
        device: &wgpu::Device,
        pipeline: crate::pipeline::SampledPipeline,
        label: &'static str,
    ) -> Self {
        let (vbuf_label, ibuf_label) = buffer_labels(label);
        Self {
            pipeline,
            vbuf: new_buffer(device, wgpu::BufferUsages::VERTEX, &vbuf_label),
            ibuf: new_buffer(device, wgpu::BufferUsages::INDEX, &ibuf_label),
            vbuf_label,
            ibuf_label,
            index_count: 0,
        }
    }

    /// Bake one frame's indexed geometry. Clears the count, runs `build` to fill
    /// the supplied CPU scratch (the build returns the index count it emitted),
    /// and — if it produced geometry — grows the buffers to fit, uploads the
    /// vertex + index slices and records the count.
    ///
    /// The scratch is passed in (not owned) so subsystems that intentionally
    /// reuse the same `verts`/`indices` across sequential bakes keep doing so.
    pub(super) fn bake<V: bytemuck::Pod>(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        verts: &mut Vec<V>,
        indices: &mut Vec<u32>,
        build: impl FnOnce(&mut Vec<V>, &mut Vec<u32>) -> u32,
    ) {
        self.index_count = 0;
        let count = build(verts, indices);
        if count == 0 {
            return;
        }
        upload(
            device,
            queue,
            &mut self.vbuf,
            verts,
            wgpu::BufferUsages::VERTEX,
            &self.vbuf_label,
        );
        upload(
            device,
            queue,
            &mut self.ibuf,
            indices,
            wgpu::BufferUsages::INDEX,
            &self.ibuf_label,
        );
        self.index_count = count;
    }

    /// Bind this subsystem's pipeline + vbuf/ibuf and draw its baked index range.
    /// The caller sets any shared bind groups (uniform/atlas) first; this issues
    /// `set_pipeline` + buffers + one `draw_indexed`. No-op when nothing is baked.
    pub(super) fn draw(&self, pass: &mut wgpu::RenderPass<'_>, samples: u32) {
        if self.index_count == 0 {
            return;
        }
        pass.set_pipeline(self.pipeline.get(samples));
        pass.set_vertex_buffer(0, self.vbuf.slice(..));
        pass.set_index_buffer(self.ibuf.slice(..), wgpu::IndexFormat::Uint32);
        pass.draw_indexed(0..self.index_count, 0, 0..1);
    }
}

/// The index list for `prims` primitives that each own `verts_per_prim`
/// consecutive vertices and share one relative index `pattern` — a quad's
/// `[0, 1, 2, 0, 2, 3]`, a cube's six of those.
pub(crate) fn prim_index_list(pattern: &[u32], verts_per_prim: u32, prims: u32) -> Vec<u32> {
    let mut out = Vec::with_capacity(pattern.len() * prims as usize);
    for prim in 0..prims {
        let base = prim * verts_per_prim;
        out.extend(pattern.iter().map(|i| base + i));
    }
    out
}

/// How many primitives a patterned draw's index buffer must cover once its
/// vertex buffer holds `vbuf_bytes` for primitives of `prim_bytes` each and
/// this frame baked `prims`: every primitive the vertex buffer can hold, and
/// never fewer than were baked. Indexing the whole buffer is what lets the
/// index list stand until the NEXT growth instead of being rebuilt per frame.
fn prims_to_index(vbuf_bytes: u64, prim_bytes: u64, prims: u32) -> u32 {
    let capacity = (vbuf_bytes / prim_bytes.max(1)) as u32;
    capacity.max(prims)
}

/// A dynamic draw over PATTERNED primitives — the particle cubes, the shadow
/// quads — whose index list is the same relative pattern per primitive. Only
/// the vertex stream is rebuilt each frame; the index buffer is regenerated
/// only when the vertex buffer grows past what it covers. Stores the vertex
/// count baked this frame; the index count is derived per draw.
pub(super) struct DynamicVertexDraw {
    pub pipeline: crate::pipeline::SampledPipeline,
    pub vbuf: wgpu::Buffer,
    pub ibuf: wgpu::Buffer,
    vbuf_label: String,
    ibuf_label: String,
    verts_per_prim: u32,
    pattern: &'static [u32],
    /// How many primitives `ibuf` currently indexes.
    prims_indexed: u32,
    /// Vertex count uploaded this frame (`0` = nothing baked).
    pub vertex_count: u32,
}

impl DynamicVertexDraw {
    pub(super) fn new(
        device: &wgpu::Device,
        pipeline: crate::pipeline::SampledPipeline,
        label: &'static str,
        verts_per_prim: u32,
        pattern: &'static [u32],
    ) -> Self {
        let (vbuf_label, ibuf_label) = buffer_labels(label);
        Self {
            pipeline,
            vbuf: new_buffer(device, wgpu::BufferUsages::VERTEX, &vbuf_label),
            ibuf: new_buffer(device, wgpu::BufferUsages::INDEX, &ibuf_label),
            vbuf_label,
            ibuf_label,
            verts_per_prim,
            pattern,
            prims_indexed: 0,
            vertex_count: 0,
        }
    }

    /// Bake one frame's vertex stream. Clears the count, runs `build` to fill the
    /// supplied scratch (returns the vertex count emitted), grows the vertex
    /// buffer to fit, and extends the index pattern over every primitive the
    /// grown buffer can hold.
    pub(super) fn bake<V: bytemuck::Pod>(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        verts: &mut Vec<V>,
        build: impl FnOnce(&mut Vec<V>) -> u32,
    ) {
        self.vertex_count = 0;
        let count = build(verts);
        if count == 0 {
            return;
        }
        upload(
            device,
            queue,
            &mut self.vbuf,
            verts,
            wgpu::BufferUsages::VERTEX,
            &self.vbuf_label,
        );
        let prims = count / self.verts_per_prim;
        if prims > self.prims_indexed {
            let prim_bytes = self.verts_per_prim as u64 * std::mem::size_of::<V>() as u64;
            let to_index = prims_to_index(self.vbuf.size(), prim_bytes, prims);
            let indices = prim_index_list(self.pattern, self.verts_per_prim, to_index);
            upload(
                device,
                queue,
                &mut self.ibuf,
                &indices,
                wgpu::BufferUsages::INDEX,
                &self.ibuf_label,
            );
            self.prims_indexed = to_index;
        }
        self.vertex_count = count;
    }

    /// Bind this subsystem's pipeline + vbuf + ibuf and draw `index_count`
    /// indices (derived by the caller from `vertex_count`). The caller sets shared
    /// bind groups first. No-op when nothing is baked.
    pub(super) fn draw(&self, pass: &mut wgpu::RenderPass<'_>, index_count: u32, samples: u32) {
        if self.vertex_count == 0 {
            return;
        }
        pass.set_pipeline(self.pipeline.get(samples));
        pass.set_vertex_buffer(0, self.vbuf.slice(..));
        pass.set_index_buffer(self.ibuf.slice(..), wgpu::IndexFormat::Uint32);
        pass.draw_indexed(0..index_count, 0, 0..1);
    }
}

#[cfg(test)]
mod tests;
