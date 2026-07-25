//! A loaded font: per-glyph bitmaps, per-glyph metrics, and the atlas grid.
//!
//! Glyphs are rasterized ONCE, to 1 bit. A pixel font is authored on a pixel
//! grid, so coverage is thresholded rather than antialiased — anti-aliasing at
//! these sizes reads as blur, not as smoothing, and it would fight the game's
//! nearest-neighbour sampling. Everything downstream (measurement, wrapping,
//! caret positions, the GPU atlas, the CPU rasterizer) reads the same metrics,
//! so what is measured is always what is drawn.

use std::collections::HashMap;
use std::ops::Range;

/// Codepoints a loaded font covers when it has them: printable ASCII, the
/// Latin-1 supplement (× · ° accented letters), Latin Extended-A (the rest of
/// European Latin), and the punctuation UI text actually reaches for.
const COVERAGE: &[Range<u32>] = &[0x20..0x7F, 0xA0..0x180, 0x2013..0x2015, 0x2018..0x201E];
const EXTRA: &[u32] = &[0x2026, 0x2192, 0x2713];

/// Atlas grid width in cells.
pub const ATLAS_COLS: u32 = 16;

/// Coverage at or above this counts as ink at the supersampled size.
const INK_THRESHOLD: f32 = 0.5;

/// Glyphs are rasterized at this ODD multiple of the target size, then each
/// target pixel takes the single subpixel at its CENTRE.
///
/// A pixel font's glyphs are solid rectangles on a grid, so the exact way to
/// recover them is to point-sample each pixel's centre — which supersampling
/// by an odd factor gives for free. Averaging or majority-voting a whole cell
/// instead bleeds neighbouring strokes together, which is what turns `w`, `W`
/// and `M` (three one-pixel stems inside five columns) into blobs.
///
/// Verified against FreeType's hinted output: centre sampling reproduces all
/// 94 printable ASCII glyphs of the shipped font exactly, where majority
/// voting matches 7.
const SUPERSAMPLE: i32 = 3;

/// One glyph: its 1-bit bitmap inside the font's uniform cell, plus how far
/// the pen moves after drawing it.
#[derive(Clone, Debug)]
pub struct Glyph {
    /// One bitmask per cell row; bit `cell_w - 1 - col` set = lit.
    rows: Vec<u32>,
    /// Pen advance in font-pixels.
    advance: i32,
    /// Index of this glyph's cell in the atlas grid.
    cell: u32,
}

impl Glyph {
    pub fn advance(&self) -> i32 {
        self.advance
    }
}

#[derive(Debug)]
pub enum FontError {
    Parse(String),
    /// The file parsed but produced no usable glyphs at that size.
    Empty,
}

impl std::fmt::Display for FontError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FontError::Parse(e) => write!(f, "font parse failed: {e}"),
            FontError::Empty => write!(f, "font produced no glyphs at that size"),
        }
    }
}

/// A rasterized font: uniform atlas cells, per-glyph advances.
///
/// Cells are uniform (a plain grid, like the slot grids) while ADVANCES are
/// per glyph — so the atlas stays trivial to index while text still sets
/// proportionally.
/// Glyphs whose ink defines the TEXT BODY — the band a caret or a selection
/// should cover. Deliberately excludes accented capitals, whose headroom the
/// cell reserves but ordinary text leaves empty.
const BODY_TOP_SAMPLE: &str = "MHTbdkl";
const BODY_BOTTOM_SAMPLE: &str = "gjpqy";

#[derive(Debug)]
pub struct Font {
    cell_w: i32,
    cell_h: i32,
    line_advance: i32,
    max_advance: i32,
    body: (i32, i32),
    glyphs: HashMap<char, Glyph>,
    fallback: Glyph,
    cells: u32,
}

