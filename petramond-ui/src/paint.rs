//! Paint primitives: the renderer-agnostic [`DrawList`] and the emission
//! helpers that fill it.
//!
//! Vertices are in **physical pixels, top-left origin, y down** — the game's
//! wgpu adapter converts to NDC on upload; the software rasterizer consumes
//! them directly. All logical→physical scaling happens here (one multiply, in
//! [`Painter`]), so layout stays integer-logical and draw/hit can't diverge.
//!
//! Paint semantics are deliberately tiny so two backends stay bit-identical:
//! axis-aligned textured quads (plus rotated quads for `rotimage`), nearest
//! sampling, straight-alpha over-blending, per-batch scissor clips.

use crate::layout::RectI;

/// A single UI vertex. `uv = (-1, -1)` is the solid-color sentinel (no
/// texture sample) — same convention as the game's `ui.wgsl`.
#[repr(C)]
#[derive(Copy, Clone, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct UiVertex {
    pub pos: [f32; 2],
    pub uv: [f32; 2],
    pub color: [f32; 4],
}

/// uv sentinel marking a solid-color quad.
pub const SOLID_UV: [f32; 2] = [-1.0, -1.0];

/// Which texture a batch samples. The host maps each to a real binding.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum TexId {
    /// No texture: solid vertex color.
    Solid,
    /// The theme kit atlas.
    ThemeAtlas,
    /// The font atlas ([`crate::text::build_atlas`]).
    Font,
    /// A document-local image, by the host's per-document registry index.
    DocImage(u16),
}

/// One contiguous vertex range drawn with one texture and one optional
/// scissor rect (physical px).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Batch {
    pub tex: TexId,
    pub start: u32,
    pub count: u32,
    pub clip: Option<[i32; 4]>,
}

/// The CPU-built frame: every quad of one GUI in paint order. Buffers are
/// reused across frames (cleared, capacity kept).
#[derive(Default, Debug)]
pub struct DrawList {
    pub vertices: Vec<UiVertex>,
    pub batches: Vec<Batch>,
}

impl DrawList {
    pub fn clear(&mut self) {
        self.vertices.clear();
        self.batches.clear();
    }

    pub fn is_empty(&self) -> bool {
        self.vertices.is_empty()
    }

    /// Push one quad given its four physical-px corners (tl, tr, br, bl) and
    /// matching UVs, merging into the previous batch when texture and clip
    /// agree.
    #[allow(clippy::too_many_arguments)]
    pub fn push_quad(
        &mut self,
        tex: TexId,
        corners: [[f32; 2]; 4],
        uvs: [[f32; 2]; 4],
        color: [f32; 4],
        clip: Option<[i32; 4]>,
    ) {
        let [tl, tr, br, bl] = corners;
        let [uv_tl, uv_tr, uv_br, uv_bl] = uvs;
        let v = |pos: [f32; 2], uv: [f32; 2]| UiVertex { pos, uv, color };
        let start = self.vertices.len() as u32;
        self.vertices.extend_from_slice(&[
            v(tl, uv_tl),
            v(bl, uv_bl),
            v(br, uv_br),
            v(tl, uv_tl),
            v(br, uv_br),
            v(tr, uv_tr),
        ]);
        match self.batches.last_mut() {
            Some(b) if b.tex == tex && b.clip == clip && b.start + b.count == start => {
                b.count += 6;
            }
            _ => self.batches.push(Batch {
                tex,
                start,
                count: 6,
                clip,
            }),
        }
    }

    /// Axis-aligned quad from a physical-px rect and a pixel rect within a
    /// texture of `tex_size`.
    #[allow(clippy::too_many_arguments)]
    pub fn push_rect(
        &mut self,
        tex: TexId,
        dst: [f32; 4],
        src_px: [f32; 4],
        tex_size: (u32, u32),
        color: [f32; 4],
        clip: Option<[i32; 4]>,
    ) {
        let [x, y, w, h] = dst;
        let (tw, th) = (tex_size.0 as f32, tex_size.1 as f32);
        let [sx, sy, sw, sh] = src_px;
        let (u0, v0) = (sx / tw, sy / th);
        let (u1, v1) = ((sx + sw) / tw, (sy + sh) / th);
        self.push_quad(
            tex,
            [[x, y], [x + w, y], [x + w, y + h], [x, y + h]],
            [[u0, v0], [u1, v0], [u1, v1], [u0, v1]],
            color,
            clip,
        );
    }
}

