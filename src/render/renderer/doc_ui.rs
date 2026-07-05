//! GUI-document draw path: uploads a [`llama_ui::DrawList`] and draws it in
//! the UI pass — every screen's chrome (panels, widgets, text, dim) comes
//! through here; `ui_frame.rs` adds only game-owned content on top.
//!
//! llama-ui vertices are physical px (y down); the px→NDC conversion happens
//! here on upload so the shared crate stays resolution-agnostic. Batches map
//! `TexId` to bind groups: the theme atlas + font upload once (lazily), and
//! per-batch scissor rects carry the runtime's clip semantics to the GPU.

use super::*;

/// One uploaded batch: which texture, which vertex range, which scissor.
pub(super) struct DocBatch {
    tex: llama_ui::TexId,
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
    /// This frame's `TexId::DocImage` index → image path order.
    frame_paths: Vec<std::path::PathBuf>,
    /// Uploaded image textures by path (session-lived; image file changes
    /// need a restart).
    image_binds: HashMap<std::path::PathBuf, wgpu::BindGroup>,
}

struct ThemeBinds {
    atlas: wgpu::BindGroup,
    font: wgpu::BindGroup,
}

impl Renderer {
    /// Upload this frame's GUI-document draw list (`None` = no document UI).
    /// `images` is the frame's `TexId::DocImage` index → path order.
    pub fn set_doc_ui(&mut self, draw: Option<(&llama_ui::DrawList, &[std::path::PathBuf])>) {
        self.doc_ui.batches.clear();
        self.doc_ui.frame_paths.clear();
        let Some((draw, images)) = draw else {
            return;
        };
        self.doc_ui.frame_paths.extend_from_slice(images);
        self.ensure_doc_image_binds();
        if draw.vertices.is_empty() {
            return;
        }
        let screen = self.ui.screen;
        if screen.0 == 0 || screen.1 == 0 {
            return;
        }
        self.ensure_doc_theme_binds();

        // px (y down) → NDC (y up); uv/color pass through, including the
        // solid sentinel.
        let (w, h) = (screen.0 as f32, screen.1 as f32);
        self.doc_ui.verts.clear();
        self.doc_ui
            .verts
            .extend(draw.vertices.iter().map(|v| UiVertex {
                pos: [v.pos[0] / w * 2.0 - 1.0, 1.0 - v.pos[1] / h * 2.0],
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
            .frame_paths
            .iter()
            .filter(|p| !self.doc_ui.image_binds.contains_key(*p))
            .cloned()
            .collect();
        for path in missing {
            let Ok(img) = image::open(&path) else {
                continue;
            };
            let img = img.to_rgba8();
            let size = img.dimensions();
            let data = llama_ui::ImageData {
                rgba: img.into_raw(),
                size,
            };
            let bind = self.doc_texture_bind(&data, "doc ui image");
            self.doc_ui.image_binds.insert(path, bind);
        }
    }

    fn doc_texture_bind(&self, image: &llama_ui::ImageData, label: &str) -> wgpu::BindGroup {
        let (w, h) = image.size;
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
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &image.rgba,
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
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let sampler = self.device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some(label),
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });
        self.device.create_bind_group(&wgpu::BindGroupDescriptor {
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
        })
    }

    /// Draw the uploaded document UI inside the UI pass. The pipeline is
    /// already set; each batch binds its texture and scissors its clip.
    pub(super) fn draw_doc_ui(&self, pass: &mut wgpu::RenderPass<'_>) {
        let (Some(vbuf), Some(binds)) = (&self.doc_ui.vbuf, &self.doc_ui.theme_binds) else {
            return;
        };
        let screen = self.ui.screen;
        pass.set_vertex_buffer(0, vbuf.slice(..));
        for batch in &self.doc_ui.batches {
            let bind = match batch.tex {
                llama_ui::TexId::Solid => &self.icon_atlas.bind,
                llama_ui::TexId::ThemeAtlas => &binds.atlas,
                llama_ui::TexId::Font => &binds.font,
                llama_ui::TexId::DocImage(i) => {
                    match self
                        .doc_ui
                        .frame_paths
                        .get(i as usize)
                        .and_then(|p| self.doc_ui.image_binds.get(p))
                    {
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
