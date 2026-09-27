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

const BODY_TOP_SAMPLE: &str = "MHTbdkl";
const BODY_BOTTOM_SAMPLE: &str = "gjpqy";

#[derive(Clone, Copy, Debug)]
pub struct FaceSource<'a> {
    pub bytes: &'a [u8],
    pub px: f32,
    pub ranges: &'a [[u32; 2]],
}

#[derive(Clone, Debug)]
pub struct Glyph {
    bitmap: Vec<bool>,
    bounds: [i32; 4],
    advance: i32,
    atlas: [u32; 4],
}

impl Glyph {
    pub fn advance(&self) -> i32 {
        self.advance
    }

    pub fn bounds(&self) -> [i32; 4] {
        self.bounds
    }

    pub fn atlas_rect(&self) -> [u32; 4] {
        self.atlas
    }

    pub fn lit(&self, x: i32, y: i32) -> bool {
        let [_, _, w, h] = self.bounds;
        (0..w).contains(&x) && (0..h).contains(&y) && self.bitmap[(y * w + x) as usize]
    }

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

#[derive(Debug)]
pub struct Font {
    line_h: i32,
    line_advance: i32,
    max_advance: i32,
    body: (i32, i32),
    glyphs: GlyphSet,
    wraps: Mutex<WrapCache>,
}

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

const WRAP_CACHE_CAP: usize = 4096;

impl Font {
    pub fn builtin() -> Font {
        let metrics = raster::builtin_metrics();
        let covered = raster::builtin_chars(true)
            .into_iter()
            .map(|ch| (ch, 0))
            .collect();
        Font::assemble(vec![Source::Builtin], covered, metrics, false)
            .expect("the built-in table always assembles")
    }

    pub fn from_ttf(bytes: &[u8], px: f32) -> Result<Font, FontError> {
        Font::from_faces(&[FaceSource {
            bytes,
            px,
            ranges: &[],
        }])
    }

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

    pub fn line_h(&self) -> i32 {
        self.line_h
    }

    pub fn line_advance(&self) -> i32 {
        self.line_advance
    }

    pub fn max_advance(&self) -> i32 {
        self.max_advance
    }

    pub fn body_span(&self) -> (i32, i32) {
        let (top, bottom) = self.body;
        (top, (bottom - top).max(1))
    }

    pub fn glyph_count(&self) -> usize {
        self.glyphs.count()
    }

    pub fn glyph(&self, ch: char) -> &Glyph {
        self.glyphs.glyph(ch)
    }

    pub fn has_glyph(&self, ch: char) -> bool {
        self.glyphs.has(ch)
    }

    pub fn advance(&self, ch: char) -> i32 {
        self.glyphs.advance(ch)
    }

    pub fn width(&self, s: &str) -> i32 {
        s.chars().map(|ch| self.advance(ch)).sum()
    }

    pub fn prefix_width(&self, s: &str, byte_end: usize) -> i32 {
        let end = byte_end.min(s.len());
        self.width(&s[..floor_char_boundary(s, end)])
    }

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

    pub fn wrap(&self, s: &str, max_w: i32) -> Vec<Range<usize>> {
        self.with_wrapped(s, max_w, |w| w.lines.clone())
    }

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

    pub fn measure(&self, s: &str, max_w: Option<i32>) -> (i32, i32) {
        match max_w {
            None => (self.width(s), self.line_h),
            Some(max_w) => self.with_wrapped(s, max_w, |w| w.size),
        }
    }

    pub fn glyph_cell(&self, ch: char, x: i32, y: i32) -> bool {
        let glyph = self.glyph(ch);
        let [dx, dy, ..] = glyph.bounds;
        glyph.lit(x - dx, y - dy)
    }

    pub fn atlas_size(&self) -> (u32, u32) {
        self.glyphs.atlas_size()
    }

    pub fn atlas_revision(&self) -> u64 {
        self.glyphs.revision()
    }

    pub fn atlas_rect(&self, ch: char) -> [u32; 4] {
        self.glyph(ch).atlas
    }

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

fn floor_char_boundary(s: &str, index: usize) -> usize {
    let mut i = index.min(s.len());
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

#[cfg(test)]
mod tests;
