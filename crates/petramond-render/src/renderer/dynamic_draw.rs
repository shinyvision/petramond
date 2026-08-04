//! `DynamicDraw`: a per-frame-rewritten vertex(+index) buffer pair for one draw
//! subsystem, collapsing the field group every dynamic subsystem used to spell
//! out by hand (pipeline + vbuf + ibuf + caps + a CPU staging `Vec` + an
//! uploaded count). One `bake` does the shape that was repeated ~7× inline:
//! clear the count, build the geometry, bounds-check against the fixed buffer
//! caps, upload, store the count; `draw` binds + issues the indexed draw.
//!
//! The fixed buffer caps are intentionally per-subsystem (e.g. item-entity vs
//! chest are sized separately so a wall of chests can't make dropped items
//! vanish), so each instance keeps its own caps.

/// An indexed dynamic draw: an owned `{ pipeline, vbuf, ibuf }` plus the fixed
/// caps and the index count uploaded this frame (`0` = nothing to draw). The CPU
/// staging vectors are supplied to [`DynamicDraw::bake`] by the caller — several
/// subsystems (item-entity, chest, break) deliberately SHARE one scratch pair
/// because they bake sequentially, so the scratch lives on the renderer, not
/// here, to preserve that exact reuse.
pub(super) struct DynamicDraw {
    pub pipeline: wgpu::RenderPipeline,
    pub vbuf: wgpu::Buffer,
    pub ibuf: wgpu::Buffer,
    /// Max vertices the fixed `vbuf` holds; a bake that would exceed this is
    /// dropped (count stays 0) so the buffer never reallocates.
    pub vbuf_cap: usize,
    /// Max indices the fixed `ibuf` holds; see `vbuf_cap`.
    pub ibuf_cap: usize,
    /// Index count uploaded this frame (`0` = nothing baked / over budget).
    pub index_count: u32,
}

impl DynamicDraw {
    pub(super) fn new(
        pipeline: wgpu::RenderPipeline,
        vbuf: wgpu::Buffer,
        ibuf: wgpu::Buffer,
        vbuf_cap: u64,
        ibuf_cap: u64,
    ) -> Self {
        Self {
            pipeline,
            vbuf,
            ibuf,
            vbuf_cap: vbuf_cap as usize,
            ibuf_cap: ibuf_cap as usize,
            index_count: 0,
        }
    }

    /// Bake one frame's indexed geometry. Clears the count, runs `build` to fill
    /// the supplied CPU scratch (the build returns the index count it emitted),
    /// and — only if it both produced geometry and fits the fixed caps — uploads
    /// the vertex + index slices and records the count. Otherwise the count stays
    /// 0 (the buffers keep their fixed size; an over-budget frame draws nothing).
    ///
    /// The scratch is passed in (not owned) so subsystems that intentionally
    /// reuse the same `verts`/`indices` across sequential bakes keep doing so.
    pub(super) fn bake<V: bytemuck::Pod>(
        &mut self,
        queue: &wgpu::Queue,
        verts: &mut Vec<V>,
        indices: &mut Vec<u32>,
        build: impl FnOnce(&mut Vec<V>, &mut Vec<u32>) -> u32,
    ) {
        self.index_count = 0;
        let count = build(verts, indices);
        if count > 0 && verts.len() <= self.vbuf_cap && indices.len() <= self.ibuf_cap {
            queue.write_buffer(&self.vbuf, 0, bytemuck::cast_slice(verts));
            queue.write_buffer(&self.ibuf, 0, bytemuck::cast_slice(indices));
            self.index_count = count;
        }
    }

    /// Bind this subsystem's pipeline + vbuf/ibuf and draw its baked index range.
    /// The caller sets any shared bind groups (uniform/atlas) first; this issues
    /// `set_pipeline` + buffers + one `draw_indexed`. No-op when nothing is baked.
    pub(super) fn draw(&self, pass: &mut wgpu::RenderPass<'_>) {
        if self.index_count == 0 {
            return;
        }
        pass.set_pipeline(&self.pipeline);
        pass.set_vertex_buffer(0, self.vbuf.slice(..));
        pass.set_index_buffer(self.ibuf.slice(..), wgpu::IndexFormat::Uint32);
        pass.draw_indexed(0..self.index_count, 0, 0..1);
    }
}

/// A dynamic draw with a STATIC index buffer (built once) and a vertex buffer
/// rewritten each frame — the particle billboards, where the quad indices never
/// change and only the vertex stream is rebuilt. Stores the vertex count baked
/// this frame; the index count is derived per draw.
pub(super) struct DynamicVertexDraw {
    pub pipeline: wgpu::RenderPipeline,
    pub vbuf: wgpu::Buffer,
    /// Static index buffer, uploaded once at construction.
    pub ibuf: wgpu::Buffer,
    pub vbuf_cap: usize,
    /// Vertex count uploaded this frame (`0` = nothing baked / over budget).
    pub vertex_count: u32,
}

impl DynamicVertexDraw {
    pub(super) fn new(
        pipeline: wgpu::RenderPipeline,
        vbuf: wgpu::Buffer,
        ibuf: wgpu::Buffer,
        vbuf_cap: u64,
    ) -> Self {
        Self {
            pipeline,
            vbuf,
            ibuf,
            vbuf_cap: vbuf_cap as usize,
            vertex_count: 0,
        }
    }

    /// Bake one frame's vertex stream. Clears the count, runs `build` to fill the
    /// supplied scratch (returns the vertex count emitted), and uploads + records
    /// it only if it both produced geometry and fits the cap.
    ///
    /// The vertex buffer GROWS on demand (25% headroom, 4 KiB granularity,
    /// bounded by `vbuf_cap` vertices) instead of preallocating the worst case:
    /// the particle caps are ~4 MB of VRAM each, while a quiet scene holds a few
    /// hundred flecks. A shrunken buffer never reallocates downward — particle
    /// load is bursty and the cap bounds the waste.
    pub(super) fn bake<V: bytemuck::Pod>(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        verts: &mut Vec<V>,
        build: impl FnOnce(&mut Vec<V>) -> u32,
    ) {
        self.vertex_count = 0;
        let count = build(verts);
        if count > 0 && verts.len() <= self.vbuf_cap {
            let needed = std::mem::size_of_val(verts.as_slice()) as u64;
            if self.vbuf.size() < needed {
                let max = (self.vbuf_cap * std::mem::size_of::<V>()) as u64;
                let cap = ((needed + needed / 4 + 4095) & !4095).min(max);
                self.vbuf = device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("dynamic vertex draw vbuf"),
                    size: cap,
                    usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                });
            }
            queue.write_buffer(&self.vbuf, 0, bytemuck::cast_slice(verts));
            self.vertex_count = count;
        }
    }

    /// Bind this subsystem's pipeline + vbuf + static ibuf and draw `index_count`
    /// indices (derived by the caller from `vertex_count`). The caller sets shared
    /// bind groups first. No-op when nothing is baked.
    pub(super) fn draw(&self, pass: &mut wgpu::RenderPass<'_>, index_count: u32) {
        if self.vertex_count == 0 {
            return;
        }
        pass.set_pipeline(&self.pipeline);
        pass.set_vertex_buffer(0, self.vbuf.slice(..));
        pass.set_index_buffer(self.ibuf.slice(..), wgpu::IndexFormat::Uint32);
        pass.draw_indexed(0..index_count, 0, 0..1);
    }
}
