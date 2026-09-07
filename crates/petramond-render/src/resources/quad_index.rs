/// The process-wide index buffer every implied-triangulation terrain draw uses
/// (see [`petramond_mesh::QuadIdx`]): `0,1,2, 0,2,3` repeated, so a draw is
/// `draw_indexed(0..6*quads, base_vertex = the section's first vertex)`.
///
/// It replaces a per-column index allocation for the opaque and far-LOD streams
/// — 122 MiB of VRAM at render distance 32, and the same again on the CPU side,
/// for one buffer of a couple of megabytes.
pub(crate) struct QuadIndexBuffer {
    buf: wgpu::Buffer,
    quads: u32,
}

/// Quads the shared index buffer covers on creation. Sized so an ordinary
/// column's whole-column opaque draw never has to grow it.
const QUAD_INDEX_INITIAL: u32 = 1 << 16;

impl QuadIndexBuffer {
    pub(crate) fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Self {
        Self {
            buf: Self::build(device, queue, QUAD_INDEX_INITIAL),
            quads: QUAD_INDEX_INITIAL,
        }
    }

    fn build(device: &wgpu::Device, queue: &wgpu::Queue, quads: u32) -> wgpu::Buffer {
        let mut data: Vec<u32> = Vec::with_capacity(quads as usize * 6);
        for q in 0..quads {
            let b = q * 4;
            data.extend_from_slice(&[b, b + 1, b + 2, b, b + 2, b + 3]);
        }
        let buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("shared quad index buffer"),
            size: (data.len() * 4) as u64,
            usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&buf, 0, bytemuck::cast_slice(&data));
        buf
    }

    /// Guarantee the buffer covers a draw of `quads` quads.
    pub(crate) fn ensure(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, quads: u32) {
        if quads <= self.quads {
            return;
        }
        let grown = quads.next_power_of_two().max(self.quads * 2);
        self.buf = Self::build(device, queue, grown);
        self.quads = grown;
    }

    pub(crate) fn slice(&self) -> wgpu::BufferSlice<'_> {
        self.buf.slice(..)
    }
}