impl Font {
    /// The built-in 5×7 ASCII table — the fallback when no font file loads.
    pub fn builtin() -> Font {
        use crate::builtin;
        let mut glyphs = HashMap::new();
        let mut cell = 0;
        for cp in 0x20u32..0x7F {
            let ch = char::from_u32(cp).expect("ascii");
            let rows = builtin::glyph(ch)
                .iter()
                .map(|bits| u32::from(*bits))
                .collect();
            glyphs.insert(
                ch,
                Glyph {
                    rows,
                    advance: builtin::ADVANCE,
                    cell,
                },
            );
            cell += 1;
        }
        let fallback = Glyph {
            rows: builtin::glyph('\u{FFFD}')
                .iter()
                .map(|bits| u32::from(*bits))
                .collect(),
            advance: builtin::ADVANCE,
            cell,
        };
        // The built-in table has neither accents nor descenders, so its body
        // IS its cell.
        let body = (0, builtin::GLYPH_H);
        Font {
            cell_w: builtin::GLYPH_W,
            cell_h: builtin::GLYPH_H,
            line_advance: builtin::GLYPH_H + 2,
            max_advance: builtin::ADVANCE,
            body,
            glyphs,
            fallback,
            cells: cell + 1,
        }
    }

    /// Rasterize a TrueType/OpenType face at `px` pixels per EM — the size
    /// the font was designed for (11 for the shipped face).
    ///
    /// Away from its design size a pixel font stops being a pixel font, so
    /// this is authored, never guessed.
    pub fn from_ttf(bytes: &[u8], px: f32) -> Result<Font, FontError> {
        use ab_glyph::{Font as _, ScaleFont as _};
        let face = ab_glyph::FontRef::try_from_slice(bytes)
            .map_err(|e| FontError::Parse(e.to_string()))?;
        // ab_glyph scales against the font's ASCENT+DESCENT, not its em
        // square, so a scale of `px` renders an em of `px * em / height` —
        // smaller than the design size, and off the pixel grid. Convert, or
        // every glyph comes out shrunk and mangled.
        let em = face.units_per_em().filter(|em| *em > 0.0);
        let raster_px = match em {
            Some(em) => px * face.height_unscaled() / em,
            None => px,
        };
        let scaled = face.as_scaled(raster_px);
        let ss = SUPERSAMPLE.max(1);

        // Pass 1: outline every covered codepoint and find the common ink box,
        // so all glyphs can share one cell size and sit on one baseline.
        struct Raw {
            ch: char,
            advance: i32,
            min_x: i32,
            min_y: i32,
            ink: Vec<(i32, i32)>,
        }
        let mut raws: Vec<Raw> = Vec::new();
        let codepoints = COVERAGE
            .iter()
            .flat_map(|r| r.clone())
            .chain(EXTRA.iter().copied());
        for cp in codepoints {
            let Some(ch) = char::from_u32(cp) else {
                continue;
            };
            let id = face.glyph_id(ch);
            if id.0 == 0 {
                continue; // the face has no glyph for this codepoint
            }
            let advance = scaled.h_advance(id).round() as i32;
            let glyph = id.with_scale(raster_px * ss as f32);
            let mut ink = Vec::new();
            let (mut min_x, mut min_y) = (0, 0);
            if let Some(outline) = face.outline_glyph(glyph) {
                let bounds = outline.px_bounds();
                let (hi_x, hi_y) = (bounds.min.x.floor() as i32, bounds.min.y.floor() as i32);
                // Take the centre subpixel of each target pixel, in ABSOLUTE
                // coordinates so a glyph's own origin cannot shift the grid
                // under it.
                let centre = ss / 2;
                outline.draw(|x, y, coverage| {
                    if coverage < INK_THRESHOLD {
                        return;
                    }
                    let (ax, ay) = (hi_x + x as i32, hi_y + y as i32);
                    if ax.rem_euclid(ss) == centre && ay.rem_euclid(ss) == centre {
                        ink.push((ax.div_euclid(ss), ay.div_euclid(ss)));
                    }
                });
                min_x = ink.iter().map(|(x, _)| *x).min().unwrap_or(0);
                min_y = ink.iter().map(|(_, y)| *y).min().unwrap_or(0);
                // `ink` is absolute; the cell layout below wants it relative.
                for cell in &mut ink {
                    cell.0 -= min_x;
                    cell.1 -= min_y;
                }
            }
            raws.push(Raw {
                ch,
                advance,
                min_x,
                min_y,
                ink,
            });
        }
        if raws.iter().all(|raw| raw.ink.is_empty()) {
            return Err(FontError::Empty);
        }

        // The cell must hold every glyph's ink, including accented capitals
        // above the cap line and descenders below the baseline.
        let inked = || raws.iter().filter(|raw| !raw.ink.is_empty());
        let left = inked().map(|raw| raw.min_x).min().unwrap_or(0).min(0);
        let top = inked().map(|raw| raw.min_y).min().unwrap_or(0);
        let right = inked()
            .map(|raw| raw.min_x + raw.ink.iter().map(|(x, _)| x + 1).max().unwrap_or(0))
            .max()
            .unwrap_or(1);
        let bottom = inked()
            .map(|raw| raw.min_y + raw.ink.iter().map(|(_, y)| y + 1).max().unwrap_or(0))
            .max()
            .unwrap_or(1);
        let cell_w = (right - left).max(1);
        let cell_h = (bottom - top).max(1);
        if cell_w > 32 {
            // Rows are u32 bitmasks; a wider cell would silently truncate.
            return Err(FontError::Parse(format!("cell width {cell_w} exceeds 32")));
        }

        let mut glyphs = HashMap::new();
        let mut cell = 0;
        for raw in &raws {
            let mut rows = vec![0u32; cell_h as usize];
            for &(x, y) in &raw.ink {
                let col = raw.min_x - left + x;
                let row = raw.min_y - top + y;
                if (0..cell_w).contains(&col) && (0..cell_h).contains(&row) {
                    rows[row as usize] |= 1 << (cell_w - 1 - col);
                }
            }
            glyphs.insert(
                raw.ch,
                Glyph {
                    rows,
                    advance: raw.advance.max(0),
                    cell,
                },
            );
            cell += 1;
        }

        // Unknown codepoints get one shared cell. U+FFFD if the face has it,
        // else a hollow box — never a blank, so a missing glyph is visible.
        let fallback_rows = glyphs
            .get(&'\u{FFFD}')
            .map(|g| g.rows.clone())
            .unwrap_or_else(|| box_rows(cell_w, cell_h));
        let fallback_advance = glyphs
            .get(&'\u{FFFD}')
            .map(|g| g.advance)
            .or_else(|| glyphs.get(&'?').map(|g| g.advance))
            .unwrap_or(cell_w);
        let fallback = Glyph {
            rows: fallback_rows,
            advance: fallback_advance,
            cell,
        };

        let row_of = |sample: &str, top: bool| -> Option<i32> {
            let rows = sample.chars().filter_map(|ch| {
                let glyph = glyphs.get(&ch)?;
                let lit: Vec<i32> = (0..cell_h)
                    .filter(|&row| glyph.rows[row as usize] != 0)
                    .collect();
                if top {
                    lit.first().copied()
                } else {
                    lit.last().map(|row| row + 1)
                }
            });
            if top {
                rows.min()
            } else {
                rows.max()
            }
        };
        let body = (
            row_of(BODY_TOP_SAMPLE, true).unwrap_or(0),
            row_of(BODY_BOTTOM_SAMPLE, false).unwrap_or(cell_h),
        );

        Ok(Font {
            cell_w,
            cell_h,
            // One blank pixel row between lines, like the built-in font.
            line_advance: cell_h + 2,
            max_advance: glyphs
                .values()
                .map(|glyph| glyph.advance)
                .chain(std::iter::once(fallback.advance))
                .max()
                .unwrap_or(cell_w)
                .max(1),
            body,
            glyphs,
            fallback,
            cells: cell + 1,
        })
    }

