//! A loaded font: per-glyph bitmaps and metrics, a fallback chain of faces,
//! and a shelf-packed atlas.
//!
//! Glyphs are rasterized ONCE, to 1 bit. A pixel font is authored on a pixel
//! grid, so coverage is thresholded rather than antialiased — anti-aliasing at
//! these sizes reads as blur, not as smoothing, and it would fight the game's
//! nearest-neighbour sampling. Everything downstream (measurement, wrapping,
//! caret positions, the GPU atlas, the CPU rasterizer) reads the same metrics,
//! so what is measured is always what is drawn.
//!
//! Coverage is whatever the faces provide: each face in the chain contributes
//! every codepoint it maps (or the ranges its manifest names) that an earlier
//! face lacks, and the built-in table closes the chain. The LINE box comes
//! from the primary face's own ascent/descent — each glyph keeps a tight
//! bitmap placed against the shared baseline, so one tall glyph (an accented
//! capital, a fallback script) never enlarges every other glyph.

mod atlas;
mod glyphs;
mod raster;

use glyphs::{fallback_glyph, GlyphSet, Source};
use raster::{FaceMetrics, RawGlyph};
use std::collections::HashMap;
use std::ops::Range;
use std::sync::Mutex;

/// Glyphs whose ink defines the TEXT BODY — the band a caret or a selection
/// should cover. Deliberately excludes accented capitals, whose headroom the
/// line box reserves but ordinary text leaves empty.
const BODY_TOP_SAMPLE: &str = "MHTbdkl";
const BODY_BOTTOM_SAMPLE: &str = "gjpqy";

/// One face of a fallback chain: a TrueType/OpenType file at the pixel size
/// it was designed for.
#[derive(Clone, Copy, Debug)]
pub struct FaceSource<'a> {
    pub bytes: &'a [u8],
    /// Pixels per EM. Away from its design size a pixel font stops being a
    /// pixel font, so this is authored, never guessed.
    pub px: f32,
    /// Inclusive codepoint ranges to take from this face; empty takes every
    /// codepoint the face maps.
    pub ranges: &'a [[u32; 2]],
}

/// One glyph: a tight 1-bit bitmap, where it sits against the pen and the
/// line top, how far the pen moves after it, and its atlas rect.
#[derive(Clone, Debug)]
pub struct Glyph {
    /// Row-major, `w * h`, `true` = ink.
    bitmap: Vec<bool>,
    /// Bitmap rect relative to (pen x, line top): `[dx, dy, w, h]`.
    bounds: [i32; 4],
    advance: i32,
    atlas: [u32; 4],
}

impl Glyph {
    /// Pen advance in font-pixels.
    pub fn advance(&self) -> i32 {
        self.advance
    }

    /// The bitmap's rect relative to the pen x and the line top, as
    /// `[dx, dy, w, h]` (zero-sized for blank glyphs such as a space; `dy`
    /// may be negative for ink above the line box).
    pub fn bounds(&self) -> [i32; 4] {
        self.bounds
    }

    /// The glyph's atlas pixel rect `[x, y, w, h]`.
    pub fn atlas_rect(&self) -> [u32; 4] {
        self.atlas
    }

    /// Whether bitmap pixel `(x, y)` (bitmap-local) is ink.
    pub fn lit(&self, x: i32, y: i32) -> bool {
        let [_, _, w, h] = self.bounds;
        (0..w).contains(&x) && (0..h).contains(&y) && self.bitmap[(y * w + x) as usize]
    }

    /// Every ink pixel relative to (pen x, line top).
    pub fn ink(&self) -> impl Iterator<Item = (i32, i32)> + '_ {
        let [dx, dy, w, _] = self.bounds;
        self.bitmap
            .iter()
            .enumerate()
            .filter(|(_, lit)| **lit)
            .map(move |(i, _)| (dx + i as i32 % w, dy + i as i32 / w))
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

/// A rasterized font: per-glyph bitmaps and advances over one line box.
#[derive(Debug)]
pub struct Font {
    line_h: i32,
    line_advance: i32,
    max_advance: i32,
    body: (i32, i32),
    glyphs: GlyphSet,
    /// Memoised wraps: layout measures a wrapped label and paint wraps it
    /// again, every frame — both now read one shaping.
    wraps: Mutex<WrapCache>,
}

/// Wrapped text by (max width, string): its line ranges and wrapped size.
#[derive(Debug, Default)]
struct WrapCache {
    by_width: HashMap<i32, HashMap<String, Wrapped>>,
    entries: usize,
}

#[derive(Debug)]
struct Wrapped {
    lines: Vec<Range<usize>>,
    size: (i32, i32),
}

/// Distinct wraps held before the cache starts over — far more than any
/// screen shows, small enough that churning text cannot grow it unbounded.
const WRAP_CACHE_CAP: usize = 4096;

impl Font {
    /// The built-in 5×7 ASCII table — the fallback when no font file loads.
    pub fn builtin() -> Font {
        let metrics = raster::builtin_metrics();
        let covered = raster::builtin_chars(true)
            .into_iter()
            .map(|ch| (ch, 0))
            .collect();
        Font::assemble(vec![Source::Builtin], covered, metrics, false)
            .expect("the built-in table always assembles")
    }

