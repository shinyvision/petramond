//! Renderer-neutral Petramond text: glyph bitmaps, measurement, wrapping,
//! atlas generation, and CPU rasterization.
//!
//! Text is shared presentation infrastructure, not GUI infrastructure. GUI
//! documents, canvas overlays, HUDs, and tools all consume this crate. The atlas generator
//! turns the tables into a plain RGBA grid (ASCII 32..=126 plus a fallback
//! cell), so paint samples a texture like any other — and a future theme can
//! swap in a hand-drawn `font.png` of the same geometry.

pub mod tiny;

/// Glyph width in font-pixels.
pub const GLYPH_W: i32 = 5;
/// Glyph height in font-pixels.
pub const GLYPH_H: i32 = 7;
/// Horizontal advance per glyph (glyph + 1px gap).
pub const ADVANCE: i32 = GLYPH_W + 1;
/// Vertical advance between wrapped lines (glyph + 2px leading).
pub const LINE_ADVANCE: i32 = GLYPH_H + 2;

/// Width in font-pixels of a single-line run of `chars` characters.
pub fn width_chars(chars: usize) -> i32 {
    if chars == 0 {
        0
    } else {
        chars as i32 * ADVANCE - 1
    }
}

/// Width in font-pixels of `s` on one line.
pub fn width(s: &str) -> i32 {
    width_chars(s.chars().count())
}

