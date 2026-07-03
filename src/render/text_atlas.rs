use super::ui::{push_quad_uv, TextRun, UiVertex};
use super::ui_text::{self, TEXT_GLYPH_ADVANCE, TEXT_GLYPH_H, TEXT_GLYPH_W};
use std::collections::{HashMap, HashSet};

const WHITE: [f32; 4] = [1.0, 1.0, 1.0, 1.0];
const SHADOW: [f32; 4] = [0.0, 0.0, 0.0, 1.0];
const STATIC_ATLAS_MAX_W: u32 = 2048;
const GLYPH_ATLAS_MAX_W: u32 = 512;

#[derive(Copy, Clone, Debug, Default)]
struct AtlasRect {
    x: u32,
    y: u32,
    w: u32,
    h: u32,
}

#[derive(Clone, Debug)]
struct TextBitmap {
    w: u32,
    h: u32,
    rgba: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct RunKey {
    text: String,
    cell_px: u32,
    color: [u8; 4],
    shadow: bool,
}

pub(super) struct StaticTextAtlas {
    bind: wgpu::BindGroup,
    bitmaps: HashMap<RunKey, TextBitmap>,
    rects: HashMap<RunKey, AtlasRect>,
    last_keys: Vec<RunKey>,
    atlas_size: (u32, u32),
}

impl StaticTextAtlas {
    pub(super) fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        layout: &wgpu::BindGroupLayout,
    ) -> Self {
        Self {
            bind: create_bind(
                device,
                queue,
                layout,
                &[0, 0, 0, 0],
                1,
                1,
                "static text atlas",
            ),
            bitmaps: HashMap::new(),
            rects: HashMap::new(),
            last_keys: Vec::new(),
            atlas_size: (1, 1),
        }
    }

    pub(super) fn bind(&self) -> &wgpu::BindGroup {
        &self.bind
    }

    pub(super) fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        layout: &wgpu::BindGroupLayout,
        screen: (u32, u32),
        runs: &[TextRun],
        verts: &mut Vec<UiVertex>,
    ) {
        verts.clear();
        let keys = unique_keys(runs);
        if keys.is_empty() {
            self.last_keys.clear();
            self.rects.clear();
            return;
        }
        if keys != self.last_keys {
            self.repack(device, queue, layout, &keys);
        }
        let (aw, ah) = self.atlas_size;
        for run in runs {
            let key = run_key(run);
            let Some(rect) = self.rects.get(&key).copied() else {
                continue;
            };
            let draw_w = (ui_text::text_width(&run.text) as f32 + shadow_extra(run)) * run.cell_px;
            let draw_h = (TEXT_GLYPH_H as f32 + shadow_extra(run)) * run.cell_px;
            if draw_w <= 0.0 || draw_h <= 0.0 {
                continue;
            }
            push_quad_uv(
                verts,
                screen,
                run.x,
                run.y,
                draw_w,
                draw_h,
                [rect.x as f32 / aw as f32, rect.y as f32 / ah as f32],
                [
                    (rect.x + rect.w) as f32 / aw as f32,
                    (rect.y + rect.h) as f32 / ah as f32,
                ],
                WHITE,
            );
        }
    }

    fn repack(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        layout: &wgpu::BindGroupLayout,
        keys: &[RunKey],
    ) {
        for key in keys {
            self.bitmaps
                .entry(key.clone())
                .or_insert_with(|| raster_text_run(key));
        }

        let mut x = 1u32;
        let mut y = 1u32;
        let mut row_h = 0u32;
        let mut used_w = 1u32;
        self.rects.clear();
        for key in keys {
            let bmp = &self.bitmaps[key];
            if x > 1 && x + bmp.w + 1 > STATIC_ATLAS_MAX_W {
                x = 1;
                y += row_h + 1;
                row_h = 0;
            }
            self.rects.insert(
                key.clone(),
                AtlasRect {
                    x,
                    y,
                    w: bmp.w,
                    h: bmp.h,
                },
            );
            used_w = used_w.max(x + bmp.w + 1);
            x += bmp.w + 1;
            row_h = row_h.max(bmp.h);
        }
        let used_h = (y + row_h + 1).max(1);
        let aw = used_w.next_power_of_two().max(1);
        let ah = used_h.next_power_of_two().max(1);
        let mut rgba = vec![0u8; (aw * ah * 4) as usize];
        for key in keys {
            let bmp = &self.bitmaps[key];
            let rect = self.rects[key];
            blit(&mut rgba, aw, rect.x, rect.y, bmp);
        }
        self.bind = create_bind(device, queue, layout, &rgba, aw, ah, "static text atlas");
        self.atlas_size = (aw, ah);
        self.last_keys = keys.to_vec();
    }
}