    /// Load one TrueType/OpenType face at `px` pixels per EM, closed by the
    /// built-in table.
    pub fn from_ttf(bytes: &[u8], px: f32) -> Result<Font, FontError> {
        Font::from_faces(&[FaceSource {
            bytes,
            px,
            ranges: &[],
        }])
    }

    /// A fallback chain: the first face sets the line box, each later face
    /// fills missing codepoints, and the built-in table closes the chain.
    pub fn from_faces(faces: &[FaceSource<'_>]) -> Result<Font, FontError> {
        let mut sources = Vec::new();
        let mut covered = Vec::new();
        let mut seen = std::collections::HashSet::new();
        let mut primary = None;
        for source in faces {
            let face = raster::Face::open(source)?;
            let chars = face.coverage(source.ranges, &|ch| !seen.contains(&ch));
            if primary.is_none() {
                if !chars.iter().any(|&ch| !face.raster(ch).ink.is_empty()) {
                    return Err(FontError::Empty);
                }
                primary = Some(face.metrics);
            }
            let index = u16::try_from(sources.len())
                .map_err(|_| FontError::Parse("too many fallback faces".into()))?;
            for ch in chars {
                seen.insert(ch);
                covered.push((ch, index));
            }
            sources.push(Source::Face(face));
        }
        let primary = primary.ok_or(FontError::Empty)?;
        let index = u16::try_from(sources.len())
            .map_err(|_| FontError::Parse("too many fallback faces".into()))?;
        covered.extend(
            raster::builtin_chars(false)
                .into_iter()
                .filter(|ch| !seen.contains(ch))
                .map(|ch| (ch, index)),
        );
        sources.push(Source::Builtin);
        Font::assemble(sources, covered, primary, true)
    }

    fn assemble(
        sources: Vec<Source>,
        covered: Vec<(char, u16)>,
        primary: FaceMetrics,
        boxed_fallback: bool,
    ) -> Result<Font, FontError> {
        let line_h = primary.line_h;
        let glyphs = GlyphSet::new(sources, covered, primary.ascent)?;
        let body = {
            let top = BODY_TOP_SAMPLE
                .chars()
                .filter_map(|ch| glyphs.own_glyph(ch))
                .filter(|g| g.bounds[3] > 0)
                .map(|g| g.bounds[1])
                .min()
                .unwrap_or(0);
            let bottom = BODY_BOTTOM_SAMPLE
                .chars()
                .filter_map(|ch| glyphs.own_glyph(ch))
                .filter(|g| g.bounds[3] > 0)
                .map(|g| g.bounds[1] + g.bounds[3])
                .max()
                .unwrap_or(line_h);
            (top, bottom.max(top + 1))
        };
        glyphs.seal_fallback(fallback_glyph(&glyphs, boxed_fallback, body, line_h));
        let max_advance = glyphs.max_advance();
        Ok(Font {
            line_h,
            line_advance: line_h + 2,
            max_advance,
            body,
            glyphs,
            wraps: Mutex::default(),
        })
    }

    /// Height of one line box — what a single-line label measures.
    pub fn line_h(&self) -> i32 {
        self.line_h
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

    /// The text BODY as `(top row within the line box, height)`: ascender top
    /// to descender bottom, excluding the headroom reserved for accented
    /// capitals. A caret or a selection sized to the whole line towers over
    /// ordinary text, because that headroom is nearly always empty.
    pub fn body_span(&self) -> (i32, i32) {
        let (top, bottom) = self.body;
        (top, (bottom - top).max(1))
    }

    /// How many codepoints resolve to a real glyph (fallback excluded).
    pub fn glyph_count(&self) -> usize {
        self.glyphs.count()
    }

    pub fn glyph(&self, ch: char) -> &Glyph {
        self.glyphs.glyph(ch)
    }

    /// Whether `ch` has its own glyph (false = it draws the fallback).
    pub fn has_glyph(&self, ch: char) -> bool {
        self.glyphs.has(ch)
    }

    /// The pen advance of `ch`, without rasterizing its bitmap.
    pub fn advance(&self, ch: char) -> i32 {
        self.glyphs.advance(ch)
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
            let advance = self.advance(ch);
            if x < pen + advance / 2 {
                return bi;
            }
            pen += advance;
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
    /// spaces where possible and mid-word only when a word alone overflows;
    /// a `\n` always ends its line. Returns byte ranges into `s`, newlines
    /// left out; never empty (empty text = one empty line).
    pub fn wrap(&self, s: &str, max_w: i32) -> Vec<Range<usize>> {
        self.with_wrapped(s, max_w, |w| w.lines.clone())
    }

    /// Run `f` over the memoised wrap of `s` at `max_w`, shaping it first
    /// on a miss.
    fn with_wrapped<R>(&self, s: &str, max_w: i32, f: impl FnOnce(&Wrapped) -> R) -> R {
        if let Ok(cache) = self.wraps.lock() {
            if let Some(hit) = cache.by_width.get(&max_w).and_then(|m| m.get(s)) {
                return f(hit);
            }
        }
        let lines = self.wrap_uncached(s, max_w);
        let w = lines
            .iter()
            .map(|r| self.width(&s[r.clone()]))
            .max()
            .unwrap_or(0);
        let h = self.line_h + (lines.len() as i32 - 1) * self.line_advance;
        let wrapped = Wrapped {
            lines,
            size: (w, h),
        };
        let out = f(&wrapped);
        if let Ok(mut cache) = self.wraps.lock() {
            if cache.entries >= WRAP_CACHE_CAP {
                cache.by_width.clear();
                cache.entries = 0;
            }
            cache
                .by_width
                .entry(max_w)
                .or_default()
                .insert(s.to_owned(), wrapped);
            cache.entries += 1;
        }
        out
    }

    fn wrap_uncached(&self, s: &str, max_w: i32) -> Vec<Range<usize>> {
        let mut lines: Vec<Range<usize>> = Vec::new();
        let mut line_start = 0usize;
        let mut line_w = 0i32;
        let mut last_space: Option<usize> = None;
        for (bi, ch) in s.char_indices() {
            if ch == '\n' {
                lines.push(line_start..bi);
                line_start = bi + 1;
                line_w = 0;
                last_space = None;
                continue;
            }
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
            None => (self.width(s), self.line_h),
            Some(max_w) => self.with_wrapped(s, max_w, |w| w.size),
        }
    }

    /// Whether pixel `(x, y)` relative to (pen x, line top) is ink in `ch`.
    pub fn glyph_cell(&self, ch: char, x: i32, y: i32) -> bool {
        let glyph = self.glyph(ch);
        let [dx, dy, ..] = glyph.bounds;
        glyph.lit(x - dx, y - dy)
    }

    // ---- atlas ---------------------------------------------------------

    pub fn atlas_size(&self) -> (u32, u32) {
        self.glyphs.atlas_size()
    }

    /// Increases when a newly used glyph adds pixels to the atlas.
    pub fn atlas_revision(&self) -> u64 {
        self.glyphs.revision()
    }

    /// The atlas pixel rect `[x, y, w, h]` of `ch`'s glyph bitmap.
    pub fn atlas_rect(&self, ch: char) -> [u32; 4] {
        self.glyph(ch).atlas
    }

    /// The atlas as tightly-packed RGBA (white glyphs on transparent).
    pub fn build_atlas(&self) -> (Vec<u8>, (u32, u32)) {
        let (w, h) = self.glyphs.atlas_size();
        let mut rgba = vec![0u8; (w * h * 4) as usize];
        for glyph in self.glyphs.rasterized() {
            let [ax, ay, gw, _] = glyph.atlas;
            for (i, _) in glyph.bitmap.iter().enumerate().filter(|(_, lit)| **lit) {
                let (x, y) = (ax + i as u32 % gw, ay + i as u32 / gw);
                let at = ((y * w + x) * 4) as usize;
                rgba[at..at + 4].copy_from_slice(&[255, 255, 255, 255]);
            }
        }
        (rgba, (w, h))
    }
}

/// A raw glyph's ink as a tight bitmap placed against the line's baseline.
fn place(raw: &RawGlyph, baseline: i32) -> Glyph {
    let (Some(x0), Some(y0)) = (
        raw.ink.iter().map(|(x, _)| *x).min(),
        raw.ink.iter().map(|(_, y)| *y).min(),
    ) else {
        return Glyph {
            bitmap: Vec::new(),
            bounds: [0, 0, 0, 0],
            advance: raw.advance,
            atlas: [0; 4],
        };
    };
    let x1 = raw.ink.iter().map(|(x, _)| *x).max().unwrap_or(x0);
    let y1 = raw.ink.iter().map(|(_, y)| *y).max().unwrap_or(y0);
    let (w, h) = (x1 - x0 + 1, y1 - y0 + 1);
    let mut bitmap = vec![false; (w * h) as usize];
    for &(x, y) in &raw.ink {
        bitmap[((y - y0) * w + (x - x0)) as usize] = true;
    }
    Glyph {
        bitmap,
        bounds: [x0, baseline + y0, w, h],
        advance: raw.advance,
        atlas: [0; 4],
    }
}

/// A hollow box over the text body, drawn for unknown codepoints when the
/// chain has no U+FFFD.
fn hollow_box(advance: i32, body: (i32, i32)) -> Glyph {
    let w = (advance - 1).max(2);
    let h = (body.1 - body.0).max(2);
    let bitmap = (0..h)
        .flat_map(|y| (0..w).map(move |x| y == 0 || y == h - 1 || x == 0 || x == w - 1))
        .collect();
    Glyph {
        bitmap,
        bounds: [0, body.0, w, h],
        advance,
        atlas: [0; 4],
    }
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
mod tests;
