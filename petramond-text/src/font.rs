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
mod raster;

use raster::{FaceMetrics, RawGlyph};
use std::collections::HashMap;
use std::ops::Range;
use std::sync::Mutex;

/// Codepoints below this resolve through a dense table (Latin, Greek,
/// Cyrillic — the text the UI actually draws); the rest through a map.
const DENSE_LIMIT: u32 = 0x800;
const NO_GLYPH: u32 = u32::MAX;

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
    /// Every real glyph, then the fallback last.
    glyphs: Vec<Glyph>,
    dense: Vec<u32>,
    sparse: HashMap<char, u32>,
    atlas_size: (u32, u32),
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

/// Where one chain member's raw glyphs came from.
struct Contribution {
    metrics: FaceMetrics,
    glyphs: Vec<RawGlyph>,
}

impl Font {
    /// The built-in 5×7 ASCII table — the fallback when no font file loads.
    pub fn builtin() -> Font {
        let (metrics, glyphs) = raster::builtin_glyphs(true);
        Font::assemble(vec![Contribution { metrics, glyphs }], false)
            .expect("the built-in table always assembles")
    }

    /// Rasterize one TrueType/OpenType face at `px` pixels per EM — every
    /// codepoint it maps — closed by the built-in table.
    pub fn from_ttf(bytes: &[u8], px: f32) -> Result<Font, FontError> {
        Font::from_faces(&[FaceSource {
            bytes,
            px,
            ranges: &[],
        }])
    }

    /// A fallback chain: the first face is primary (its ascent/descent set
    /// the line box); each later face fills only codepoints every earlier
    /// face lacks; the built-in table closes the chain.
    pub fn from_faces(faces: &[FaceSource<'_>]) -> Result<Font, FontError> {
        let mut parts: Vec<Contribution> = Vec::new();
        let mut covered: std::collections::HashSet<char> = std::collections::HashSet::new();
        for source in faces {
            let (metrics, glyphs) = raster::rasterize_face(source, &|ch| !covered.contains(&ch))?;
            covered.extend(glyphs.iter().map(|g| g.ch));
            parts.push(Contribution { metrics, glyphs });
        }
        let primary_has_ink = parts
            .first()
            .is_some_and(|p| p.glyphs.iter().any(|g| !g.ink.is_empty()));
        if !primary_has_ink {
            return Err(FontError::Empty);
        }
        let (metrics, mut glyphs) = raster::builtin_glyphs(false);
        glyphs.retain(|g| !covered.contains(&g.ch));
        parts.push(Contribution { metrics, glyphs });
        Font::assemble(parts, true)
    }

    /// Lay every contribution's glyphs against the primary's baseline, pick
    /// the fallback, and pack the atlas. `boxed_fallback`: with no U+FFFD in
    /// the chain, draw a hollow box over the text body.
    fn assemble(parts: Vec<Contribution>, boxed_fallback: bool) -> Result<Font, FontError> {
        let primary = parts.first().map(|p| p.metrics).ok_or(FontError::Empty)?;
        let baseline = primary.ascent;
        let line_h = primary.line_h;

        let mut glyphs: Vec<Glyph> = Vec::new();
        let mut chars: Vec<char> = Vec::new();
        for part in parts {
            for raw in part.glyphs {
                glyphs.push(place(&raw, baseline));
                chars.push(raw.ch);
            }
        }
        let find = |ch: char| chars.iter().position(|c| *c == ch);
        let body = {
            let top = BODY_TOP_SAMPLE
                .chars()
                .filter_map(|ch| find(ch).map(|i| &glyphs[i]))
                .filter(|g| g.bounds[3] > 0)
                .map(|g| g.bounds[1])
                .min()
                .unwrap_or(0);
            let bottom = BODY_BOTTOM_SAMPLE
                .chars()
                .filter_map(|ch| find(ch).map(|i| &glyphs[i]))
                .filter(|g| g.bounds[3] > 0)
                .map(|g| g.bounds[1] + g.bounds[3])
                .max()
                .unwrap_or(line_h);
            (top, bottom.max(top + 1))
        };

        // Unknown codepoints share one glyph: U+FFFD from the chain, else a
        // hollow box over the body — never a blank, so a missing glyph shows.
        let fallback = match find('\u{FFFD}') {
            Some(i) if !boxed_fallback || glyphs[i].bounds[3] > 0 => glyphs[i].clone(),
            _ => {
                let advance = find('?').map_or(line_h / 2, |i| glyphs[i].advance).max(3);
                hollow_box(advance, body)
            }
        };
        glyphs.push(fallback);

        let sizes: Vec<(u32, u32)> = glyphs
            .iter()
            .map(|g| (g.bounds[2] as u32, g.bounds[3] as u32))
            .collect();
        let (origins, atlas_size) = atlas::pack(&sizes)?;
        for (glyph, [x, y]) in glyphs.iter_mut().zip(origins) {
            glyph.atlas = [x, y, glyph.bounds[2] as u32, glyph.bounds[3] as u32];
        }

        let mut dense = vec![NO_GLYPH; DENSE_LIMIT as usize];
        let mut sparse = HashMap::new();
        for (i, ch) in chars.iter().enumerate() {
            match dense.get_mut(*ch as usize) {
                Some(slot) => *slot = i as u32,
                None => {
                    sparse.insert(*ch, i as u32);
                }
            }
        }
        let max_advance = glyphs.iter().map(|g| g.advance).max().unwrap_or(1).max(1);
        Ok(Font {
            line_h,
            // One blank pixel row between lines, like the built-in font.
            line_advance: line_h + 2,
            max_advance,
            body,
            glyphs,
            dense,
            sparse,
            atlas_size,
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
        self.glyphs.len() - 1
    }

    fn index(&self, ch: char) -> Option<u32> {
        let i = match self.dense.get(ch as usize) {
            Some(&i) => i,
            None => *self.sparse.get(&ch)?,
        };
        (i != NO_GLYPH).then_some(i)
    }

    pub fn glyph(&self, ch: char) -> &Glyph {
        let i = self.index(ch).unwrap_or(self.glyphs.len() as u32 - 1);
        &self.glyphs[i as usize]
    }

    /// Whether `ch` has its own glyph (false = it draws the fallback).
    pub fn has_glyph(&self, ch: char) -> bool {
        self.index(ch).is_some()
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
    /// spaces where possible and mid-word only when a word alone overflows.
    /// Returns byte ranges into `s`; never empty (empty text = one empty line).
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
        self.atlas_size
    }

    /// The atlas pixel rect `[x, y, w, h]` of `ch`'s glyph bitmap.
    pub fn atlas_rect(&self, ch: char) -> [u32; 4] {
        self.glyph(ch).atlas
    }

    /// The atlas as tightly-packed RGBA (white glyphs on transparent).
    pub fn build_atlas(&self) -> (Vec<u8>, (u32, u32)) {
        let (w, h) = self.atlas_size;
        let mut rgba = vec![0u8; (w * h * 4) as usize];
        for glyph in &self.glyphs {
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
