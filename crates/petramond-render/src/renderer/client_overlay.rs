use super::*;

#[derive(Default)]
pub(super) struct ClientOverlays {
    pub(super) batches: Vec<OverlayBatch>,
    vbuf: Option<wgpu::Buffer>,
    verts: Vec<UiVertex>,
    images: ClientImageBinds,
}

#[derive(Default)]
pub(super) struct ClientImageBinds {
    binds: Vec<(String, OverlayBind)>,
}

pub(super) struct OverlayBatch {
    tex: OverlayTex,
    start: u32,
    count: u32,
    clip: Option<[i32; 4]>,
}

#[derive(Copy, Clone)]
enum OverlayTex {
    Image(usize),
    Solid,
    Font,
}

struct OverlayBind {
    revision: u64,
    size: (u16, u16),
    texture: wgpu::Texture,
    bind: wgpu::BindGroup,
}

impl ClientOverlays {
    pub(super) fn prepare(
        &mut self,
        gpu: &super::doc_ui::UiGpu<'_>,
        theme: &mut Option<super::doc_ui::ThemeBinds>,
        layer: &super::super::ClientOverlayLayer,
        screen: (u32, u32),
        dim_background: bool,
    ) {
        self.batches.clear();
        self.verts.clear();
        self.images
            .retain_keys(layer.items.iter().filter_map(|item| match item {
                super::super::ClientOverlayItem::Image(image) => Some(image.key.as_str()),
                super::super::ClientOverlayItem::Paint { .. } => None,
            }));
        if screen.0 == 0 || screen.1 == 0 {
            return;
        }

        if dim_background {
            let start = self.verts.len() as u32;
            crate::ui::push_solid(
                &mut self.verts,
                screen,
                0.0,
                0.0,
                screen.0 as f32,
                screen.1 as f32,
                [0.0, 0.0, 0.0, 0.55],
            );
            self.batches.push(OverlayBatch {
                tex: OverlayTex::Solid,
                start,
                count: 6,
                clip: None,
            });
        }

        for item in &layer.items {
            let image = match item {
                super::super::ClientOverlayItem::Image(image) => image,
                super::super::ClientOverlayItem::Paint { batches } => {
                    if self.push_paint(&layer.paint, batches.clone(), screen) {
                        gpu.theme(theme);
                    }
                    continue;
                }
            };
            let bind_index = self.images.ensure(gpu, image);
            let start = self.verts.len() as u32;
            crate::ui::push_quad_uv(
                &mut self.verts,
                screen,
                image.rect[0],
                image.rect[1],
                image.rect[2],
                image.rect[3],
                [image.uv[0], image.uv[1]],
                [image.uv[2], image.uv[3]],
                [1.0; 4],
            );
            self.batches.push(OverlayBatch {
                tex: OverlayTex::Image(bind_index),
                start,
                count: 6,
                clip: None,
            });
        }

        let bytes = bytemuck::cast_slice::<_, u8>(&self.verts);
        if bytes.is_empty() {
            return;
        }
        let needs = bytes.len() as u64;
        if self
            .vbuf
            .as_ref()
            .is_none_or(|buffer| buffer.size() < needs)
        {
            self.vbuf = Some(gpu.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("client overlay vbuf"),
                size: needs.next_power_of_two(),
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }));
        }
        gpu.queue
            .write_buffer(self.vbuf.as_ref().unwrap(), 0, bytes);
    }

    fn push_paint(
        &mut self,
        paint: &petramond_ui::DrawList,
        batches: std::ops::Range<usize>,
        screen: (u32, u32),
    ) -> bool {
        let Some(batches) = paint.batches.get(batches) else {
            return false;
        };
        let mut font = false;
        for batch in batches {
            let tex = match batch.tex {
                petramond_ui::TexId::Solid => OverlayTex::Solid,
                petramond_ui::TexId::Font => OverlayTex::Font,
                petramond_ui::TexId::ThemePage(_) | petramond_ui::TexId::DocImage(_) => continue,
            };
            font |= matches!(tex, OverlayTex::Font);
            let range = batch.start as usize..(batch.start + batch.count) as usize;
            let Some(verts) = paint.vertices.get(range) else {
                continue;
            };
            let start = self.verts.len() as u32;
            self.verts.extend(verts.iter().map(|v| UiVertex {
                pos: crate::ui::pixel_to_ndc(screen, v.pos[0], v.pos[1]),
                uv: v.uv,
                color: v.color,
            }));
            self.batches.push(OverlayBatch {
                tex,
                start,
                count: batch.count,
                clip: batch.clip,
            });
        }
        font
    }

    pub(super) fn draw(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        theme: Option<&super::doc_ui::ThemeBinds>,
        solid: &wgpu::BindGroup,
        screen: (u32, u32),
    ) {
        let Some(vbuf) = &self.vbuf else {
            return;
        };
        pass.set_vertex_buffer(0, vbuf.slice(..));
        for batch in &self.batches {
            let bind = match batch.tex {
                OverlayTex::Image(index) => match self.images.bind(index) {
                    Some(bind) => bind,
                    None => continue,
                },
                OverlayTex::Solid => solid,
                OverlayTex::Font => match theme {
                    Some(theme) => &theme.font,
                    None => continue,
                },
            };
            if !super::doc_ui::set_batch_scissor(pass, batch.clip, screen) {
                continue;
            }
            pass.set_bind_group(0, bind, &[]);
            pass.draw(batch.start..batch.start + batch.count, 0..1);
        }
        pass.set_scissor_rect(0, 0, screen.0, screen.1);
    }
}

