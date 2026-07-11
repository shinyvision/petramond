//! Physical-pixel client-WASM overlays and modal canvases.
//!
//! These images deliberately bypass GUI documents and `gui_scale`: placement
//! and display dimensions are physical screen pixels. The renderer owns upload
//! caching and draws nearest-sampled quads in the ordinary depthless UI pass.

use super::*;

#[derive(Default)]
pub(super) struct ClientOverlays {
    pub(super) batches: Vec<OverlayBatch>,
    vbuf: Option<wgpu::Buffer>,
    verts: Vec<UiVertex>,
    binds: Vec<(String, OverlayBind)>,
}

pub(super) struct OverlayBatch {
    bind_index: Option<usize>,
    start: u32,
    count: u32,
}

struct OverlayBind {
    revision: u64,
    size: (u16, u16),
    texture: wgpu::Texture,
    bind: wgpu::BindGroup,
}

impl Renderer {
    pub(super) fn prepare_client_overlays(
        &mut self,
        images: &[super::super::ClientOverlayImage],
        screen: (u32, u32),
        dim_background: bool,
    ) {
        self.client_overlays.batches.clear();
        self.client_overlays.verts.clear();
        if screen.0 == 0 || screen.1 == 0 {
            return;
        }

        if dim_background {
            let start = self.client_overlays.verts.len() as u32;
            crate::render::ui::push_solid(
                &mut self.client_overlays.verts,
                screen,
                0.0,
                0.0,
                screen.0 as f32,
                screen.1 as f32,
                [0.0, 0.0, 0.0, 0.55],
            );
            self.client_overlays.batches.push(OverlayBatch {
                bind_index: None,
                start,
                count: 6,
            });
        }

        for image in images {
            let bind_index = self.ensure_client_overlay_bind(image);
            let start = self.client_overlays.verts.len() as u32;
            crate::render::ui::push_quad_uv(
                &mut self.client_overlays.verts,
                screen,
                image.rect[0],
                image.rect[1],
                image.rect[2],
                image.rect[3],
                [image.uv[0], image.uv[1]],
                [image.uv[2], image.uv[3]],
                [1.0; 4],
            );
            self.client_overlays.batches.push(OverlayBatch {
                bind_index: Some(bind_index),
                start,
                count: 6,
            });
        }

        let bytes = bytemuck::cast_slice::<_, u8>(&self.client_overlays.verts);
        if bytes.is_empty() {
            return;
        }
        let needs = bytes.len() as u64;
        if self
            .client_overlays
            .vbuf
            .as_ref()
            .is_none_or(|buffer| buffer.size() < needs)
        {
            self.client_overlays.vbuf = Some(self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("client overlay vbuf"),
                size: needs.next_power_of_two(),
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }));
        }
        self.queue
            .write_buffer(self.client_overlays.vbuf.as_ref().unwrap(), 0, bytes);
    }

    fn ensure_client_overlay_bind(&mut self, image: &super::super::ClientOverlayImage) -> usize {
        if let Some(index) = self
            .client_overlays
            .binds
            .iter()
            .position(|(key, _)| key == &image.key)
        {
            let existing = &mut self.client_overlays.binds[index].1;
            if existing.size == image.size {
                if existing.revision != image.revision {
                    write_overlay_texture(&self.queue, &existing.texture, image.size, &image.rgba);
                    existing.revision = image.revision;
                }
                return index;
            }
        }

        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("client overlay image"),
            size: wgpu::Extent3d {
                width: image.size.0 as u32,
                height: image.size.1 as u32,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        write_overlay_texture(&self.queue, &texture, image.size, &image.rgba);
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let sampler = self.device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("client overlay image"),
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });
        let bind = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("client overlay image"),
            layout: &self.ui_texture_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
            ],
        });
        if let Some(index) = self
            .client_overlays
            .binds
            .iter()
            .position(|(key, _)| key == &image.key)
        {
            self.client_overlays.binds[index].1 = OverlayBind {
                revision: image.revision,
                size: image.size,
                texture,
                bind,
            };
            return index;
        }
        self.client_overlays.binds.push((
            image.key.clone(),
            OverlayBind {
                revision: image.revision,
                size: image.size,
                texture,
                bind,
            },
        ));
        self.client_overlays.binds.len() - 1
    }

    pub(super) fn draw_client_overlays(&self, pass: &mut wgpu::RenderPass<'_>) {
        let Some(vbuf) = &self.client_overlays.vbuf else {
            return;
        };
        pass.set_vertex_buffer(0, vbuf.slice(..));
        for batch in &self.client_overlays.batches {
            let bind = match batch.bind_index {
                Some(index) => match self.client_overlays.binds.get(index) {
                    Some((_, image)) => &image.bind,
                    None => continue,
                },
                None => &self.icon_atlas.bind,
            };
            pass.set_bind_group(0, bind, &[]);
            pass.draw(batch.start..batch.start + batch.count, 0..1);
        }
    }
}

fn write_overlay_texture(
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    size: (u16, u16),
    rgba: &[u8],
) {
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        rgba,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(4 * size.0 as u32),
            rows_per_image: Some(size.1 as u32),
        },
        wgpu::Extent3d {
            width: size.0 as u32,
            height: size.1 as u32,
            depth_or_array_layers: 1,
        },
    );
}
