//! Renderer-neutral Petramond text: a loaded font's glyph bitmaps, metrics,
//! measurement, wrapping, atlas generation, and CPU rasterization.
//!
//! Text is shared presentation infrastructure, not GUI infrastructure. GUI
//! documents, canvas overlays, HUDs, and tools all consume this crate.
//!
//! Glyphs come from a real font FILE ([`Font::from_ttf`]), rasterized once to
//! 1 bit at the font's design pixel size — so the UI can spell `×`, `ö` and
//! `Æ` instead of falling back to a box, while still looking like it was drawn
//! on a pixel grid. A hardcoded 5×7 ASCII table ([`Font::builtin`]) stays as
//! the fallback for tests, the placeholder theme, and a pack whose font fails
//! to load.
//!
//! There is exactly ONE UI font per process, so the free functions here read a
//! process default that the host installs when it loads the theme
//! ([`install`]). Call sites that hold a font (the GUI theme does) should use
//! its methods directly.

pub mod builtin;
mod font;
pub mod tiny;

pub use font::{Font, FontError, Glyph, ATLAS_COLS};

use std::sync::{Arc, OnceLock, RwLock};

fn slot() -> &'static RwLock<Arc<Font>> {
    static SLOT: OnceLock<RwLock<Arc<Font>>> = OnceLock::new();
    SLOT.get_or_init(|| RwLock::new(Arc::new(Font::builtin())))
}

/// Install the process-wide UI font. The host calls this once, when the theme
/// loads; debug builds call it again when the theme is hot-reloaded.
pub fn install(font: Arc<Font>) {
    if let Ok(mut slot) = slot().write() {
        *slot = font;
    }
}

/// The process-wide UI font (the built-in table until [`install`] runs).
pub fn font() -> Arc<Font> {
    slot()
        .read()
        .map(|slot| Arc::clone(&slot))
        .unwrap_or_else(|_| Arc::new(Font::builtin()))
}

/// Height of a single line's glyph box, in font-pixels.
pub fn line_h() -> i32 {
    font().line_h()
}

/// Baseline-to-baseline distance between wrapped lines, in font-pixels.
pub fn line_advance() -> i32 {
    font().line_advance()
}

/// The uniform atlas cell width, in font-pixels. This is a GLYPH BOX, not an
/// advance: proportional text must be measured, never multiplied.
pub fn cell_w() -> i32 {
    font().cell_w()
}

/// Pen advance of one character, in font-pixels.
pub fn advance(ch: char) -> i32 {
    font().advance(ch)
}

/// Width of `s` on one line, in font-pixels.
pub fn width(s: &str) -> i32 {
    font().width(s)
}

/// Width of the first `byte_end` bytes of `s` — the caret x for an index.
pub fn prefix_width(s: &str, byte_end: usize) -> i32 {
    font().prefix_width(s, byte_end)
}

/// The byte index whose caret position is nearest `x` font-pixels.
pub fn index_at_x(s: &str, x: i32) -> usize {
    font().index_at_x(s, x)
}

/// How many leading characters of `s` fit in `max_w` font-pixels.
pub fn fit_chars(s: &str, max_w: i32) -> usize {
    font().fit_chars(s, max_w)
}

/// Greedy word wrap: byte ranges of lines at most `max_w` font-pixels wide.
pub fn wrap(s: &str, max_w: i32) -> Vec<std::ops::Range<usize>> {
    font().wrap(s, max_w)
}

/// Size in font-pixels of `s` wrapped to `max_w` (`None` = single line).
pub fn measure(s: &str, max_w: Option<i32>) -> (i32, i32) {
    font().measure(s, max_w)
}

/// `true` if cell `(col, row)` of `ch`'s glyph box is lit.
pub fn glyph_cell(ch: char, col: i32, row: i32) -> bool {
    font().glyph_cell(ch, col, row)
}

/// The atlas pixel rect `[x, y, w, h]` of `ch`'s cell.
pub fn atlas_rect(ch: char) -> [u32; 4] {
    font().atlas_rect(ch)
}

/// Atlas pixel size `(w, h)`.
pub fn atlas_size() -> (u32, u32) {
    font().atlas_size()
}

/// Generate the font atlas as tightly-packed RGBA (white glyphs on
/// transparent), suitable for direct texture upload.
pub fn build_atlas() -> (Vec<u8>, (u32, u32)) {
    font().build_atlas()
}

/// Pixel size of one single-line text run at integer glyph scale.
pub fn measure_scaled(s: &str, scale: u8) -> [u32; 2] {
    let font = font();
    let scale = scale.max(1) as u32;
    [
        font.width(s).max(0) as u32 * scale,
        font.line_h() as u32 * scale,
    ]
}