/// Scaled emission over a [`DrawList`]: all inputs are *logical* px; the one
/// logical→physical multiply lives here.
pub struct Painter<'a> {
    pub list: &'a mut DrawList,
    pub scale: i32,
}

impl Painter<'_> {
    fn s(&self) -> f32 {
        self.scale as f32
    }

    fn phys(&self, r: RectI) -> [f32; 4] {
        [
            (r.x * self.scale) as f32,
            (r.y * self.scale) as f32,
            (r.w * self.scale) as f32,
            (r.h * self.scale) as f32,
        ]
    }

    fn phys_clip(&self, clip: Option<RectI>) -> Option<[i32; 4]> {
        clip.map(|c| {
            [
                c.x * self.scale,
                c.y * self.scale,
                c.w * self.scale,
                c.h * self.scale,
            ]
        })
    }

    pub fn solid(&mut self, r: RectI, color: [f32; 4], clip: Option<RectI>) {
        let [x, y, w, h] = self.phys(r);
        self.list.push_quad(
            TexId::Solid,
            [[x, y], [x + w, y], [x + w, y + h], [x, y + h]],
            [SOLID_UV; 4],
            color,
            self.phys_clip(clip),
        );
    }

    /// A texture sub-rect stretched over `r`.
    #[allow(clippy::too_many_arguments)]
    pub fn sprite(
        &mut self,
        tex: TexId,
        r: RectI,
        src: [u32; 4],
        tex_size: (u32, u32),
        color: [f32; 4],
        clip: Option<RectI>,
    ) {
        let clip = self.phys_clip(clip);
        self.list.push_rect(
            tex,
            self.phys(r),
            src.map(|v| v as f32),
            tex_size,
            color,
            clip,
        );
    }

    /// The texture repeated over `r` at its natural 1x-art size (logical px),
    /// with partial tiles at the right/bottom edges.
    #[allow(clippy::too_many_arguments)]
    pub fn tiled_sprite(
        &mut self,
        tex: TexId,
        r: RectI,
        src: [u32; 4],
        tex_size: (u32, u32),
        color: [f32; 4],
        clip: Option<RectI>,
    ) {
        let clip_px = self.phys_clip(clip);
        let (tile_w, tile_h) = (src[2].max(1) as i32, src[3].max(1) as i32);
        let mut y = 0;
        while y < r.h {
            let th = tile_h.min(r.h - y);
            let mut x = 0;
            while x < r.w {
                let tw = tile_w.min(r.w - x);
                let dst = RectI {
                    x: r.x + x,
                    y: r.y + y,
                    w: tw,
                    h: th,
                };
                self.list.push_rect(
                    tex,
                    self.phys(dst),
                    [src[0] as f32, src[1] as f32, tw as f32, th as f32],
                    tex_size,
                    color,
                    clip_px,
                );
                x += tile_w;
            }
            y += tile_h;
        }
    }

    /// A 9-sliced texture part over `r`: corners stay 1:1 (slice insets are
    /// 1x-art px = logical px), edges and centre stretch.
    #[allow(clippy::too_many_arguments)]
    pub fn nine_slice(
        &mut self,
        tex: TexId,
        r: RectI,
        src: [u32; 4],
        slice: [i32; 4],
        tex_size: (u32, u32),
        color: [f32; 4],
        clip: Option<RectI>,
    ) {
        let clip = self.phys_clip(clip);
        let [sl, st, sr, sb] = slice.map(|v| v.max(0) as f32);
        let [sx, sy, sw, sh] = src.map(|v| v as f32);
        let [dx, dy, dw, dh] = self.phys(r);
        // Destination insets scale with the gui scale so corner pixels stay
        // on the pixel grid; clamp so tiny rects degrade to plain stretch.
        let s = self.s();
        let (dl, dr2) = clamp_pair(sl * s, sr * s, dw);
        let (dt, db) = clamp_pair(st * s, sb * s, dh);
        let xs_dst = [dx, dx + dl, dx + dw - dr2, dx + dw];
        let ys_dst = [dy, dy + dt, dy + dh - db, dy + dh];
        let xs_src = [sx, sx + sl, sx + sw - sr, sx + sw];
        let ys_src = [sy, sy + st, sy + sh - sb, sy + sh];
        for row in 0..3 {
            for col in 0..3 {
                let (x0, x1) = (xs_dst[col], xs_dst[col + 1]);
                let (y0, y1) = (ys_dst[row], ys_dst[row + 1]);
                if x1 - x0 <= 0.0 || y1 - y0 <= 0.0 {
                    continue;
                }
                let (u0, u1) = (xs_src[col], xs_src[col + 1]);
                let (v0, v1) = (ys_src[row], ys_src[row + 1]);
                self.list.push_rect(
                    tex,
                    [x0, y0, x1 - x0, y1 - y0],
                    [u0, v0, u1 - u0, v1 - v0],
                    tex_size,
                    color,
                    clip,
                );
            }
        }
    }

    /// Single-line text at logical `(x, y)` (top-left of the run).
    pub fn text(&mut self, s: &str, x: i32, y: i32, color: [f32; 4], clip: Option<RectI>) {
        self.text_scaled(s, x, y, 1, color, clip);
    }

    /// Single-line text with a glyph-size multiplier (headings).
    pub fn text_scaled(
        &mut self,
        s: &str,
        x: i32,
        y: i32,
        text_scale: u32,
        color: [f32; 4],
        clip: Option<RectI>,
    ) {
        let clip = self.phys_clip(clip);
        let k = text_scale.max(1) as i32;
        let (tw, th) = crate::text::atlas_size();
        let mut cx = x;
        for ch in s.chars() {
            let src = crate::text::atlas_rect(ch);
            let dst = RectI {
                x: cx,
                y,
                w: crate::text::GLYPH_W * k,
                h: crate::text::GLYPH_H * k,
            };
            self.list.push_rect(
                TexId::Font,
                self.phys(dst),
                src.map(|v| v as f32),
                (tw, th),
                color,
                clip,
            );
            cx += crate::text::ADVANCE * k;
        }
    }

    /// Word-wrapped text inside a logical rect (top-left aligned lines).
    pub fn text_wrapped(&mut self, s: &str, r: RectI, color: [f32; 4], clip: Option<RectI>) {
        let mut y = r.y;
        for line in crate::text::wrap(s, r.w) {
            self.text(&s[line], r.x, y, color, clip);
            y += crate::text::LINE_ADVANCE;
        }
    }

    /// A texture sub-rect over `r`, rotated by `angle` radians around `pivot`
    /// (logical px from `r`'s top-left; `None` = centre).
    #[allow(clippy::too_many_arguments)]
    pub fn rotated_sprite(
        &mut self,
        tex: TexId,
        r: RectI,
        src: [u32; 4],
        tex_size: (u32, u32),
        angle: f32,
        pivot: Option<[f32; 2]>,
        color: [f32; 4],
        clip: Option<RectI>,
    ) {
        let clip = self.phys_clip(clip);
        let [dx, dy, dw, dh] = self.phys(r);
        let s = self.s();
        let (px, py) = match pivot {
            Some([px, py]) => (dx + px * s, dy + py * s),
            None => (dx + dw * 0.5, dy + dh * 0.5),
        };
        let (sin, cos) = angle.sin_cos();
        let rot = |x: f32, y: f32| -> [f32; 2] {
            let (rx, ry) = (x - px, y - py);
            [px + rx * cos - ry * sin, py + rx * sin + ry * cos]
        };
        let corners = [
            rot(dx, dy),
            rot(dx + dw, dy),
            rot(dx + dw, dy + dh),
            rot(dx, dy + dh),
        ];
        let (tw, th) = (tex_size.0 as f32, tex_size.1 as f32);
        let [sx, sy, sw, sh] = src.map(|v| v as f32);
        let (u0, v0) = (sx / tw, sy / th);
        let (u1, v1) = ((sx + sw) / tw, (sy + sh) / th);
        self.list.push_quad(
            tex,
            corners,
            [[u0, v0], [u1, v0], [u1, v1], [u0, v1]],
            color,
            clip,
        );
    }
}

