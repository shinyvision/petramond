use std::ops::Range;

use wgpu::util::DeviceExt;

use super::dynamic_draw::{new_buffer, upload};
use crate::skinned::{SkinBatch, SkinInstance, SkinMesh};

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

pub(super) struct SkinFrame {
    pipeline: crate::pipeline::SampledPipeline,
    palette_bgl: wgpu::BindGroupLayout,
    palette: wgpu::Buffer,
    palette_bind: wgpu::BindGroup,
    instances: wgpu::Buffer,
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
