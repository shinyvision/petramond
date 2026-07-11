//! GUI-document draw path: uploads a [`petramond_ui::DrawList`] and draws it in
//! the UI pass — every screen's chrome (panels, widgets, text, dim) comes
//! through here; `ui_frame.rs` adds only game-owned content on top.
//!
//! petramond-ui vertices are physical px (y down); the px→NDC conversion happens
//! here on upload so the shared crate stays resolution-agnostic. Batches map
//! `TexId` to bind groups: the theme atlas + font upload once (lazily), and
//! per-batch scissor rects carry the runtime's clip semantics to the GPU.

use super::*;

/// One uploaded batch: which texture, which vertex range, which scissor.
pub(super) struct DocBatch {
    tex: petramond_ui::TexId,
    start: u32,
    count: u32,
    clip: Option<[i32; 4]>,
}

#[derive(Default)]
pub(super) struct DocUi {
    pub(super) batches: Vec<DocBatch>,
    vbuf: Option<wgpu::Buffer>,
    verts: Vec<UiVertex>,
    theme_binds: Option<ThemeBinds>,
    /// This frame's `TexId::DocImage` index → source order.
    frame_images: Vec<crate::gui::DocImageSource>,
    /// Uploaded image textures by path (session-lived; image file changes
    /// need a restart).
    image_binds: HashMap<std::path::PathBuf, wgpu::BindGroup>,
    dynamic_binds: HashMap<String, DynamicBind>,
}

struct ThemeBinds {
    atlas: wgpu::BindGroup,
    font: wgpu::BindGroup,
}

struct DynamicBind {
    revision: u64,
    size: (u32, u32),
    texture: wgpu::Texture,
    bind: wgpu::BindGroup,
}

