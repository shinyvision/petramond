//! Software rasterizer for [`DrawList`] — the builder's preview backend and
//! the headless test backend.
//!
//! Semantics mirror the GPU path exactly: nearest-neighbour sampling,
//! straight-alpha over-blending, per-batch scissor. Quads are rasterized as
//! two triangles with a top-left fill rule, so axis-aligned integer-snapped
//! quads (all UI chrome) cover exactly the same pixels the GPU covers; only
//! `rotimage` edges may differ by a pixel of coverage.

use crate::paint::{Batch, DrawList, TexId, UiVertex};
use crate::theme::ImageData;

/// The textures a draw list references, resolved by the host.
pub struct TextureSet<'a> {
    pub theme_atlas: &'a ImageData,
    pub font: &'a ImageData,
    pub doc_images: &'a [&'a ImageData],
}

impl TextureSet<'_> {
    fn get(&self, tex: TexId) -> Option<&ImageData> {
        match tex {
            TexId::Solid => None,
            TexId::ThemeAtlas => Some(self.theme_atlas),
            TexId::Font => Some(self.font),
            TexId::DocImage(i) => self.doc_images.get(i as usize).copied(),
        }
    }
}

/// Rasterize `draw` into an RGBA buffer of `size` physical px. The buffer is
/// cleared to `clear` first.
pub fn rasterize(
    draw: &DrawList,
    tex: &TextureSet<'_>,
    size: (u32, u32),
    clear: [u8; 4],
    out: &mut Vec<u8>,
) {
    let (w, h) = size;
    out.clear();
    out.resize((w * h * 4) as usize, 0);
    for px in out.chunks_exact_mut(4) {
        px.copy_from_slice(&clear);
    }
    for batch in &draw.batches {
        let image = tex.get(batch.tex);
        let verts = &draw.vertices[batch.start as usize..(batch.start + batch.count) as usize];
        for tri in verts.chunks_exact(3) {
            fill_triangle(tri, image, batch, size, out);
        }
    }
}