/// Clamp a leading/trailing inset pair so it never exceeds the available
/// span (shrinks both proportionally when it would).
fn clamp_pair(lead: f32, trail: f32, span: f32) -> (f32, f32) {
    let sum = lead + trail;
    if sum <= span || sum <= 0.0 {
        (lead, trail)
    } else {
        let k = span / sum;
        (lead * k, trail * k)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn batches_merge_on_same_tex_and_clip() {
        let mut dl = DrawList::default();
        let mut p = Painter {
            list: &mut dl,
            scale: 2,
        };
        p.solid(
            RectI {
                x: 0,
                y: 0,
                w: 4,
                h: 4,
            },
            [1.0; 4],
            None,
        );
        p.solid(
            RectI {
                x: 8,
                y: 0,
                w: 4,
                h: 4,
            },
            [1.0; 4],
            None,
        );
        p.text("A", 0, 0, [1.0; 4], None);
        p.solid(
            RectI {
                x: 0,
                y: 0,
                w: 2,
                h: 2,
            },
            [1.0; 4],
            Some(RectI {
                x: 0,
                y: 0,
                w: 10,
                h: 10,
            }),
        );
        assert_eq!(
            dl.batches.len(),
            3,
            "solid+solid merge; font and clipped-solid split"
        );
        assert_eq!(dl.batches[0].count, 12);
        assert_eq!(dl.batches[1].tex, TexId::Font);
        assert_eq!(
            dl.batches[2].clip,
            Some([0, 0, 20, 20]),
            "clip is physical px"
        );
    }

    #[test]
    fn painter_scales_logical_to_physical() {
        let mut dl = DrawList::default();
        let mut p = Painter {
            list: &mut dl,
            scale: 3,
        };
        p.solid(
            RectI {
                x: 5,
                y: 7,
                w: 10,
                h: 2,
            },
            [1.0; 4],
            None,
        );
        assert_eq!(dl.vertices[0].pos, [15.0, 21.0]);
        assert_eq!(dl.vertices[2].pos, [45.0, 27.0]); // br corner
    }

    #[test]
    fn nine_slice_emits_nine_cells_with_fixed_corners() {
        let mut dl = DrawList::default();
        let mut p = Painter {
            list: &mut dl,
            scale: 2,
        };
        p.nine_slice(
            TexId::ThemeAtlas,
            RectI {
                x: 0,
                y: 0,
                w: 32,
                h: 20,
            },
            [0, 0, 16, 16],
            [4, 4, 4, 4],
            (64, 64),
            [1.0; 4],
            None,
        );
        assert_eq!(dl.vertices.len(), 9 * 6);
        // Top-left corner cell: 4 logical px → 8 physical px square.
        assert_eq!(dl.vertices[0].pos, [0.0, 0.0]);
        assert_eq!(dl.vertices[2].pos, [8.0, 8.0]);
        // Corner uv spans exactly the 4px src inset.
        assert_eq!(dl.vertices[0].uv, [0.0, 0.0]);
        assert_eq!(dl.vertices[2].uv, [4.0 / 64.0, 4.0 / 64.0]);
    }

    #[test]
    fn degenerate_nine_slice_collapses_middle() {
        let mut dl = DrawList::default();
        let mut p = Painter {
            list: &mut dl,
            scale: 1,
        };
        // Dst exactly two insets wide: no middle column.
        p.nine_slice(
            TexId::ThemeAtlas,
            RectI {
                x: 0,
                y: 0,
                w: 8,
                h: 30,
            },
            [0, 0, 16, 16],
            [4, 4, 4, 4],
            (64, 64),
            [1.0; 4],
            None,
        );
        assert_eq!(dl.vertices.len(), 6 * 6, "3 rows × 2 cols survive");
    }

    #[test]
    fn rotated_sprite_spins_around_the_pivot() {
        let mut dl = DrawList::default();
        let mut p = Painter {
            list: &mut dl,
            scale: 1,
        };
        // 90° around the rect centre maps tl -> tr.
        p.rotated_sprite(
            TexId::DocImage(0),
            RectI {
                x: 0,
                y: 0,
                w: 10,
                h: 10,
            },
            [0, 0, 10, 10],
            (10, 10),
            std::f32::consts::FRAC_PI_2,
            None,
            [1.0; 4],
            None,
        );
        let tl = dl.vertices[0].pos;
        assert!(
            (tl[0] - 10.0).abs() < 1e-4 && tl[1].abs() < 1e-4,
            "tl → tr, got {tl:?}"
        );
    }
}