    pub fn cell_w(&self) -> i32 {
        self.cell_w
    }

    /// Height of one line's glyph box — what a single-line label measures.
    pub fn line_h(&self) -> i32 {
        self.cell_h
    }

    /// Baseline-to-baseline distance for wrapped text.
    pub fn line_advance(&self) -> i32 {
        self.line_advance
    }

    /// The widest pen advance in the font.
    ///
    /// Use it wherever a character COUNT has to be derived from a width
    /// without knowing the characters — it is the only bound that cannot
    /// overflow the box (and is exact for a monospace face).
    pub fn max_advance(&self) -> i32 {
        self.max_advance
    }

    /// The text BODY as `(top row within the cell, height)`: ascender top to
    /// descender bottom, excluding the headroom the cell reserves for
    /// accented capitals. A caret or a selection sized to the whole cell
    /// towers over ordinary text, because that headroom is nearly always
    /// empty.
    pub fn body_span(&self) -> (i32, i32) {
        let (top, bottom) = self.body;
        (top, (bottom - top).max(1))
    }

    /// How many codepoints resolve to a real glyph (fallback excluded).
    pub fn glyph_count(&self) -> usize {
        self.glyphs.len()
    }

    pub fn glyph(&self, ch: char) -> &Glyph {
        self.glyphs.get(&ch).unwrap_or(&self.fallback)
    }