pub(super) struct GlyphTextAtlas {
    bind: wgpu::BindGroup,
    cell_px: u32,
    atlas_size: (u32, u32),
    glyphs: HashMap<char, AtlasRect>,
}

impl GlyphTextAtlas {
    pub(super) fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        layout: &wgpu::BindGroupLayout,
    ) -> Self {
        let mut atlas = Self {
            bind: create_bind(
                device,
                queue,
                layout,
                &[0, 0, 0, 0],
                1,
                1,
                "glyph text atlas",
            ),
            cell_px: 0,
            atlas_size: (1, 1),
            glyphs: HashMap::new(),
        };
        atlas.rebuild(device, queue, layout, 1);
        atlas
    }

    pub(super) fn bind(&self) -> &wgpu::BindGroup {
        &self.bind
    }

    pub(super) fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        layout: &wgpu::BindGroupLayout,
        screen: (u32, u32),
        runs: &[TextRun],
        verts: &mut Vec<UiVertex>,
    ) {
        verts.clear();
        if runs.is_empty() {
            return;
        }
        let cell_px = runs
            .iter()
            .map(|run| cell_key(run.cell_px))
            .max()
            .unwrap_or(1);
        if self.cell_px != cell_px {
            self.rebuild(device, queue, layout, cell_px);
        }
        for run in runs {
            if run.shadow {
                self.emit_run(screen, verts, run, run.cell_px, run.cell_px, SHADOW);
            }
            self.emit_run(screen, verts, run, 0.0, 0.0, run.color);
        }
    }

    fn rebuild(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        layout: &wgpu::BindGroupLayout,
        cell_px: u32,
    ) {
        let glyph_w = TEXT_GLYPH_W * cell_px;
        let glyph_h = TEXT_GLYPH_H * cell_px;
        let mut x = 1u32;
        let mut y = 1u32;
        let mut row_h = 0u32;
        let mut used_w = 1u32;
        self.glyphs.clear();
        for ch in glyph_chars() {
            if x > 1 && x + glyph_w + 1 > GLYPH_ATLAS_MAX_W {
                x = 1;
                y += row_h + 1;
                row_h = 0;
            }
            self.glyphs.insert(
                ch,
                AtlasRect {
                    x,
                    y,
                    w: glyph_w,
                    h: glyph_h,
                },
            );
            used_w = used_w.max(x + glyph_w + 1);
            x += glyph_w + 1;
            row_h = row_h.max(glyph_h);
        }
        let used_h = (y + row_h + 1).max(1);
        let aw = used_w.next_power_of_two().max(1);
        let ah = used_h.next_power_of_two().max(1);
        let mut rgba = vec![0u8; (aw * ah * 4) as usize];
        for ch in glyph_chars() {
            let rect = self.glyphs[&ch];
            raster_glyph(&mut rgba, aw, rect.x, rect.y, ch, cell_px);
        }
        self.bind = create_bind(device, queue, layout, &rgba, aw, ah, "glyph text atlas");
        self.atlas_size = (aw, ah);
        self.cell_px = cell_px;
    }

    fn emit_run(
        &self,
        screen: (u32, u32),
        verts: &mut Vec<UiVertex>,
        run: &TextRun,
        dx: f32,
        dy: f32,
        color: [f32; 4],
    ) {
        let (aw, ah) = self.atlas_size;
        let mut x = run.x;
        let draw_w = TEXT_GLYPH_W as f32 * run.cell_px;
        let draw_h = TEXT_GLYPH_H as f32 * run.cell_px;
        let advance = TEXT_GLYPH_ADVANCE as f32 * run.cell_px;
        for ch in run.text.chars() {
            let ch = normalize_glyph_char(ch);
            if ch != ' ' {
                if let Some(rect) = self.glyphs.get(&ch).copied() {
                    push_quad_uv(
                        verts,
                        screen,
                        x + dx,
                        run.y + dy,
                        draw_w,
                        draw_h,
                        [rect.x as f32 / aw as f32, rect.y as f32 / ah as f32],
                        [
                            (rect.x + rect.w) as f32 / aw as f32,
                            (rect.y + rect.h) as f32 / ah as f32,
                        ],
                        color,
                    );
                }
            }
            x += advance;
        }
    }
}