impl ClientImageBinds {
    pub(super) fn retain_keys<'a>(&mut self, drawn: impl IntoIterator<Item = &'a str>) {
        let drawn: Vec<&str> = drawn.into_iter().collect();
        self.binds.retain(|(key, _)| drawn.contains(&key.as_str()));
    }

    pub(super) fn bind(&self, index: usize) -> Option<&wgpu::BindGroup> {
        self.binds.get(index).map(|(_, image)| &image.bind)
    }

    pub(super) fn ensure(
        &mut self,
        gpu: &super::doc_ui::UiGpu<'_>,
        image: &super::super::ClientOverlayImage,
    ) -> usize {
        if let Some(index) = self.binds.iter().position(|(key, _)| key == &image.key) {
            let existing = &mut self.binds[index].1;
            if existing.size == image.size {
                if existing.revision != image.revision {
                    let chain_covers = image
                        .recent_blits
                        .first()
                        .is_some_and(|&(oldest, _)| existing.revision >= oldest.saturating_sub(1));
                    if chain_covers {
                        for &(revision, rect) in &image.recent_blits {
                            if revision > existing.revision {
                                write_overlay_texture_rect(
                                    gpu.queue,
                                    &existing.texture,
                                    image.size,
                                    &image.rgba,
                                    rect,
                                );
                            }
                        }
                    } else {
                        write_overlay_texture(
                            gpu.queue,
                            &existing.texture,
                            image.size,
                            &image.rgba,
                        );
                    }
                    existing.revision = image.revision;
                }
                return index;
            }
        }

        let texture = crate::gpu_mem::create_texture(
            gpu.device,
            &wgpu::TextureDescriptor {
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
            },
        );
        write_overlay_texture(gpu.queue, &texture, image.size, &image.rgba);
        let bind = gpu.nearest_bind(&texture, "client overlay image");
        let entry = OverlayBind {
            revision: image.revision,
            size: image.size,
            texture,
            bind,
        };
        if let Some(index) = self.binds.iter().position(|(key, _)| key == &image.key) {
            self.binds[index].1 = entry;
            return index;
        }
        self.binds.push((image.key.clone(), entry));
        self.binds.len() - 1
    }
}

fn write_overlay_texture_rect(
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    size: (u16, u16),
    rgba: &[u8],
    rect: [u16; 4],
) {
    let [x, y, w, h] = rect.map(|v| v as u32);
    if w == 0 || h == 0 || x + w > size.0 as u32 || y + h > size.1 as u32 {
        return;
    }
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d { x, y, z: 0 },
            aspect: wgpu::TextureAspect::All,
        },
        rgba,
        wgpu::TexelCopyBufferLayout {
            offset: (y as u64 * size.0 as u64 + x as u64) * 4,
            bytes_per_row: Some(4 * size.0 as u32),
            rows_per_image: Some(h),
        },
        wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
    );
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