fn fill_triangle(
    tri: &[UiVertex],
    image: Option<&ImageData>,
    batch: &Batch,
    size: (u32, u32),
    out: &mut [u8],
) {
    let (w, h) = (size.0 as i32, size.1 as i32);
    let [a, b, c] = [tri[0], tri[1], tri[2]];

    // Scissor ∩ framebuffer ∩ triangle bounds.
    let (mut x0, mut y0, mut x1, mut y1) = (0, 0, w, h);
    if let Some([cx, cy, cw, ch]) = batch.clip {
        x0 = x0.max(cx);
        y0 = y0.max(cy);
        x1 = x1.min(cx + cw);
        y1 = y1.min(cy + ch);
    }
    let min_x = a.pos[0].min(b.pos[0]).min(c.pos[0]).floor() as i32;
    let min_y = a.pos[1].min(b.pos[1]).min(c.pos[1]).floor() as i32;
    let max_x = a.pos[0].max(b.pos[0]).max(c.pos[0]).ceil() as i32;
    let max_y = a.pos[1].max(b.pos[1]).max(c.pos[1]).ceil() as i32;
    x0 = x0.max(min_x);
    y0 = y0.max(min_y);
    x1 = x1.min(max_x);
    y1 = y1.min(max_y);
    if x0 >= x1 || y0 >= y1 {
        return;
    }

    let edge = |p: [f32; 2], q: [f32; 2], x: f32, y: f32| -> f32 {
        (q[0] - p[0]) * (y - p[1]) - (q[1] - p[1]) * (x - p[0])
    };
    let raw_area = edge(a.pos, b.pos, c.pos[0], c.pos[1]);
    if raw_area == 0.0 {
        return;
    }
    // Normalize winding so edge values are positive inside, then apply the
    // top-left fill rule: a pixel center exactly ON an edge belongs to the
    // triangle only when the (winding-normalized) edge points up, or exactly
    // right — so the diagonal shared by a quad's two triangles is covered
    // exactly once and translucent quads never double-blend.
    let sign = if raw_area > 0.0 { 1.0 } else { -1.0 };
    let area = raw_area * sign;
    let includes_zero = |p: [f32; 2], q: [f32; 2]| -> bool {
        let (dx, dy) = ((q[0] - p[0]) * sign, (q[1] - p[1]) * sign);
        dy < 0.0 || (dy == 0.0 && dx > 0.0)
    };
    let inc = [
        includes_zero(b.pos, c.pos),
        includes_zero(c.pos, a.pos),
        includes_zero(a.pos, b.pos),
    ];

    for py in y0..y1 {
        for px in x0..x1 {
            let (fx, fy) = (px as f32 + 0.5, py as f32 + 0.5);
            let wa = edge(b.pos, c.pos, fx, fy) * sign;
            let wb = edge(c.pos, a.pos, fx, fy) * sign;
            let wc = edge(a.pos, b.pos, fx, fy) * sign;
            let on = |w: f32, include: bool| w > 0.0 || (w == 0.0 && include);
            if !(on(wa, inc[0]) && on(wb, inc[1]) && on(wc, inc[2])) {
                continue;
            }
            let (ka, kb, kc) = (wa / area, wb / area, wc / area);
            let lerp2 = |f: fn(&UiVertex) -> [f32; 2]| {
                [
                    f(&a)[0] * ka + f(&b)[0] * kb + f(&c)[0] * kc,
                    f(&a)[1] * ka + f(&b)[1] * kb + f(&c)[1] * kc,
                ]
            };
            let uv = lerp2(|v| v.uv);
            let color: [f32; 4] =
                std::array::from_fn(|i| a.color[i] * ka + b.color[i] * kb + c.color[i] * kc);
            let src = if uv[0] < 0.0 {
                color
            } else {
                match image {
                    Some(img) => {
                        let t = sample_nearest(img, uv);
                        [
                            t[0] * color[0],
                            t[1] * color[1],
                            t[2] * color[2],
                            t[3] * color[3],
                        ]
                    }
                    None => color,
                }
            };
            if src[3] <= 0.0 {
                continue;
            }
            let i = ((py * w + px) * 4) as usize;
            blend_over(&mut out[i..i + 4], src);
        }
    }
}

fn sample_nearest(img: &ImageData, uv: [f32; 2]) -> [f32; 4] {
    let (w, h) = (img.size.0 as f32, img.size.1 as f32);
    let x = (uv[0] * w).floor().clamp(0.0, w - 1.0) as u32;
    let y = (uv[1] * h).floor().clamp(0.0, h - 1.0) as u32;
    let i = ((y * img.size.0 + x) * 4) as usize;
    [
        img.rgba[i] as f32 / 255.0,
        img.rgba[i + 1] as f32 / 255.0,
        img.rgba[i + 2] as f32 / 255.0,
        img.rgba[i + 3] as f32 / 255.0,
    ]
}