fn unique_keys(runs: &[TextRun]) -> Vec<RunKey> {
    let mut seen = HashSet::new();
    let mut keys = Vec::new();
    for run in runs {
        if run.text.is_empty() {
            continue;
        }
        let key = run_key(run);
        if seen.insert(key.clone()) {
            keys.push(key);
        }
    }
    keys
}

fn run_key(run: &TextRun) -> RunKey {
    RunKey {
        text: run.text.clone(),
        cell_px: cell_key(run.cell_px),
        color: color_key(run.color),
        shadow: run.shadow,
    }
}

fn cell_key(cell_px: f32) -> u32 {
    cell_px.round().clamp(1.0, 64.0) as u32
}

fn color_key(color: [f32; 4]) -> [u8; 4] {
    color.map(|c| (c.clamp(0.0, 1.0) * 255.0).round() as u8)
}

fn shadow_extra(run: &TextRun) -> f32 {
    if run.shadow {
        1.0
    } else {
        0.0
    }
}

fn raster_text_run(key: &RunKey) -> TextBitmap {
    let text_w = ui_text::text_width(&key.text) * key.cell_px;
    let text_h = TEXT_GLYPH_H * key.cell_px;
    let shadow = if key.shadow { key.cell_px } else { 0 };
    let w = (text_w + shadow).max(1);
    let h = (text_h + shadow).max(1);
    let mut rgba = vec![0u8; (w * h * 4) as usize];
    if key.shadow {
        raster_text_cells(
            &mut rgba,
            w,
            &key.text,
            key.cell_px,
            key.cell_px,
            key.cell_px,
            [0, 0, 0, 255],
        );
    }
    raster_text_cells(&mut rgba, w, &key.text, 0, 0, key.cell_px, key.color);
    TextBitmap { w, h, rgba }
}

fn raster_text_cells(
    rgba: &mut [u8],
    width: u32,
    text: &str,
    off_x: u32,
    off_y: u32,
    cell_px: u32,
    color: [u8; 4],
) {
    ui_text::for_each_text_lit_cell(text, |px, py| {
        fill_rect(
            rgba,
            width,
            off_x + px * cell_px,
            off_y + py * cell_px,
            cell_px,
            cell_px,
            color,
        );
    });
}

fn raster_glyph(rgba: &mut [u8], width: u32, x: u32, y: u32, ch: char, cell_px: u32) {
    let text = ch.to_string();
    ui_text::for_each_text_lit_cell(&text, |px, py| {
        fill_rect(
            rgba,
            width,
            x + px * cell_px,
            y + py * cell_px,
            cell_px,
            cell_px,
            [255, 255, 255, 255],
        );
    });
}

fn fill_rect(rgba: &mut [u8], width: u32, x: u32, y: u32, w: u32, h: u32, color: [u8; 4]) {
    for yy in y..y + h {
        for xx in x..x + w {
            let i = ((yy * width + xx) * 4) as usize;
            rgba[i..i + 4].copy_from_slice(&color);
        }
    }
}

fn blit(dst: &mut [u8], dst_w: u32, x: u32, y: u32, src: &TextBitmap) {
    for row in 0..src.h {
        let dst_i = (((y + row) * dst_w + x) * 4) as usize;
        let src_i = (row * src.w * 4) as usize;
        let len = (src.w * 4) as usize;
        dst[dst_i..dst_i + len].copy_from_slice(&src.rgba[src_i..src_i + len]);
    }
}

fn create_bind(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    layout: &wgpu::BindGroupLayout,
    rgba: &[u8],
    w: u32,
    h: u32,
    label: &str,
) -> wgpu::BindGroup {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        rgba,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(w * 4),
            rows_per_image: Some(h),
        },
        wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
    );
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("text atlas sampler"),
        address_mode_u: wgpu::AddressMode::ClampToEdge,
        address_mode_v: wgpu::AddressMode::ClampToEdge,
        address_mode_w: wgpu::AddressMode::ClampToEdge,
        mag_filter: wgpu::FilterMode::Nearest,
        min_filter: wgpu::FilterMode::Nearest,
        mipmap_filter: wgpu::FilterMode::Nearest,
        ..Default::default()
    });
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some(label),
        layout,
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

fn glyph_chars() -> impl Iterator<Item = char> {
    (b' '..=b'~').map(char::from)
}

fn normalize_glyph_char(ch: char) -> char {
    if ch.is_ascii() && (' '..='~').contains(&ch) {
        ch
    } else {
        '?'
    }
}