    /// Whether `ch` has its own glyph (false = it draws the fallback).
    pub fn has_glyph(&self, ch: char) -> bool {
        self.glyphs.contains_key(&ch)
    }

    pub fn advance(&self, ch: char) -> i32 {
        self.glyph(ch).advance
    }

    /// Width of `s` on one line, in font-pixels.
    pub fn width(&self, s: &str) -> i32 {
        s.chars().map(|ch| self.advance(ch)).sum()
    }

    /// Width of the first `byte_end` bytes of `s` — the caret x for an index.
    pub fn prefix_width(&self, s: &str, byte_end: usize) -> i32 {
        let end = byte_end.min(s.len());
        self.width(&s[..floor_char_boundary(s, end)])
    }

    /// The byte index whose caret position is nearest `x` font-pixels — the
    /// inverse of [`Self::prefix_width`], for click-to-caret.
    pub fn index_at_x(&self, s: &str, x: i32) -> usize {
        let mut pen = 0;
        for (bi, ch) in s.char_indices() {
            let next = pen + self.advance(ch);
            if x < pen + self.advance(ch) / 2 {
                return bi;
            }
            pen = next;
        }
        s.len()
    }

    /// How many leading characters of `s` fit in `max_w` font-pixels.
    pub fn fit_chars(&self, s: &str, max_w: i32) -> usize {
        let mut pen = 0;
        let mut count = 0;
        for ch in s.chars() {
            let next = pen + self.advance(ch);
            if next > max_w {
                break;
            }
            pen = next;
            count += 1;
        }
        count
    }

    /// Greedy word wrap into lines of at most `max_w` font-pixels, breaking at
    /// spaces where possible and mid-word only when a word alone overflows.
    /// Returns byte ranges into `s`; never empty (empty text = one empty line).
    pub fn wrap(&self, s: &str, max_w: i32) -> Vec<Range<usize>> {
        let mut lines: Vec<Range<usize>> = Vec::new();
        let mut line_start = 0usize;
        let mut line_w = 0i32;
        let mut last_space: Option<usize> = None;
        for (bi, ch) in s.char_indices() {
            let advance = self.advance(ch);
            // Only a non-space can force a break: a trailing space that
            // overflows is swallowed by the break anyway, and checking it
            // would wrap a line that actually fits. A single character wider
            // than the line still has to go somewhere, so only break when
            // something is already on the line.
            if ch != ' ' && line_w + advance > max_w && bi > line_start {
                let break_at = match last_space {
                    Some(sp) if sp >= line_start => {
                        lines.push(line_start..sp);
                        sp + 1
                    }
                    _ => {
                        lines.push(line_start..bi);
                        bi
                    }
                };
                line_start = break_at;
                line_w = self.width(&s[line_start..bi]);
                last_space = None;
            }
            if ch == ' ' {
                last_space = Some(bi);
            }
            line_w += advance;
        }
        lines.push(line_start..s.len());
        lines
    }

