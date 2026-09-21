//! Schematic thumbnails: a native render of a scene through an isolated
//! camera and depth target, read back and PNG-encoded wherever the caller
//! runs it — never the frame thread.

use super::Renderer;
use crate::{schematic::Geometry, uniforms::Uniforms};
use glam::{Mat4, Vec3};
use wgpu::util::DeviceExt;

/// Cloned device and pipeline handles, free to render on a worker.
pub struct SchematicThumbnailer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    blocks: crate::pipeline::SampledPipeline,
    models: crate::pipeline::SampledPipeline,
    block_atlas: wgpu::BindGroup,
    model_atlas: wgpu::BindGroup,
    format: wgpu::TextureFormat,
}

impl SchematicThumbnailer {
    fn new(r: &Renderer) -> Self {
        Self {
            device: r.device.clone(),
            queue: r.queue.clone(),
            blocks: r.block_entity.chest_draw.pipeline.clone(),
            models: r.world_model_pipe.clone(),
            block_atlas: r.atlas_array_bind.clone(),
            model_atlas: r.model_atlas_bind.clone(),
            format: r.config.format,
        }
    }
    /// Mesh `scene` and render it to PNG bytes. Blocks on the GPU readback.
    pub fn render(&self, scene: &petramond::schematic::Scene) -> Result<Vec<u8>, String> {
        self.render_geometry(&Geometry::build(scene))
    }

    fn render_geometry(&self, geometry: &Geometry) -> Result<Vec<u8>, String> {
        let (width, height) = (384u32, 256u32);
        let size = Vec3::from_array(geometry.size.map(|s| s as f32));
        let center = size * 0.5;
        let radius = (size.length() * 0.5 + 1.0).max(2.0);
        let eye = center + Vec3::new(1.0, 0.75, 1.0).normalize() * radius * 3.0;
        let vp = Mat4::orthographic_rh(
            -radius * 1.5,
            radius * 1.5,
            -radius,
            radius,
            0.1,
            radius * 6.0,
        ) * Mat4::look_at_rh(eye, center, Vec3::Y);
        let u = Uniforms {
            view_proj: vp.to_cols_array_2d(),
            inv_view_proj: vp.inverse().to_cols_array_2d(),
            cam_pos: eye.extend(0.0).to_array(),
            fog: [10000.0, 20000.0, 0.0, 0.0],
            fog_color: [0.3, 0.4, 0.5, 1.0],
            render_origin: [0; 4],
            atlas_layout: crate::atlas::atlas_layout_uniform(),
            sky_color: [1.0; 4],
            sun_dir: [0.3, 0.9, 0.3, 1.0],
            volume_tint: [1.0; 4],
        };
        let buffer = |label, bytes: &[u8], usage| {
            self.device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some(label),
                    contents: if bytes.is_empty() { &[0; 4] } else { bytes },
                    usage,
                })
        };
        let ub = buffer(
            "schematic camera",
            bytemuck::bytes_of(&u),
            wgpu::BufferUsages::UNIFORM,
        );
        let uv = buffer(
            "schematic uv table",
            &vec![0u8; crate::uniforms::UV_RECTS_LEN * 16],
            wgpu::BufferUsages::UNIFORM,
        );
        let bind = crate::selection_highlight::inactive_bind(
            &self.device,
            &self.blocks.get(1).get_bind_group_layout(0),
            &ub,
            &uv,
        );
        let vertices = buffer(
            "schematic blocks",
            bytemuck::cast_slice(&geometry.blocks),
            wgpu::BufferUsages::VERTEX,
        );
        let indices = buffer(
            "schematic block indices",
            bytemuck::cast_slice(&geometry.block_indices),
            wgpu::BufferUsages::INDEX,
        );
        let models = buffer(
            "schematic models",
            bytemuck::cast_slice(&geometry.models),
            wgpu::BufferUsages::VERTEX,
        );
        let model_indices = buffer(
            "schematic model indices",
            bytemuck::cast_slice(&geometry.model_indices),
            wgpu::BufferUsages::INDEX,
        );
        let origin = buffer(
            "schematic origin",
            bytemuck::cast_slice(&[0i32; 4]),
            wgpu::BufferUsages::VERTEX,
        );
        let extent = wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        };
        let color = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("schematic screenshot"),
            size: extent,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: self.format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let depth = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("schematic depth"),
            size: extent,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Depth32Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let mut enc = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        {
            let cv = color.create_view(&Default::default());
            let dv = depth.create_view(&Default::default());
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("schematic screenshot"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &cv,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.055,
                            g: 0.08,
                            b: 0.10,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &dv,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Discard,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_bind_group(0, &bind, &[]);
            if !geometry.block_indices.is_empty() {
                pass.set_pipeline(self.blocks.get(1));
                pass.set_bind_group(1, &self.block_atlas, &[]);
                pass.set_vertex_buffer(0, vertices.slice(..));
                pass.set_index_buffer(indices.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..geometry.block_indices.len() as u32, 0, 0..1);
            }
            if !geometry.model_indices.is_empty() {
                pass.set_pipeline(self.models.get(1));
                pass.set_bind_group(1, &self.model_atlas, &[]);
                pass.set_vertex_buffer(0, models.slice(..));
                pass.set_vertex_buffer(1, origin.slice(..));
                pass.set_index_buffer(model_indices.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..geometry.model_indices.len() as u32, 0, 0..1);
            }
        }
        let row = (width * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
        let readback = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("schematic readback"),
            size: u64::from(row * height),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        enc.copy_texture_to_buffer(
            color.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(row),
                    rows_per_image: None,
                },
            },
            extent,
        );
        self.queue.submit([enc.finish()]);
        let (tx, rx) = std::sync::mpsc::channel();
        readback.slice(..).map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        self.device
            .poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: Some(std::time::Duration::from_secs(30)),
            })
            .map_err(|e| e.to_string())?;
        rx.recv()
            .map_err(|e| e.to_string())?
            .map_err(|e| e.to_string())?;
        let mapped = readback.slice(..).get_mapped_range();
        let mut rgba = Vec::with_capacity((width * height * 4) as usize);
        for line in mapped.chunks_exact(row as usize) {
            rgba.extend_from_slice(&line[..(width * 4) as usize]);
        }
        drop(mapped);
        readback.unmap();
        if matches!(
            self.format,
            wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb
        ) {
            for p in rgba.chunks_exact_mut(4) {
                p.swap(0, 2);
            }
        }
        use image::ImageEncoder;
        let mut png = Vec::new();
        image::codecs::png::PngEncoder::new_with_quality(
            &mut png,
            image::codecs::png::CompressionType::Default,
            image::codecs::png::FilterType::Adaptive,
        )
        .write_image(&rgba, width, height, image::ExtendedColorType::Rgba8)
        .map_err(|e| e.to_string())?;
        Ok(png)
    }
}

impl Renderer {
    pub fn schematic_thumbnailer(&self) -> SchematicThumbnailer {
        SchematicThumbnailer::new(self)
    }
}

#[cfg(test)]
mod tests;