/// Blend one single-line run into a straight-alpha RGBA8 image.
///
/// `position` is the run's top-left in image pixels. Drawing is clipped to the
/// destination, so callers can place labels at image edges without pre-clipping.
pub fn draw_rgba(
    rgba: &mut [u8],
    width: u32,
    text: &str,
    position: [i32; 2],
    scale: u8,
    color: [u8; 4],
) {
    if width == 0 || !rgba.len().is_multiple_of(width as usize * 4) {
        return;
    }
    let font = font();
    let height = rgba.len() / (width as usize * 4);
    let scale = scale.max(1) as i32;
    let mut glyph_x = position[0];
    for ch in text.chars() {
        for row in 0..font.line_h() {
            for col in 0..font.cell_w() {
                if !font.glyph_cell(ch, col, row) {
                    continue;
                }
                let left = glyph_x + col * scale;
                let top = position[1] + row * scale;
                for dy in 0..scale {
                    for dx in 0..scale {
                        blend_rgba_pixel(rgba, width as usize, height, left + dx, top + dy, color);
                    }
                }
            }
        }
        glyph_x += font.advance(ch) * scale;
    }
}

fn blend_rgba_pixel(rgba: &mut [u8], width: usize, height: usize, x: i32, y: i32, color: [u8; 4]) {
    if x < 0 || y < 0 || x >= width as i32 || y >= height as i32 || color[3] == 0 {
        return;
    }
    let i = (y as usize * width + x as usize) * 4;
    let src_a = color[3] as f32 / 255.0;
    let dst_a = rgba[i + 3] as f32 / 255.0;
    let out_a = src_a + dst_a * (1.0 - src_a);
    if out_a <= 0.0 {
        return;
    }
    for channel in 0..3 {
        rgba[i + channel] = ((color[channel] as f32 * src_a
            + rgba[i + channel] as f32 * dst_a * (1.0 - src_a))
            / out_a)
            .round()
            .clamp(0.0, 255.0) as u8;
    }
    rgba[i + 3] = (out_a * 255.0).round() as u8;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shipped font is what players read: it must load at its declared
    /// size and cover the characters item names and UI copy actually use.
    #[test]
    fn the_shipped_font_loads_and_covers_european_latin() {
        let bytes = std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../assets/ui/font/DepartureMono-Regular.otf"
        ))
        .expect("shipped font is vendored");
        let font = Font::from_ttf(&bytes, 11.0).expect("shipped font rasterizes at its size");
        for ch in "AZaz09 ×·—…'\"Ööäëéèñçßæø".chars() {
            assert!(font.has_glyph(ch), "missing glyph {ch:?}");
        }
        assert!(font.glyph_count() > 200, "{}", font.glyph_count());
        // Real glyph coverage costs room: accented capitals sit above the cap
        // line and descenders below the baseline.
        let builtin = Font::builtin();
        assert!(font.line_h() > builtin.line_h());
    }

    /// The rasterizer reproduces the font's own pixel grid.
    ///
    /// These are the glyphs that caught it getting this wrong: `w`, `W` and
    /// `M` each pack three one-pixel stems into five columns, so any
    /// decimation that averages a cell instead of point-sampling its centre
    /// merges them into a blob — `hushjaw` renders as `hushjau`. The expected
    /// bitmaps are FreeType's hinted output for the same face and size.
    #[test]
    fn glyph_bitmaps_match_the_fonts_own_pixel_grid() {
        let bytes = std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../assets/ui/font/DepartureMono-Regular.otf"
        ))
        .expect("shipped font is vendored");
        let font = Font::from_ttf(&bytes, 11.0).expect("shipped font rasterizes");
        let expect: &[(char, &[&str])] = &[
            (
                'w',
                &["#.#.#", "#.#.#", "#.#.#", "#.#.#", "#.#.#", ".#.##"],
            ),
            (
                'W',
                &[
                    "#...#", "#...#", "#...#", "#.#.#", "#.#.#", ".#.#.", ".#.#.", ".#.#.",
                ],
            ),
            (
                'M',
                &[
                    "#...#", "#...#", "##.##", "#.#.#", "#.#.#", "#...#", "#...#", "#...#",
                ],
            ),
        ];
        for (ch, want) in expect {
            let got = trimmed_glyph(&font, *ch);
            assert_eq!(&got, want, "{ch:?} rasterized as {got:#?}");
        }
        // One-pixel stems mean the advance leaves exactly one column of gap.
        assert_eq!(font.advance('W'), 7);
    }

    /// A glyph's lit cells with the blank border trimmed off.
    fn trimmed_glyph(font: &Font, ch: char) -> Vec<String> {
        let lit = |x: i32, y: i32| font.glyph_cell(ch, x, y);
        let rows: Vec<i32> = (0..font.line_h())
            .filter(|&y| (0..font.cell_w()).any(|x| lit(x, y)))
            .collect();
        let cols: Vec<i32> = (0..font.cell_w())
            .filter(|&x| (0..font.line_h()).any(|y| lit(x, y)))
            .collect();
        let (x0, x1) = (cols[0], *cols.last().unwrap());
        rows.iter()
            .map(|&y| {
                (x0..=x1)
                    .map(|x| if lit(x, y) { '#' } else { '.' })
                    .collect()
            })
            .collect()
    }

    #[test]
    fn a_broken_font_file_is_an_error_not_a_panic() {
        assert!(Font::from_ttf(b"not a font", 10.0).is_err());
        assert!(Font::from_ttf(&[], 10.0).is_err());
    }

    #[test]
    fn free_functions_read_the_installed_font() {
        // The default is the built-in table until a host installs one.
        assert_eq!(width("AB"), font().width("AB"));
        assert_eq!(measure("hi", None), (width("hi"), line_h()));
    }
}