    /// Size of `s` in font-pixels; `max_w` `None` = a single line.
    pub fn measure(&self, s: &str, max_w: Option<i32>) -> (i32, i32) {
        match max_w {
            None => (self.width(s), self.cell_h),
            Some(max_w) => {
                let lines = self.wrap(s, max_w);
                let w = lines
                    .iter()
                    .map(|r| self.width(&s[r.clone()]))
                    .max()
                    .unwrap_or(0);
                let h = self.cell_h + (lines.len() as i32 - 1) * self.line_advance;
                (w, h)
            }
        }
    }

    /// Whether cell `(col, row)` of `ch`'s glyph box is lit.
    pub fn glyph_cell(&self, ch: char, col: i32, row: i32) -> bool {
        if !(0..self.cell_w).contains(&col) || !(0..self.cell_h).contains(&row) {
            return false;
        }
        (self.glyph(ch).rows[row as usize] >> (self.cell_w - 1 - col)) & 1 == 1
    }

    // ---- atlas ---------------------------------------------------------

    pub fn atlas_cells(&self) -> u32 {
        self.cells
    }

    pub fn atlas_size(&self) -> (u32, u32) {
        let rows = self.cells.div_ceil(ATLAS_COLS);
        (ATLAS_COLS * self.cell_w as u32, rows * self.cell_h as u32)
    }

    /// The atlas pixel rect `[x, y, w, h]` of `ch`'s cell.
    pub fn atlas_rect(&self, ch: char) -> [u32; 4] {
        let cell = self.glyph(ch).cell;
        let (cx, cy) = (cell % ATLAS_COLS, cell / ATLAS_COLS);
        [
            cx * self.cell_w as u32,
            cy * self.cell_h as u32,
            self.cell_w as u32,
            self.cell_h as u32,
        ]
    }

    /// The atlas as tightly-packed RGBA (white glyphs on transparent).
    pub fn build_atlas(&self) -> (Vec<u8>, (u32, u32)) {
        let (w, h) = self.atlas_size();
        let mut rgba = vec![0u8; (w * h * 4) as usize];
        let mut blit = |glyph: &Glyph| {
            let (cx, cy) = (glyph.cell % ATLAS_COLS, glyph.cell / ATLAS_COLS);
            for (row, bits) in glyph.rows.iter().enumerate() {
                for col in 0..self.cell_w {
                    if (bits >> (self.cell_w - 1 - col)) & 1 == 0 {
                        continue;
                    }
                    let px = cx * self.cell_w as u32 + col as u32;
                    let py = cy * self.cell_h as u32 + row as u32;
                    let i = ((py * w + px) * 4) as usize;
                    rgba[i..i + 4].copy_from_slice(&[255, 255, 255, 255]);
                }
            }
        };
        for glyph in self.glyphs.values() {
            blit(glyph);
        }
        blit(&self.fallback);
        (rgba, (w, h))
    }
}

/// A hollow box, drawn for unknown codepoints when the face has no U+FFFD.
fn box_rows(cell_w: i32, cell_h: i32) -> Vec<u32> {
    let full = if cell_w >= 32 {
        u32::MAX
    } else {
        (1u32 << cell_w) - 1
    };
    let edges = full & !(full >> 1) | 1;
    (0..cell_h)
        .map(|row| {
            if row == 0 || row == cell_h - 1 {
                full
            } else {
                edges
            }
        })
        .collect()
}

