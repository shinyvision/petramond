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

const PROBE_ALL: [u32; 2] = [0x20, 0x1FFFF];

pub(super) struct RawGlyph {
    pub advance: i32,
    pub ink: Vec<(i32, i32)>,
}

#[derive(Clone, Copy)]
pub(super) struct FaceMetrics {
    pub ascent: i32,
    pub line_h: i32,
}

pub(super) struct Face {
    font: ab_glyph::FontVec,
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

    pub fn coverage(&self, ranges: &[[u32; 2]], wanted: &dyn Fn(char) -> bool) -> Vec<char> {
        candidate_chars(&self.font, ranges)
            .into_iter()
            .filter(|&ch| wanted(ch) && self.font.glyph_id(ch).0 != 0)
            .collect()
    }

    pub fn advance(&self, ch: char) -> i32 {
        let scaled = self.font.as_scaled(self.raster_px);
        scaled.h_advance(self.font.glyph_id(ch)).round().max(0.0) as i32
    }

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

fn candidate_chars(face: &ab_glyph::FontVec, ranges: &[[u32; 2]]) -> Vec<char> {
    let mut out: Vec<char> = if ranges.is_empty() {
        let [lo, hi] = PROBE_ALL;
        let mut chars: Vec<char> = (lo..=hi).filter_map(char::from_u32).collect();
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

pub(super) fn builtin_metrics() -> FaceMetrics {
    use crate::builtin::GLYPH_H;
    FaceMetrics {
        ascent: GLYPH_H,
        line_h: GLYPH_H,
    }
}

pub(super) fn builtin_chars(replacement: bool) -> Vec<char> {
    (0x20u8..0x7F)
        .map(char::from)
        .chain(replacement.then_some('\u{FFFD}'))
        .collect()
}

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
