use crate::layout::RectI;

#[repr(C)]
#[derive(Copy, Clone, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct UiVertex {
    pub pos: [f32; 2],
    pub uv: [f32; 2],
    pub color: [f32; 4],
}

pub const SOLID_UV: [f32; 2] = [-1.0, -1.0];

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum TexId {
    Solid,
    ThemePage(u16),
    Font,
    DocImage(u16),
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Batch {
    pub tex: TexId,
    pub start: u32,
    pub count: u32,
    pub clip: Option<[i32; 4]>,
}

#[derive(Default, Debug)]
pub struct DrawList {
    pub vertices: Vec<UiVertex>,
    pub batches: Vec<Batch>,
    pub overlay_start: usize,
}

impl DrawList {
    pub fn clear(&mut self) {
        self.vertices.clear();
        self.batches.clear();
        self.overlay_start = 0;
    }

    pub fn is_empty(&self) -> bool {
        self.vertices.is_empty()
    }

    pub fn begin_overlay(&mut self) {
        self.overlay_start = self.batches.len();
    }

    pub fn base_batches(&self) -> &[Batch] {
        &self.batches[..self.overlay_start.min(self.batches.len())]
    }

    pub fn overlay_batches(&self) -> &[Batch] {
        &self.batches[self.overlay_start.min(self.batches.len())..]
    }

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
        let mergeable = self.batches.len() > self.overlay_start;
        match self.batches.last_mut().filter(|_| mergeable) {
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

#[derive(Copy, Clone, Debug, PartialEq)]
pub struct SpriteSrc {
    pub tex: TexId,
    pub rect: [u32; 4],
    pub tex_size: (u32, u32),
}

impl SpriteSrc {
    fn rect_f32(&self) -> [f32; 4] {
        self.rect.map(|v| v as f32)
    }
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub struct PaintStyle {
    pub color: [f32; 4],
    pub clip: Option<RectI>,
}

impl PaintStyle {
    pub fn plain(clip: Option<RectI>) -> PaintStyle {
        PaintStyle {
            color: [1.0; 4],
            clip,
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub enum Fit {
    Stretch,
    Cover,
    Tile,
    NineSlice([i32; 4]),
    Rotated { angle: f32, pivot: Option<[f32; 2]> },
}

pub struct Painter<'a> {
    pub list: &'a mut DrawList,
    pub scale: i32,
    pub font: &'a crate::text::Font,
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

    pub fn sprite(&mut self, src: &SpriteSrc, r: RectI, fit: Fit, style: PaintStyle) {
        let clip = self.phys_clip(style.clip);
        let color = style.color;
        match fit {
            Fit::Stretch => {
                let dst = self.phys(r);
                self.list
                    .push_rect(src.tex, dst, src.rect_f32(), src.tex_size, color, clip);
            }
            Fit::Cover => self.cover(src, r, color, clip),
            Fit::Tile => self.tiled(src, r, color, clip),
            Fit::NineSlice(slice) => self.nine_slice(src, r, slice, color, clip),
            Fit::Rotated { angle, pivot } => self.rotated(src, r, angle, pivot, color, clip),
        }
    }

    fn cover(&mut self, src: &SpriteSrc, r: RectI, color: [f32; 4], clip: Option<[i32; 4]>) {
        if r.w <= 0 || r.h <= 0 || src.rect[2] == 0 || src.rect[3] == 0 {
            return;
        }
        let [sx, sy, sw, sh] = src.rect_f32();
        let rect_aspect = r.w as f32 / r.h as f32;
        let image_aspect = sw / sh;
        let crop = if rect_aspect > image_aspect {
            let crop_h = (sw / rect_aspect).min(sh);
            [sx, sy + (sh - crop_h) * 0.5, sw, crop_h]
        } else {
            let crop_w = (sh * rect_aspect).min(sw);
            [sx + (sw - crop_w) * 0.5, sy, crop_w, sh]
        };
        let dst = self.phys(r);
        self.list
            .push_rect(src.tex, dst, crop, src.tex_size, color, clip);
    }

    fn tiled(&mut self, src: &SpriteSrc, r: RectI, color: [f32; 4], clip: Option<[i32; 4]>) {
        let (tile_w, tile_h) = (src.rect[2].max(1) as i32, src.rect[3].max(1) as i32);
        let mut y = 0;
        while y < r.h {
            let th = tile_h.min(r.h - y);
            let mut x = 0;
            while x < r.w {
                let tw = tile_w.min(r.w - x);
                let dst = self.phys(RectI {
                    x: r.x + x,
                    y: r.y + y,
                    w: tw,
                    h: th,
                });
                let part = [src.rect[0] as f32, src.rect[1] as f32, tw as f32, th as f32];
                self.list
                    .push_rect(src.tex, dst, part, src.tex_size, color, clip);
                x += tile_w;
            }
            y += tile_h;
        }
    }

    fn nine_slice(
        &mut self,
        src: &SpriteSrc,
        r: RectI,
        slice: [i32; 4],
        color: [f32; 4],
        clip: Option<[i32; 4]>,
    ) {
        let [sl, st, sr, sb] = slice.map(|v| v.max(0) as f32);
        let [sx, sy, sw, sh] = src.rect_f32();
        let [dx, dy, dw, dh] = self.phys(r);
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
                    src.tex,
                    [x0, y0, x1 - x0, y1 - y0],
                    [u0, v0, u1 - u0, v1 - v0],
                    src.tex_size,
                    color,
                    clip,
                );
            }
        }
    }

    fn rotated(
        &mut self,
        src: &SpriteSrc,
        r: RectI,
        angle: f32,
        pivot: Option<[f32; 2]>,
        color: [f32; 4],
        clip: Option<[i32; 4]>,
    ) {
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
        let (tw, th) = (src.tex_size.0 as f32, src.tex_size.1 as f32);
        let [sx, sy, sw, sh] = src.rect_f32();
        let (u0, v0) = (sx / tw, sy / th);
        let (u1, v1) = ((sx + sw) / tw, (sy + sh) / th);
        self.list.push_quad(
            src.tex,
            corners,
            [[u0, v0], [u1, v0], [u1, v1], [u0, v1]],
            color,
            clip,
        );
    }

    pub fn text(&mut self, s: &str, x: i32, y: i32, color: [f32; 4], clip: Option<RectI>) {
        self.text_scaled(s, x, y, 1, color, clip);
    }

    pub fn text_ellipsized(&mut self, s: &str, rect: RectI, color: [f32; 4], clip: Option<RectI>) {
        let k = self.scale;
        self.ellipsized_at(s, rect, k, color, clip);
    }

    pub fn text_ellipsized_small(
        &mut self,
        s: &str,
        rect: RectI,
        color: [f32; 4],
        clip: Option<RectI>,
    ) {
        let k = self.small_text_step();
        self.ellipsized_at(s, rect, k, color, clip);
    }

    pub fn ellipsized_at(
        &mut self,
        s: &str,
        rect: RectI,
        k: i32,
        color: [f32; 4],
        clip: Option<RectI>,
    ) {
        let clip = clip.map_or(rect, |inherited| inherited.intersect(rect));
        if clip.w == 0 || clip.h == 0 || rect.w <= 0 {
            return;
        }
        let font = self.font;
        let room_font_px = rect.w * self.scale / k.max(1);
        if font.width(s) <= room_font_px {
            self.text_at(s, rect.x, rect.y, k, color, Some(clip));
            return;
        }
        let dots = "...";
        let room = room_font_px - font.width(dots);
        if room <= 0 {
            let dots = &dots[..font.fit_chars(dots, room_font_px)];
            self.text_at(dots, rect.x, rect.y, k, color, Some(clip));
            return;
        }
        let kept = font.fit_chars(s, room);
        if kept == 0 {
            return;
        }
        let mut shown: String = s.chars().take(kept).collect();
        shown.push_str(dots);
        self.text_at(&shown, rect.x, rect.y, k, color, Some(clip));
    }

    pub fn text_input_view(
        &mut self,
        view: &crate::text_edit::TextInputRender,
        x: i32,
        y: i32,
        text_color: [f32; 4],
        selection_color: [f32; 4],
        clip: Option<RectI>,
    ) {
        let font = self.font;
        let run_x = |chars: usize| -> i32 {
            font.width(&view.text.chars().take(chars).collect::<String>())
        };
        let (body_top, body_h) = font.body_span();
        if let Some((s0, s1)) = view.selection {
            let (x0, x1) = (run_x(s0), run_x(s1));
            self.solid(
                RectI {
                    x: x + x0,
                    y: y + body_top - 1,
                    w: (x1 - x0 - 1).max(0),
                    h: body_h + 2,
                },
                selection_color,
                clip,
            );
        }
        self.text(&view.text, x, y, text_color, clip);
        if view.show_cursor {
            self.solid(
                RectI {
                    x: x + run_x(view.cursor) - 1,
                    y: y + body_top,
                    w: 1,
                    h: body_h,
                },
                text_color,
                clip,
            );
        }
    }

    pub fn text_scaled(
        &mut self,
        s: &str,
        x: i32,
        y: i32,
        text_scale: u32,
        color: [f32; 4],
        clip: Option<RectI>,
    ) {
        let k = text_scale.max(1) as i32 * self.scale;
        self.text_at(s, x, y, k, color, clip);
    }

    pub fn small_text_step(&self) -> i32 {
        (self.scale - 1).max(1)
    }

    pub fn text_small(&mut self, s: &str, x: i32, y: i32, color: [f32; 4], clip: Option<RectI>) {
        let k = self.small_text_step();
        self.text_at(s, x, y, k, color, clip);
    }

    fn text_at(&mut self, s: &str, x: i32, y: i32, k: i32, color: [f32; 4], clip: Option<RectI>) {
        let clip = self.phys_clip(clip);
        let font = self.font;
        let (tw, th) = font.atlas_size();
        let (mut cx, py) = (x * self.scale, y * self.scale);
        for ch in s.chars() {
            let glyph = font.glyph(ch);
            let [dx, dy, w, h] = glyph.bounds();
            if w > 0 && h > 0 {
                self.list.push_rect(
                    TexId::Font,
                    [
                        (cx + dx * k) as f32,
                        (py + dy * k) as f32,
                        (w * k) as f32,
                        (h * k) as f32,
                    ],
                    glyph.atlas_rect().map(|v| v as f32),
                    (tw, th),
                    color,
                    clip,
                );
            }
            cx += glyph.advance() * k;
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn text_wrapped_lines(
        &mut self,
        s: &str,
        r: RectI,
        small: bool,
        max_lines: Option<u32>,
        color: [f32; 4],
        clip: Option<RectI>,
    ) {
        let k = if small {
            self.small_text_step()
        } else {
            self.scale
        };
        let room_font_px = r.w * self.scale / k.max(1);
        let advance = self.font.line_advance() * k / self.scale.max(1);
        let lines = self.font.wrap(s, room_font_px);
        let cap = max_lines.map_or(lines.len(), |m| (m.max(1) as usize).min(lines.len()));
        let line_h = (self.font.line_h() * k + self.scale - 1) / self.scale.max(1);
        let mut y = r.y;
        for (n, line) in lines.iter().take(cap).enumerate() {
            if n + 1 == cap && cap < lines.len() {
                let rest = &s[line.start..];
                let row = RectI {
                    x: r.x,
                    y,
                    w: r.w,
                    h: line_h,
                };
                self.ellipsized_at(rest, row, k, color, clip);
            } else {
                self.text_at(&s[line.clone()], r.x, y, k, color, clip);
            }
            y += advance;
        }
    }

    pub fn text_wrapped(&mut self, s: &str, r: RectI, color: [f32; 4], clip: Option<RectI>) {
        let mut y = r.y;
        let advance = self.font.line_advance();
        for line in self.font.wrap(s, r.w) {
            self.text(&s[line], r.x, y, color, clip);
            y += advance;
        }
    }

    pub fn text_wrapped_small(&mut self, s: &str, r: RectI, color: [f32; 4], clip: Option<RectI>) {
        let k = self.small_text_step();
        let room_font_px = r.w * self.scale / k.max(1);
        let advance = self.font.line_advance() * k / self.scale.max(1);
        let mut y = r.y;
        for line in self.font.wrap(s, room_font_px) {
            self.text_at(&s[line], r.x, y, k, color, clip);
            y += advance;
        }
    }
}

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
    fn single_line_text_ellipsizes_and_clips_to_its_layout_box() {
        let mut dl = DrawList::default();
        let font = crate::text::Font::builtin();
        let mut p = Painter {
            list: &mut dl,
            scale: 2,
            font: &font,
        };
        let rect = RectI {
            x: 4,
            y: 5,
            w: p.font.width("Abc..."),
            h: p.font.line_h(),
        };
        p.text_ellipsized("A long recipe name", rect, [1.0; 4], None);

        let glyphs = dl.vertices.len() / 6;
        assert!(glyphs > 3, "some of the text survives: {glyphs}");
        assert!(
            glyphs <= "Abc...".chars().count(),
            "truncation never overflows the box: {glyphs}"
        );
        assert_eq!(
            dl.batches[0].clip,
            Some([8, 10, rect.w * 2, rect.h * 2]),
            "the solved label box becomes the physical scissor"
        );
    }

    #[test]
    fn batches_merge_on_same_tex_and_clip() {
        let mut dl = DrawList::default();
        let font = crate::text::Font::builtin();
        let mut p = Painter {
            list: &mut dl,
            scale: 2,
            font: &font,
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
        let font = crate::text::Font::builtin();
        let mut p = Painter {
            list: &mut dl,
            scale: 3,
            font: &font,
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
        assert_eq!(dl.vertices[2].pos, [45.0, 27.0]);
    }

    #[test]
    fn cover_sprite_crops_source_without_squishing() {
        let mut dl = DrawList::default();
        let font = crate::text::Font::builtin();
        let mut p = Painter {
            list: &mut dl,
            scale: 1,
            font: &font,
        };
        let src = SpriteSrc {
            tex: TexId::DocImage(0),
            rect: [0, 0, 200, 100],
            tex_size: (200, 100),
        };
        p.sprite(
            &src,
            RectI {
                x: 0,
                y: 0,
                w: 100,
                h: 100,
            },
            Fit::Cover,
            PaintStyle::plain(None),
        );
        assert_eq!(dl.vertices[0].pos, [0.0, 0.0]);
        assert_eq!(dl.vertices[2].pos, [100.0, 100.0]);
        assert_eq!(dl.vertices[0].uv, [0.25, 0.0]);
        assert_eq!(dl.vertices[2].uv, [0.75, 1.0]);
    }

    const PAGE0: SpriteSrc = SpriteSrc {
        tex: TexId::ThemePage(0),
        rect: [0, 0, 16, 16],
        tex_size: (64, 64),
    };

    #[test]
    fn nine_slice_emits_nine_cells_with_fixed_corners() {
        let mut dl = DrawList::default();
        let font = crate::text::Font::builtin();
        let mut p = Painter {
            list: &mut dl,
            scale: 2,
            font: &font,
        };
        p.sprite(
            &PAGE0,
            RectI {
                x: 0,
                y: 0,
                w: 32,
                h: 20,
            },
            Fit::NineSlice([4, 4, 4, 4]),
            PaintStyle::plain(None),
        );
        assert_eq!(dl.vertices.len(), 9 * 6);
        assert_eq!(dl.vertices[0].pos, [0.0, 0.0]);
        assert_eq!(dl.vertices[2].pos, [8.0, 8.0]);
        assert_eq!(dl.vertices[0].uv, [0.0, 0.0]);
        assert_eq!(dl.vertices[2].uv, [4.0 / 64.0, 4.0 / 64.0]);
    }

    #[test]
    fn degenerate_nine_slice_collapses_middle() {
        let mut dl = DrawList::default();
        let font = crate::text::Font::builtin();
        let mut p = Painter {
            list: &mut dl,
            scale: 1,
            font: &font,
        };
        p.sprite(
            &PAGE0,
            RectI {
                x: 0,
                y: 0,
                w: 8,
                h: 30,
            },
            Fit::NineSlice([4, 4, 4, 4]),
            PaintStyle::plain(None),
        );
        assert_eq!(dl.vertices.len(), 6 * 6, "3 rows × 2 cols survive");
    }

    #[test]
    fn rotated_sprite_spins_around_the_pivot() {
        let mut dl = DrawList::default();
        let font = crate::text::Font::builtin();
        let mut p = Painter {
            list: &mut dl,
            scale: 1,
            font: &font,
        };
        let src = SpriteSrc {
            tex: TexId::DocImage(0),
            rect: [0, 0, 10, 10],
            tex_size: (10, 10),
        };
        p.sprite(
            &src,
            RectI {
                x: 0,
                y: 0,
                w: 10,
                h: 10,
            },
            Fit::Rotated {
                angle: std::f32::consts::FRAC_PI_2,
                pivot: None,
            },
            PaintStyle::plain(None),
        );
        let tl = dl.vertices[0].pos;
        assert!(
            (tl[0] - 10.0).abs() < 1e-4 && tl[1].abs() < 1e-4,
            "tl → tr, got {tl:?}"
        );
    }
}
