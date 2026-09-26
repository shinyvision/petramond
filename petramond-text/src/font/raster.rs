//! Face rasterization: every codepoint a source covers, thresholded to 1 bit
//! at its design size, as ink relative to the pen origin on the baseline.

use super::{FaceSource, FontError};

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
    pub ch: char,
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

/// Rasterize every codepoint `source` covers that `wanted` still accepts.
pub(super) fn rasterize_face(
    source: &FaceSource<'_>,
    wanted: &dyn Fn(char) -> bool,
) -> Result<(FaceMetrics, Vec<RawGlyph>), FontError> {
    use ab_glyph::{Font as _, ScaleFont as _};
    let face = ab_glyph::FontRef::try_from_slice(source.bytes)
        .map_err(|e| FontError::Parse(e.to_string()))?;
    // ab_glyph scales against the font's ASCENT+DESCENT, not its em square,
    // so a scale of `px` renders an em of `px * em / height` — smaller than
    // the design size, and off the pixel grid. Convert, or every glyph comes
    // out shrunk and mangled.
    let px = source.px;
    let raster_px = match face.units_per_em().filter(|em| *em > 0.0) {
        Some(em) => px * face.height_unscaled() / em,
        None => px,
    };
    let scaled = face.as_scaled(raster_px);
    let ascent = scaled.ascent().round() as i32;
    let descent = scaled.descent().round() as i32;
    let metrics = FaceMetrics {
        ascent,
        line_h: (ascent - descent).max(1),
    };

    let ss = SUPERSAMPLE.max(1);
    let mut glyphs = Vec::new();
    for ch in candidate_chars(&face, source.ranges) {
        if !wanted(ch) {
            continue;
        }
        let id = face.glyph_id(ch);
        if id.0 == 0 {
            continue; // the face has no glyph for this codepoint
        }
        let advance = scaled.h_advance(id).round() as i32;
        let mut ink = Vec::new();
        if let Some(outline) = face.outline_glyph(id.with_scale(raster_px * ss as f32)) {
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
        glyphs.push(RawGlyph {
            ch,
            advance: advance.max(0),
            ink,
        });
    }
    Ok((metrics, glyphs))
}

/// Every printable codepoint to try, in ascending order: the declared
/// ranges, or (none declared) everything the face maps.
fn candidate_chars(face: &ab_glyph::FontRef<'_>, ranges: &[[u32; 2]]) -> Vec<char> {
    use ab_glyph::Font as _;
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

/// The built-in 5×7 ASCII table as raw glyphs, baseline at the bottom row.
/// `replacement` adds its U+FFFD box — wanted only when the table is the
/// primary face: at the end of a real font's chain a 5×7 box would be a
/// speck, and the chain draws a body-sized box instead.
pub(super) fn builtin_glyphs(replacement: bool) -> (FaceMetrics, Vec<RawGlyph>) {
    use crate::builtin::{glyph, ADVANCE, GLYPH_H, GLYPH_W};
    let raw = |ch: char| {
        let rows = glyph(ch);
        let mut ink = Vec::new();
        for (row, bits) in rows.iter().enumerate() {
            for col in 0..GLYPH_W {
                if (bits >> (GLYPH_W - 1 - col)) & 1 == 1 {
                    ink.push((col, row as i32 - GLYPH_H));
                }
            }
        }
        RawGlyph {
            ch,
            advance: ADVANCE,
            ink,
        }
    };
    let glyphs = (0x20u8..0x7F)
        .map(char::from)
        .chain(replacement.then_some('\u{FFFD}'))
        .map(raw)
        .collect();
    let metrics = FaceMetrics {
        ascent: GLYPH_H,
        line_h: GLYPH_H,
    };
    (metrics, glyphs)
}