/// Greedy word wrap: split `s` into lines of at most `max_w` font-pixels,
/// breaking at spaces where possible and mid-word only when a word alone
/// exceeds the width. Returns byte ranges into `s`. Never returns an empty
/// vec (empty text = one empty line).
pub fn wrap(s: &str, max_w: i32) -> Vec<std::ops::Range<usize>> {
    let per_line = ((max_w + 1) / ADVANCE).max(1) as usize;
    let mut lines: Vec<std::ops::Range<usize>> = Vec::new();
    let mut line_start = 0usize;
    let mut line_chars = 0usize;
    let mut last_space: Option<usize> = None; // byte index of last space on line
    for (bi, ch) in s.char_indices() {
        if line_chars + 1 > per_line {
            let break_at = match last_space {
                // Break after the space; the space itself is swallowed.
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
            line_chars = s[line_start..bi].chars().count();
            last_space = None;
        }
        if ch == ' ' {
            last_space = Some(bi);
        }
        line_chars += 1;
    }
    lines.push(line_start..s.len());
    lines
}

/// Size in font-pixels of `s` wrapped to `max_w` (`None` = single line).
pub fn measure(s: &str, max_w: Option<i32>) -> (i32, i32) {
    match max_w {
        None => (width(s), GLYPH_H),
        Some(max_w) => {
            let lines = wrap(s, max_w);
            let w = lines
                .iter()
                .map(|r| width(&s[r.clone()]))
                .max()
                .unwrap_or(0);
            let h = GLYPH_H + (lines.len() as i32 - 1) * LINE_ADVANCE;
            (w, h)
        }
    }
}

/// `true` if cell `(col, row)` of `ch`'s 5×7 glyph is lit.
pub fn glyph_cell(ch: char, col: i32, row: i32) -> bool {
    if !(0..GLYPH_W).contains(&col) || !(0..GLYPH_H).contains(&row) {
        return false;
    }
    let bits = glyph(ch)[row as usize];
    (bits >> (GLYPH_W - 1 - col)) & 1 == 1
}

// ---- font atlas -------------------------------------------------------------

/// Codepoints covered by the atlas grid, in cell order: printable ASCII, then
/// one trailing fallback cell every unknown character maps to.
pub const ATLAS_FIRST: u32 = 32;
pub const ATLAS_LAST: u32 = 126;
pub const ATLAS_COLS: u32 = 16;

/// Grid cell index of `ch` in the font atlas.
pub fn atlas_cell(ch: char) -> u32 {
    let cp = ch as u32;
    if (ATLAS_FIRST..=ATLAS_LAST).contains(&cp) {
        cp - ATLAS_FIRST
    } else {
        ATLAS_LAST - ATLAS_FIRST + 1
    }
}

/// Total cells in the atlas (ASCII range + fallback).
pub fn atlas_cells() -> u32 {
    ATLAS_LAST - ATLAS_FIRST + 2
}

/// Atlas pixel size `(w, h)`.
pub fn atlas_size() -> (u32, u32) {
    let rows = atlas_cells().div_ceil(ATLAS_COLS);
    (ATLAS_COLS * GLYPH_W as u32, rows * GLYPH_H as u32)
}

/// The UV-space pixel rect `[x, y, w, h]` of `ch`'s atlas cell.
pub fn atlas_rect(ch: char) -> [u32; 4] {
    let cell = atlas_cell(ch);
    let (cx, cy) = (cell % ATLAS_COLS, cell / ATLAS_COLS);
    [
        cx * GLYPH_W as u32,
        cy * GLYPH_H as u32,
        GLYPH_W as u32,
        GLYPH_H as u32,
    ]
}

/// Generate the font atlas as tightly-packed RGBA (white glyphs on
/// transparent), suitable for direct texture upload.
pub fn build_atlas() -> (Vec<u8>, (u32, u32)) {
    let (w, h) = atlas_size();
    let mut rgba = vec![0u8; (w * h * 4) as usize];
    let mut blit = |cell: u32, ch: char| {
        let (cx, cy) = (cell % ATLAS_COLS, cell / ATLAS_COLS);
        for row in 0..GLYPH_H {
            for col in 0..GLYPH_W {
                if glyph_cell(ch, col, row) {
                    let px = cx * GLYPH_W as u32 + col as u32;
                    let py = cy * GLYPH_H as u32 + row as u32;
                    let i = ((py * w + px) * 4) as usize;
                    rgba[i..i + 4].copy_from_slice(&[255, 255, 255, 255]);
                }
            }
        }
    };
    for cp in ATLAS_FIRST..=ATLAS_LAST {
        blit(cp - ATLAS_FIRST, char::from_u32(cp).unwrap());
    }
    blit(ATLAS_LAST - ATLAS_FIRST + 1, '\u{FFFD}'); // fallback cell ('?'-boxed glyph)
    (rgba, (w, h))
}

/// Pixel size of one single-line text run at integer glyph scale.
pub fn measure_scaled(s: &str, scale: u8) -> [u32; 2] {
    let scale = scale.max(1) as u32;
    [width(s).max(0) as u32 * scale, GLYPH_H as u32 * scale]
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
    if width == 0 || rgba.len() % (width as usize * 4) != 0 {
        return;
    }
    let height = rgba.len() / (width as usize * 4);
    let scale = scale.max(1) as i32;
    let mut glyph_x = position[0];
    for ch in text.chars() {
        for row in 0..GLYPH_H {
            for col in 0..GLYPH_W {
                if !glyph_cell(ch, col, row) {
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
        glyph_x += ADVANCE * scale;
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

fn glyph(ch: char) -> [u8; GLYPH_H as usize] {
    if ch.is_ascii_lowercase() {
        return lowercase_glyph(ch);
    }
    match ch.to_ascii_uppercase() {
        'A' => [14, 17, 17, 31, 17, 17, 17],
        'B' => [30, 17, 17, 30, 17, 17, 30],
        'C' => [14, 17, 16, 16, 16, 17, 14],
        'D' => [30, 17, 17, 17, 17, 17, 30],
        'E' => [31, 16, 16, 30, 16, 16, 31],
        'F' => [31, 16, 16, 30, 16, 16, 16],
        'G' => [14, 17, 16, 23, 17, 17, 15],
        'H' => [17, 17, 17, 31, 17, 17, 17],
        'I' => [14, 4, 4, 4, 4, 4, 14],
        'J' => [7, 2, 2, 2, 18, 18, 12],
        'K' => [17, 18, 20, 24, 20, 18, 17],
        'L' => [16, 16, 16, 16, 16, 16, 31],
        'M' => [17, 27, 21, 21, 17, 17, 17],
        'N' => [17, 25, 21, 19, 17, 17, 17],
        'O' => [14, 17, 17, 17, 17, 17, 14],
        'P' => [30, 17, 17, 30, 16, 16, 16],
        'Q' => [14, 17, 17, 17, 21, 18, 13],
        'R' => [30, 17, 17, 30, 20, 18, 17],
        'S' => [15, 16, 16, 14, 1, 1, 30],
        'T' => [31, 4, 4, 4, 4, 4, 4],
        'U' => [17, 17, 17, 17, 17, 17, 14],
        'V' => [17, 17, 17, 17, 17, 10, 4],
        'W' => [17, 17, 17, 21, 21, 21, 10],
        'X' => [17, 17, 10, 4, 10, 17, 17],
        'Y' => [17, 17, 10, 4, 4, 4, 4],
        'Z' => [31, 1, 2, 4, 8, 16, 31],
        '0' => [14, 17, 19, 21, 25, 17, 14],
        '1' => [4, 12, 4, 4, 4, 4, 14],
        '2' => [14, 17, 1, 2, 4, 8, 31],
        '3' => [30, 1, 1, 14, 1, 1, 30],
        '4' => [2, 6, 10, 18, 31, 2, 2],
        '5' => [31, 16, 16, 30, 1, 1, 30],
        '6' => [14, 16, 16, 30, 17, 17, 14],
        '7' => [31, 1, 2, 4, 8, 8, 8],
        '8' => [14, 17, 17, 14, 17, 17, 14],
        '9' => [14, 17, 17, 15, 1, 1, 14],
        ' ' => [0, 0, 0, 0, 0, 0, 0],
        '-' => [0, 0, 0, 31, 0, 0, 0],
        '_' => [0, 0, 0, 0, 0, 0, 31],
        '.' => [0, 0, 0, 0, 0, 4, 4],
        ',' => [0, 0, 0, 0, 4, 4, 8],
        ':' => [0, 4, 4, 0, 4, 4, 0],
        ';' => [0, 4, 4, 0, 4, 4, 8],
        '!' => [4, 4, 4, 4, 4, 0, 4],
        '?' => [14, 17, 1, 2, 4, 0, 4],
        '/' => [1, 1, 2, 4, 8, 16, 16],
        '\\' => [16, 16, 8, 4, 2, 1, 1],
        '\'' => [4, 4, 8, 0, 0, 0, 0],
        '"' => [10, 10, 0, 0, 0, 0, 0],
        '(' => [2, 4, 8, 8, 8, 4, 2],
        ')' => [8, 4, 2, 2, 2, 4, 8],
        '[' => [14, 8, 8, 8, 8, 8, 14],
        ']' => [14, 2, 2, 2, 2, 2, 14],
        '<' => [2, 4, 8, 16, 8, 4, 2],
        '>' => [8, 4, 2, 1, 2, 4, 8],
        '+' => [0, 4, 4, 31, 4, 4, 0],
        '=' => [0, 0, 31, 0, 31, 0, 0],
        '*' => [0, 21, 14, 31, 14, 21, 0],
        '$' => [4, 15, 20, 14, 5, 30, 4],
        '#' => [10, 31, 10, 10, 31, 10, 0],
        '@' => [14, 17, 23, 21, 23, 16, 14],
        '%' => [17, 1, 2, 4, 8, 16, 17],
        '&' => [12, 18, 20, 8, 21, 18, 13],
        '|' => [4, 4, 4, 4, 4, 4, 4],
        '^' => [4, 10, 17, 0, 0, 0, 0],
        '{' => [2, 4, 4, 8, 4, 4, 2],
        '}' => [8, 4, 4, 2, 4, 4, 8],
        '`' => [8, 4, 2, 0, 0, 0, 0],
        '~' => [0, 0, 8, 21, 2, 0, 0],
        _ => [14, 17, 1, 2, 4, 0, 4],
    }
}

fn lowercase_glyph(ch: char) -> [u8; GLYPH_H as usize] {
    match ch {
        'a' => [0, 0, 14, 1, 15, 17, 15],
        'b' => [16, 16, 22, 25, 17, 17, 30],
        'c' => [0, 0, 14, 16, 16, 17, 14],
        'd' => [1, 1, 13, 19, 17, 17, 15],
        'e' => [0, 0, 14, 17, 31, 16, 14],
        'f' => [6, 8, 8, 30, 8, 8, 8],
        'g' => [0, 0, 14, 17, 15, 1, 14],
        'h' => [16, 16, 22, 25, 17, 17, 17],
        'i' => [4, 0, 12, 4, 4, 4, 14],
        'j' => [2, 0, 2, 2, 2, 18, 12],
        'k' => [16, 16, 18, 20, 24, 20, 18],
        'l' => [4, 4, 4, 4, 4, 4, 4],
        'm' => [0, 0, 26, 21, 21, 17, 17],
        'n' => [0, 0, 22, 25, 17, 17, 17],
        'o' => [0, 0, 14, 17, 17, 17, 14],
        'p' => [0, 0, 30, 17, 30, 16, 16],
        'q' => [0, 0, 13, 19, 15, 1, 1],
        'r' => [0, 0, 22, 25, 16, 16, 16],
        's' => [0, 0, 15, 16, 14, 1, 30],
        't' => [8, 8, 30, 8, 8, 9, 6],
        'u' => [0, 0, 17, 17, 17, 19, 13],
        'v' => [0, 0, 17, 17, 17, 10, 4],
        'w' => [0, 0, 17, 17, 21, 21, 10],
        'x' => [0, 0, 17, 10, 4, 10, 17],
        'y' => [0, 0, 17, 17, 15, 1, 14],
        'z' => [0, 0, 31, 2, 4, 8, 31],
        _ => [14, 17, 1, 2, 4, 0, 4],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn width_tracks_fixed_advance() {
        assert_eq!(width(""), 0);
        assert_eq!(width("A"), GLYPH_W);
        assert_eq!(width("AB"), GLYPH_W * 2 + 1);
    }

    #[test]
    fn wrap_breaks_at_spaces_and_hard_breaks_long_words() {
        // 10 chars/line at max_w=59 (59+1)/6 = 10.
        let s = "hello world again";
        let lines = wrap(s, 59);
        let texts: Vec<&str> = lines.iter().map(|r| &s[r.clone()]).collect();
        assert_eq!(texts, vec!["hello", "world", "again"]);

        let long = "abcdefghijklmno";
        let lines = wrap(long, 59);
        let texts: Vec<&str> = lines.iter().map(|r| &long[r.clone()]).collect();
        assert_eq!(texts, vec!["abcdefghij", "klmno"]);
    }

    #[test]
    fn measure_wrapped_height_uses_line_advance() {
        let (w, h) = measure("hello world", Some(59));
        assert_eq!(h, GLYPH_H + LINE_ADVANCE);
        assert_eq!(w, width("hello"));
        assert_eq!(measure("hi", None), (width("hi"), GLYPH_H));
        assert_eq!(measure("", Some(30)), (0, GLYPH_H));
    }

    #[test]
    fn atlas_covers_ascii_with_fallback() {
        let (rgba, (w, h)) = build_atlas();
        assert_eq!(rgba.len(), (w * h * 4) as usize);
        // 'A' cell has lit pixels exactly matching the table.
        let [ax, ay, ..] = atlas_rect('A');
        let lit = |col: u32, row: u32| {
            let i = (((ay + row) * w + ax + col) * 4) as usize;
            rgba[i + 3] == 255
        };
        for row in 0..GLYPH_H {
            for col in 0..GLYPH_W {
                assert_eq!(
                    lit(col as u32, row as u32),
                    glyph_cell('A', col, row),
                    "atlas('A') differs from table at {col},{row}"
                );
            }
        }
        // Unknown chars share the one fallback cell.
        assert_eq!(atlas_rect('🙂'), atlas_rect('\u{80}'));
        assert_ne!(atlas_rect('🙂'), atlas_rect('?'));
    }

    #[test]
    fn cpu_raster_uses_shared_metrics_and_clips() {
        let size = measure_scaled("A", 2);
        assert_eq!(size, [10, 14]);
        let mut rgba = vec![0; 10 * 14 * 4];
        draw_rgba(&mut rgba, 10, "A", [0, 0], 2, [12, 34, 56, 255]);
        assert!(rgba.chunks_exact(4).any(|pixel| pixel == [12, 34, 56, 255]));

        let mut clipped = vec![0; 4 * 4 * 4];
        draw_rgba(&mut clipped, 4, "A", [-3, -3], 2, [255; 4]);
        assert!(clipped.chunks_exact(4).any(|pixel| pixel[3] != 0));
    }
}
