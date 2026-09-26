//! Face rasterization: one glyph at a time, thresholded to 1 bit at the
//! face's design size, as ink relative to the pen origin on the baseline.
//!
//! A [`Face`] owns its font file so glyphs can be rasterized whenever text
//! first uses them, long after loading.

use super::{FaceSource, FontError};
use ab_glyph::{Font as _, ScaleFont as _};

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

/// The codepoint space probed when a face declares no ranges: the Basic
/// Multilingual Plane and the Supplementary Multilingual Plane (historic
/// scripts, symbols, emoji). Probing (rather than walking the cmap) keeps
/// codepoints that share one glyph — a space and a no-break space — both
/// covered.
const PROBE_ALL: [u32; 2] = [0x20, 0x1FFFF];

/// One rasterized glyph before layout: ink pixels relative to the pen origin
/// on the baseline (y down, negative above the baseline).
pub(super) struct RawGlyph {
    pub advance: i32,
    pub ink: Vec<(i32, i32)>,
}

/// A face's vertical metrics at its design size, in font-pixels.
#[derive(Clone, Copy)]
pub(super) struct FaceMetrics {
    /// Baseline distance from the top of the line box.
    pub ascent: i32,
    /// Line box height (ascent + descent).
    pub line_h: i32,
}

/// One loaded face of a chain, ready to rasterize any glyph it maps.
pub(super) struct Face {
    font: ab_glyph::FontVec,
    /// The ab_glyph scale that renders an em of the authored pixel size.
    raster_px: f32,
    pub metrics: FaceMetrics,
}

impl std::fmt::Debug for Face {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Face")
            .field("raster_px", &self.raster_px)
            .finish_non_exhaustive()
    }
}

impl Face {
    /// Parse `source` (copying its bytes, so the face outlives the caller's
    /// buffer).
    pub fn open(source: &FaceSource<'_>) -> Result<Face, FontError> {
        let font = ab_glyph::FontVec::try_from_vec(source.bytes.to_vec())
            .map_err(|e| FontError::Parse(e.to_string()))?;
        // ab_glyph scales against the font's ASCENT+DESCENT, not its em
        // square, so a scale of `px` renders an em of `px * em / height` —
        // smaller than the design size, and off the pixel grid. Convert, or
        // every glyph comes out shrunk and mangled.
        let px = source.px;
        let raster_px = match font.units_per_em().filter(|em| *em > 0.0) {
            Some(em) => px * font.height_unscaled() / em,
            None => px,
        };
        let scaled = font.as_scaled(raster_px);
        let ascent = scaled.ascent().round() as i32;
        let descent = scaled.descent().round() as i32;
        let metrics = FaceMetrics {
            ascent,
            line_h: (ascent - descent).max(1),
        };
        Ok(Face {
            font,
            raster_px,
            metrics,
        })
    }

    /// Every printable codepoint this face maps within `ranges` (none =
    /// everything it maps) that `wanted` still accepts, ascending.
    pub fn coverage(&self, ranges: &[[u32; 2]], wanted: &dyn Fn(char) -> bool) -> Vec<char> {
        candidate_chars(&self.font, ranges)
            .into_iter()
            .filter(|&ch| wanted(ch) && self.font.glyph_id(ch).0 != 0)
            .collect()
    }

    /// Pen advance of `ch`, in font-pixels, without rasterizing it.
    pub fn advance(&self, ch: char) -> i32 {
        let scaled = self.font.as_scaled(self.raster_px);
        scaled.h_advance(self.font.glyph_id(ch)).round().max(0.0) as i32
    }

    /// Rasterize `ch` at the design size.
    pub fn raster(&self, ch: char) -> RawGlyph {
        let id = self.font.glyph_id(ch);
        let ss = SUPERSAMPLE.max(1);
        let mut ink = Vec::new();
        if let Some(outline) = self
            .font
            .outline_glyph(id.with_scale(self.raster_px * ss as f32))
        {
            let bounds = outline.px_bounds();
            let (hi_x, hi_y) = (bounds.min.x.floor() as i32, bounds.min.y.floor() as i32);
            // Take the centre subpixel of each target pixel, in ABSOLUTE
            // coordinates so a glyph's own origin cannot shift the grid under
            // it.
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
        }
        RawGlyph {
            advance: self.advance(ch),
            ink,
        }
    }
}

/// Every printable codepoint to try, in ascending order: the declared
/// ranges, or (none declared) everything the face maps.
fn candidate_chars(face: &ab_glyph::FontVec, ranges: &[[u32; 2]]) -> Vec<char> {
    let mut out: Vec<char> = if ranges.is_empty() {
        let [lo, hi] = PROBE_ALL;
        let mut chars: Vec<char> = (lo..=hi).filter_map(char::from_u32).collect();
        // Mapped codepoints beyond the probed planes (private use, tags).
        chars.extend(
            face.codepoint_ids()
                .map(|(_, ch)| ch)
                .filter(|ch| (*ch as u32) > hi),
        );
        chars
    } else {
        ranges
            .iter()
            .flat_map(|&[lo, hi]| lo..=hi)
            .filter_map(char::from_u32)
            .collect()
    };
    out.retain(|ch| !ch.is_control());
    out.sort_unstable();
    out.dedup();
    out
}

/// The built-in 5×7 table's vertical metrics (baseline at the bottom row).
pub(super) fn builtin_metrics() -> FaceMetrics {
    use crate::builtin::GLYPH_H;
    FaceMetrics {
        ascent: GLYPH_H,
        line_h: GLYPH_H,
    }
}

/// The codepoints the built-in table draws: printable ASCII, plus its U+FFFD
/// box when `replacement` — wanted only when the table is the primary face:
/// at the end of a real font's chain a 5×7 box would be a speck, and the
/// chain draws a body-sized box instead.
pub(super) fn builtin_chars(replacement: bool) -> Vec<char> {
    (0x20u8..0x7F)
        .map(char::from)
        .chain(replacement.then_some('\u{FFFD}'))
        .collect()
}

/// One built-in glyph as raw ink, baseline at the bottom row.
pub(super) fn builtin_raw(ch: char) -> RawGlyph {
    use crate::builtin::{glyph, ADVANCE, GLYPH_H, GLYPH_W};
    let mut ink = Vec::new();
    for (row, bits) in glyph(ch).iter().enumerate() {
        for col in 0..GLYPH_W {
            if (bits >> (GLYPH_W - 1 - col)) & 1 == 1 {
                ink.push((col, row as i32 - GLYPH_H));
            }
        }
    }
    RawGlyph {
        advance: ADVANCE,
        ink,
    }
}