impl Renderer {
    /// Upload this frame's GUI-document draw list (`None` = no document UI).
    /// `images` is the frame's `TexId::DocImage` index → source order.
    pub(super) fn prepare_doc_ui(
        &mut self,
        document: Option<&super::super::DocumentUiFrame<'_>>,
        screen: (u32, u32),
    ) {
        self.doc_ui.batches.clear();
        self.doc_ui.frame_images.clear();
        let Some(document) = document else {
            return;
        };
        let draw = document.draw;
        self.doc_ui.frame_images.extend_from_slice(document.images);
        self.ensure_doc_image_binds();
        if draw.vertices.is_empty() {
            return;
        }
        if screen.0 == 0 || screen.1 == 0 {
            return;
        }
        self.ensure_doc_theme_binds();

        // px (y down) → NDC (y up); uv/color pass through, including the
        // solid sentinel.
        self.doc_ui.verts.clear();
        self.doc_ui
            .verts
            .extend(draw.vertices.iter().map(|v| UiVertex {
                pos: crate::render::ui::pixel_to_ndc(screen, v.pos[0], v.pos[1]),
                uv: v.uv,
                color: v.color,
            }));
        let bytes: &[u8] = bytemuck::cast_slice(&self.doc_ui.verts);
        let needs = bytes.len() as u64;
        if self.doc_ui.vbuf.as_ref().is_none_or(|b| b.size() < needs) {
            self.doc_ui.vbuf = Some(self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("doc ui vbuf"),
                size: needs.next_power_of_two(),
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }));
        }
        self.queue
            .write_buffer(self.doc_ui.vbuf.as_ref().unwrap(), 0, bytes);

        self.doc_ui
            .batches
            .extend(draw.batches.iter().map(|b| DocBatch {
                tex: b.tex,
                start: b.start,
                count: b.count,
                clip: b.clip,
            }));
    }

    fn ensure_doc_theme_binds(&mut self) {
        if self.doc_ui.theme_binds.is_some() {
            return;
        }
        let theme = crate::gui::doc_theme::theme();
        let atlas = self.doc_texture_bind(&theme.atlas, "doc ui theme atlas");
        let font = self.doc_texture_bind(&theme.font, "doc ui font atlas");
        self.doc_ui.theme_binds = Some(ThemeBinds { atlas, font });
    }

    fn ensure_doc_image_binds(&mut self) {
        let missing: Vec<std::path::PathBuf> = self
            .doc_ui
            .frame_images
            .iter()
            .filter_map(|source| match source {
                crate::gui::DocImageSource::Path(path)
                    if !self.doc_ui.image_binds.contains_key(path) =>
                {
                    Some(path.clone())
                }
                _ => None,
            })
            .collect();
        for path in missing {
            let Ok(img) = image::open(&path) else {
                continue;
            };
            let img = img.to_rgba8();
            let size = img.dimensions();
            let data = petramond_ui::ImageData {
                rgba: img.into_raw(),
                size,
            };
            let bind = self.doc_texture_bind(&data, "doc ui image");
            self.doc_ui.image_binds.insert(path, bind);
        }
        let updates: Vec<_> = self
            .doc_ui
            .frame_images
            .iter()
            .filter_map(|source| match source {
                crate::gui::DocImageSource::Dynamic {
                    key,
                    size,
                    revision,
                    rgba,
                } if self
                    .doc_ui
                    .dynamic_binds
                    .get(key)
                    .is_none_or(|loaded| loaded.revision != *revision) =>
                {
                    Some((key.clone(), *size, *revision, rgba.clone()))
                }
                _ => None,
            })
            .collect();
        for (key, size, revision, rgba) in updates {
            if let Some(existing) = self.doc_ui.dynamic_binds.get_mut(&key) {
                if existing.size == size {
                    write_doc_texture(&self.queue, &existing.texture, size, &rgba);
                    existing.revision = revision;
                    continue;
                }
            }
            let (texture, bind) = self.doc_texture_resources(size, &rgba, "dynamic doc ui image");
            self.doc_ui.dynamic_binds.insert(
                key,
                DynamicBind {
                    revision,
                    size,
                    texture,
                    bind,
                },
            );
        }
    }

    fn doc_texture_bind(&self, image: &petramond_ui::ImageData, label: &str) -> wgpu::BindGroup {
        self.doc_texture_resources(image.size, &image.rgba, label).1
    }

    fn doc_texture_resources(
        &self,
        size: (u32, u32),
        rgba: &[u8],
        label: &str,
    ) -> (wgpu::Texture, wgpu::BindGroup) {
        let (w, h) = size;
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d {
                width: w.max(1),
                height: h.max(1),
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        write_doc_texture(&self.queue, &texture, size, rgba);
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let sampler = self.device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some(label),
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });
        let bind = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some(label),
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
        (texture, bind)
    }

    /// Draw the uploaded document UI inside the UI pass. The pipeline is
    /// already set; each batch binds its texture and scissors its clip.
    pub(super) fn draw_doc_ui(&self, pass: &mut wgpu::RenderPass<'_>) {
        let (Some(vbuf), Some(binds)) = (&self.doc_ui.vbuf, &self.doc_ui.theme_binds) else {
            return;
        };
        let screen = self.prepared_ui_viewport.size;
        pass.set_vertex_buffer(0, vbuf.slice(..));
        for batch in &self.doc_ui.batches {
            let bind =
                match batch.tex {
                    petramond_ui::TexId::Solid => &self.icon_atlas.bind,
                    petramond_ui::TexId::ThemeAtlas => &binds.atlas,
                    petramond_ui::TexId::Font => &binds.font,
                    petramond_ui::TexId::DocImage(i) => {
                        match self.doc_ui.frame_images.get(i as usize).and_then(|source| {
                            match source {
                                crate::gui::DocImageSource::Path(path) => {
                                    self.doc_ui.image_binds.get(path)
                                }
                                crate::gui::DocImageSource::Dynamic { key, .. } => {
                                    self.doc_ui.dynamic_binds.get(key).map(|entry| &entry.bind)
                                }
                            }
                        }) {
                            Some(bind) => bind,
                            None => continue,
                        }
                    }
                };
            match batch.clip {
                Some([x, y, w, h]) => {
                    let x0 = x.clamp(0, screen.0 as i32) as u32;
                    let y0 = y.clamp(0, screen.1 as i32) as u32;
                    let x1 = (x + w).clamp(0, screen.0 as i32) as u32;
                    let y1 = (y + h).clamp(0, screen.1 as i32) as u32;
                    if x1 <= x0 || y1 <= y0 {
                        continue;
                    }
                    pass.set_scissor_rect(x0, y0, x1 - x0, y1 - y0);
                }
                None => pass.set_scissor_rect(0, 0, screen.0, screen.1),
            }
            pass.set_bind_group(0, bind, &[]);
            pass.draw(batch.start..batch.start + batch.count, 0..1);
        }
        pass.set_scissor_rect(0, 0, screen.0, screen.1);
    }
}

fn write_doc_texture(queue: &wgpu::Queue, texture: &wgpu::Texture, size: (u32, u32), rgba: &[u8]) {
    let (w, h) = size;
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
            bytes_per_row: Some(4 * w.max(1)),
            rows_per_image: Some(h.max(1)),
        },
        wgpu::Extent3d {
            width: w.max(1),
            height: h.max(1),
            depth_or_array_layers: 1,
        },
    );
}
