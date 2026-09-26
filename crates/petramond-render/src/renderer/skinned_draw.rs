//! The GPU side of skinned bodies ([`crate::skinned`]): each model's static
//! bind-space mesh uploaded ONCE at construction, and the frame's bone
//! palette + instance rows — one pair of growable buffers shared by every
//! model — uploaded once per frame. A model's visible bodies draw as one
//! instanced call over their contiguous instance range.

use std::ops::Range;

use wgpu::util::DeviceExt;

use super::dynamic_draw::{new_buffer, upload};
use crate::skinned::{SkinBatch, SkinInstance, SkinMesh};

/// One model's static skinned mesh on the GPU.
pub(super) struct SkinnedModel {
    vbuf: wgpu::Buffer,
    ibuf: wgpu::Buffer,
    index_count: u32,
}

impl SkinnedModel {
    pub(super) fn new(device: &wgpu::Device, mesh: &SkinMesh, label: &str) -> Self {
        let vbuf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some(&format!("{label} skin vbuf")),
            contents: bytemuck::cast_slice(&mesh.verts),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let ibuf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some(&format!("{label} skin ibuf")),
            contents: bytemuck::cast_slice(&mesh.indices),
            usage: wgpu::BufferUsages::INDEX,
        });
        Self {
            vbuf,
            ibuf,
            index_count: mesh.index_count(),
        }
    }
}

/// This frame's skinned bodies across every model: the CPU batch the pose
/// step fills, and the buffers it uploads to.
pub(super) struct SkinFrame {
    pipeline: crate::pipeline::SampledPipeline,
    palette_bgl: wgpu::BindGroupLayout,
    palette: wgpu::Buffer,
    /// group(2) over `palette`; rebuilt whenever the palette buffer grows.
    palette_bind: wgpu::BindGroup,
    instances: wgpu::Buffer,
    /// The frame's palette + instance rows, filled by the pose step.
    pub(super) batch: SkinBatch,
}

const PALETTE_LABEL: &str = "bone palette";
const INSTANCE_LABEL: &str = "skin instances";

impl SkinFrame {
    pub(super) fn new(
        device: &wgpu::Device,
        pipeline: crate::pipeline::SampledPipeline,
        palette_bgl: wgpu::BindGroupLayout,
    ) -> Self {
        let palette = new_buffer(device, wgpu::BufferUsages::STORAGE, PALETTE_LABEL);
        let palette_bind = palette_bind(device, &palette_bgl, &palette);
        Self {
            pipeline,
            palette_bgl,
            palette,
            palette_bind,
            instances: new_buffer(device, wgpu::BufferUsages::VERTEX, INSTANCE_LABEL),
            batch: SkinBatch::default(),
        }
    }

    /// Upload the batch (growing the buffers to fit). A frame with no bodies
    /// uploads nothing; its models' ranges are empty and draw nothing.
    pub(super) fn upload(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        if self.batch.instances.is_empty() {
            return;
        }
        let before = self.palette.size();
        upload(
            device,
            queue,
            &mut self.palette,
            &self.batch.palette,
            wgpu::BufferUsages::STORAGE,
            PALETTE_LABEL,
        );
        if self.palette.size() != before {
            self.palette_bind = palette_bind(device, &self.palette_bgl, &self.palette);
        }
        upload(
            device,
            queue,
            &mut self.instances,
            &self.batch.instances,
            wgpu::BufferUsages::VERTEX,
            INSTANCE_LABEL,
        );
    }

    /// Draw `model` once per instance in `range`. The caller binds group(0)
    /// (world uniforms) and group(1) (the model's texture) first. No-op for an
    /// empty range or a model without faces.
    pub(super) fn draw(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        samples: u32,
        model: &SkinnedModel,
        range: Range<u32>,
    ) {
        if range.is_empty() || model.index_count == 0 {
            return;
        }
        let stride = std::mem::size_of::<SkinInstance>() as u64;
        pass.set_pipeline(self.pipeline.get(samples));
        pass.set_bind_group(2, &self.palette_bind, &[]);
        pass.set_vertex_buffer(0, model.vbuf.slice(..));
        // The model's rows start at `range.start`: offsetting the instance
        // stream there (rather than a non-zero first instance) keeps the draw
        // valid on every backend.
        pass.set_vertex_buffer(
            1,
            self.instances
                .slice(range.start as u64 * stride..range.end as u64 * stride),
        );
        pass.set_index_buffer(model.ibuf.slice(..), wgpu::IndexFormat::Uint32);
        pass.draw_indexed(0..model.index_count, 0, 0..range.len() as u32);
    }
}

fn palette_bind(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    palette: &wgpu::Buffer,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("bone palette bg"),
        layout,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: palette.as_entire_binding(),
        }],
    })
}
