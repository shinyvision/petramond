use std::ops::Range;

const INITIAL_BYTES: u64 = 4096;

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

fn grown_size(needed: u64) -> u64 {
    (needed + needed / 4).div_ceil(INITIAL_BYTES) * INITIAL_BYTES
}

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

fn buffer_labels(label: &str) -> (String, String) {
    (format!("{label} vbuf"), format!("{label} ibuf"))
}

pub(super) struct DynamicDraw {
    pub pipeline: crate::pipeline::SampledPipeline,
    pub vbuf: wgpu::Buffer,
    pub ibuf: wgpu::Buffer,
    vbuf_label: String,
    ibuf_label: String,
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

pub(crate) fn prim_index_list(pattern: &[u32], verts_per_prim: u32, prims: u32) -> Vec<u32> {
    let mut out = Vec::with_capacity(pattern.len() * prims as usize);
    for prim in 0..prims {
        let base = prim * verts_per_prim;
        out.extend(pattern.iter().map(|i| base + i));
    }
    out
}

fn prims_to_index(vbuf_bytes: u64, prim_bytes: u64, prims: u32) -> u32 {
    let capacity = (vbuf_bytes / prim_bytes.max(1)) as u32;
    capacity.max(prims)
}

pub(super) struct DynamicVertexDraw {
    pub pipeline: crate::pipeline::SampledPipeline,
    pub vbuf: wgpu::Buffer,
    pub ibuf: wgpu::Buffer,
    vbuf_label: String,
    ibuf_label: String,
    verts_per_prim: u32,
    pattern: &'static [u32],
    prims_indexed: u32,
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
}

/// Instanced version of the dynamic draw, for particles. We only upload one row `I` per primitive
/// each frame and let the vertex stage expand it from `vertex_index`. The index `pattern` is
/// uploaded once when this is built.
pub(super) struct DynamicInstanceDraw<I> {
    pub pipeline: crate::pipeline::SampledPipeline,
    ibuf: wgpu::Buffer,
    index_count: u32,
    instances: wgpu::Buffer,
    label: String,
    pub instance_count: u32,
    _row: std::marker::PhantomData<I>,
}

impl<I: bytemuck::Pod> DynamicInstanceDraw<I> {
    pub(super) fn new(
        device: &wgpu::Device,
        pipeline: crate::pipeline::SampledPipeline,
        label: &'static str,
        pattern: &[u32],
    ) -> Self {
        use wgpu::util::DeviceExt;
        let label = format!("{label} instances");
        Self {
            pipeline,
            ibuf: device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some(&format!("{label} pattern")),
                contents: bytemuck::cast_slice(pattern),
                usage: wgpu::BufferUsages::INDEX,
            }),
            index_count: pattern.len() as u32,
            instances: new_buffer(device, wgpu::BufferUsages::VERTEX, &label),
            label,
            instance_count: 0,
            _row: std::marker::PhantomData,
        }
    }

    pub(super) fn bake(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        rows: &mut Vec<I>,
        build: impl FnOnce(&mut Vec<I>) -> u32,
    ) {
        self.instance_count = 0;
        let count = build(rows);
        if count == 0 {
            return;
        }
        upload(
            device,
            queue,
            &mut self.instances,
            rows,
            wgpu::BufferUsages::VERTEX,
            &self.label,
        );
        self.instance_count = count;
    }

    pub(super) fn draw(&self, pass: &mut wgpu::RenderPass<'_>, samples: u32, range: Range<u32>) {
        if range.is_empty() {
            return;
        }
        let stride = std::mem::size_of::<I>() as u64;
        pass.set_pipeline(self.pipeline.get(samples));
        pass.set_vertex_buffer(
            0,
            self.instances
                .slice(range.start as u64 * stride..range.end as u64 * stride),
        );
        pass.set_index_buffer(self.ibuf.slice(..), wgpu::IndexFormat::Uint32);
        pass.draw_indexed(0..self.index_count, 0, 0..range.len() as u32);
    }
}

#[cfg(test)]
mod tests;