/// Straight-alpha "over" blend of `src` onto the u8 destination pixel.
fn blend_over(dst: &mut [u8], src: [f32; 4]) {
    let sa = src[3].clamp(0.0, 1.0);
    for i in 0..3 {
        let d = dst[i] as f32 / 255.0;
        let v = src[i] * sa + d * (1.0 - sa);
        dst[i] = (v * 255.0 + 0.5) as u8;
    }
    let da = dst[3] as f32 / 255.0;
    dst[3] = ((sa + da * (1.0 - sa)) * 255.0 + 0.5) as u8;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::RectI;
    use crate::paint::Painter;
    use crate::theme::Theme;

    fn px(buf: &[u8], size: (u32, u32), x: u32, y: u32) -> [u8; 4] {
        let i = ((y * size.0 + x) * 4) as usize;
        [buf[i], buf[i + 1], buf[i + 2], buf[i + 3]]
    }

    #[test]
    fn solid_quads_cover_exact_pixels() {
        let theme = Theme::placeholder();
        let mut dl = DrawList::default();
        Painter {
            list: &mut dl,
            scale: 2,
        }
        .solid(
            RectI {
                x: 1,
                y: 1,
                w: 3,
                h: 2,
            },
            [1.0, 0.0, 0.0, 1.0],
            None,
        );
        let tex = TextureSet {
            theme_atlas: &theme.atlas,
            font: &theme.font,
            doc_images: &[],
        };
        let size = (16, 16);
        let mut out = Vec::new();
        rasterize(&dl, &tex, size, [0, 0, 0, 255], &mut out);
        // Physical rect = (2,2)..(8,6): inside red, outside black, no gaps on
        // the quad diagonal.
        assert_eq!(px(&out, size, 2, 2), [255, 0, 0, 255]);
        assert_eq!(px(&out, size, 7, 5), [255, 0, 0, 255]);
        assert_eq!(px(&out, size, 4, 4), [255, 0, 0, 255]);
        assert_eq!(px(&out, size, 1, 2), [0, 0, 0, 255]);
        assert_eq!(px(&out, size, 8, 2), [0, 0, 0, 255]);
        assert_eq!(px(&out, size, 2, 6), [0, 0, 0, 255]);
        // Exact coverage count: 6×4 red pixels.
        let red = out
            .chunks_exact(4)
            .filter(|p| p[0] == 255 && p[1] == 0)
            .count();
        assert_eq!(red, 24);
    }

    #[test]
    fn scissor_clips_pixels() {
        let theme = Theme::placeholder();
        let mut dl = DrawList::default();
        Painter {
            list: &mut dl,
            scale: 1,
        }
        .solid(
            RectI {
                x: 0,
                y: 0,
                w: 10,
                h: 10,
            },
            [0.0, 1.0, 0.0, 1.0],
            Some(RectI {
                x: 2,
                y: 2,
                w: 4,
                h: 4,
            }),
        );
        let tex = TextureSet {
            theme_atlas: &theme.atlas,
            font: &theme.font,
            doc_images: &[],
        };
        let size = (10, 10);
        let mut out = Vec::new();
        rasterize(&dl, &tex, size, [0, 0, 0, 255], &mut out);
        assert_eq!(px(&out, size, 3, 3), [0, 255, 0, 255]);
        assert_eq!(
            px(&out, size, 1, 1),
            [0, 0, 0, 255],
            "clipped outside scissor"
        );
        assert_eq!(px(&out, size, 6, 6), [0, 0, 0, 255]);
    }

    #[test]
    fn alpha_blends_over_background() {
        let theme = Theme::placeholder();
        let mut dl = DrawList::default();
        Painter {
            list: &mut dl,
            scale: 1,
        }
        .solid(
            RectI {
                x: 0,
                y: 0,
                w: 2,
                h: 2,
            },
            [1.0, 1.0, 1.0, 0.5],
            None,
        );
        let tex = TextureSet {
            theme_atlas: &theme.atlas,
            font: &theme.font,
            doc_images: &[],
        };
        let mut out = Vec::new();
        rasterize(&dl, &tex, (2, 2), [0, 0, 0, 255], &mut out);
        let p = px(&out, (2, 2), 0, 0);
        assert!(
            (126..=130).contains(&p[0]),
            "50% white over black ≈ 128, got {}",
            p[0]
        );
    }

    #[test]
    fn text_renders_glyph_pixels() {
        let theme = Theme::placeholder();
        let mut dl = DrawList::default();
        Painter {
            list: &mut dl,
            scale: 1,
        }
        .text("A", 0, 0, [1.0, 1.0, 1.0, 1.0], None);
        let tex = TextureSet {
            theme_atlas: &theme.atlas,
            font: &theme.font,
            doc_images: &[],
        };
        let size = (8, 8);
        let mut out = Vec::new();
        rasterize(&dl, &tex, size, [0, 0, 0, 255], &mut out);
        for row in 0..crate::text::GLYPH_H {
            for col in 0..crate::text::GLYPH_W {
                let want = crate::text::glyph_cell('A', col, row);
                let got = px(&out, size, col as u32, row as u32)[0] == 255;
                assert_eq!(got, want, "glyph 'A' pixel ({col},{row})");
            }
        }
    }
}