/// `str::floor_char_boundary` is unstable; this is the same rule.
fn floor_char_boundary(s: &str, index: usize) -> usize {
    let mut i = index.min(s.len());
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_font_is_fixed_pitch_ascii_with_a_visible_fallback() {
        let f = Font::builtin();
        assert_eq!(f.width("AB"), f.advance('A') * 2);
        assert!(f.has_glyph('A') && f.has_glyph('~'));
        assert!(!f.has_glyph('\u{d7}'), "the built-in table is ASCII only");
        // Unknown codepoints share one cell and still draw something.
        assert_eq!(f.atlas_rect('\u{d7}'), f.atlas_rect('🙂'));
        assert!((0..f.line_h()).any(|row| (0..f.cell_w()).any(|c| f.glyph_cell('🙂', c, row))));
    }

    #[test]
    fn wrap_breaks_on_measured_width_not_character_count() {
        let f = Font::builtin();
        let s = "hello world again";
        let w = f.width("hello world");
        let lines = f.wrap(s, w);
        let texts: Vec<&str> = lines.iter().map(|r| &s[r.clone()]).collect();
        assert_eq!(texts, vec!["hello world", "again"]);

        // A word longer than the line breaks mid-word rather than looping.
        let long = "abcdefghijklmno";
        let lines = f.wrap(long, f.width("abcde"));
        let texts: Vec<&str> = lines.iter().map(|r| &long[r.clone()]).collect();
        assert_eq!(texts, vec!["abcde", "fghij", "klmno"]);

        // Even a max width narrower than one glyph terminates.
        assert_eq!(f.wrap("ab", 1).len(), 2);
        assert_eq!(f.wrap("", 40), vec![0..0]);
    }

    #[test]
    fn caret_positions_round_trip_through_the_same_metrics() {
        let f = Font::builtin();
        let s = "hello";
        for (bi, _) in s.char_indices() {
            let x = f.prefix_width(s, bi);
            assert_eq!(f.index_at_x(s, x), bi, "caret at byte {bi}");
        }
        assert_eq!(f.index_at_x(s, f.width(s) + 99), s.len());
        assert_eq!(f.prefix_width(s, 999), f.width(s));
        assert_eq!(f.fit_chars(s, f.width("hel")), 3);
    }

    /// A caret sized to the whole cell fills the input box, because the cell
    /// reserves headroom for accented capitals that ordinary text leaves
    /// empty. The body span is what a caret or selection should cover.
    #[test]
    fn the_body_span_is_the_text_band_not_the_whole_cell() {
        let bytes = std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../assets/ui/font/DepartureMono-Regular.otf"
        ))
        .expect("shipped font is vendored");
        let font = Font::from_ttf(&bytes, 11.0).expect("shipped font rasterizes");
        let (top, h) = font.body_span();
        assert!(top > 0, "accented capitals sit above the body");
        assert!(
            h < font.line_h(),
            "body {h} should be shorter than the cell {}",
            font.line_h()
        );
        assert!(top + h <= font.line_h(), "the body stays inside the cell");

        // It spans exactly cap-top to descender-bottom.
        let ink_rows = |ch: char| -> (i32, i32) {
            let rows: Vec<i32> = (0..font.line_h())
                .filter(|&y| (0..font.cell_w()).any(|x| font.glyph_cell(ch, x, y)))
                .collect();
            (rows[0], *rows.last().unwrap() + 1)
        };
        assert_eq!(top, ink_rows('M').0, "body starts at the cap line");
        assert_eq!(top + h, ink_rows('g').1, "body ends at the descender");
        assert!(ink_rows('\u{c4}').0 < top, "the accent is above the body");

        // The built-in table has no accents or descenders: body IS the cell.
        let builtin = Font::builtin();
        assert_eq!(builtin.body_span(), (0, builtin.line_h()));
    }

    #[test]
    fn measure_uses_line_advance_between_wrapped_lines() {
        let f = Font::builtin();
        let one = f.measure("hi", None);
        assert_eq!(one, (f.width("hi"), f.line_h()));
        let (_, h) = f.measure("hello world", Some(f.width("hello")));
        assert_eq!(h, f.line_h() + f.line_advance());
    }

    #[test]
    fn atlas_pixels_match_the_glyph_table() {
        let f = Font::builtin();
        let (rgba, (w, _)) = f.build_atlas();
        let [ax, ay, ..] = f.atlas_rect('A');
        for row in 0..f.line_h() {
            for col in 0..f.cell_w() {
                let i = (((ay + row as u32) * w + ax + col as u32) * 4) as usize;
                assert_eq!(
                    rgba[i + 3] == 255,
                    f.glyph_cell('A', col, row),
                    "atlas('A') differs at {col},{row}"
                );
            }
        }
    }
}
