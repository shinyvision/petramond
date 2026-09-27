pub mod builtin;
mod font;
pub mod tiny;

pub use font::{FaceSource, Font, FontError, Glyph};

impl Font {
    pub fn measure_scaled(&self, s: &str, scale: u8) -> [u32; 2] {
        let scale = scale.max(1) as u32;
        [
            self.width(s).max(0) as u32 * scale,
            self.line_h() as u32 * scale,
        ]
    }

    pub fn draw_rgba(
        &self,
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
        let height = rgba.len() / (width as usize * 4);
        let scale = scale.max(1) as i32;
        let mut pen_x = position[0];
        for ch in text.chars() {
            let glyph = self.glyph(ch);
            for (x, y) in glyph.ink() {
                let (left, top) = (pen_x + x * scale, position[1] + y * scale);
                for dy in 0..scale {
                    for dx in 0..scale {
                        let (px, py) = (left + dx, top + dy);
                        blend_rgba_pixel(rgba, width as usize, height, px, py, color);
                    }
                }
            }
            pen_x += glyph.advance() * scale;
        }
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
        let builtin = Font::builtin();
        assert!(font.line_h() > builtin.line_h());
    }

    #[test]
    fn glyph_bitmaps_match_the_fonts_own_pixel_grid() {
        let bytes = std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../assets/ui/font/DepartureMono-Regular.otf"
        ))
        .expect("shipped font is vendored");
        let font = Font::from_ttf(&bytes, 11.0).expect("shipped font rasterizes");
        let expect: &[(char, &[&str])] = &[
            ('w', &["#.#.#", "#.#.#", "#.#.#", "#.#.#", "#.#.#", ".#.##"]),
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
        assert_eq!(font.advance('W'), 7);
    }

    fn trimmed_glyph(font: &Font, ch: char) -> Vec<String> {
        let glyph = font.glyph(ch);
        let [_, _, w, h] = glyph.bounds();
        (0..h)
            .map(|y| {
                (0..w)
                    .map(|x| if glyph.lit(x, y) { '#' } else { '.' })
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
    fn a_drawn_run_stays_inside_its_measured_box() {
        let font = Font::builtin();
        let [w, h] = font.measure_scaled("Hi!", 2);
        assert_eq!(
            [w, h],
            [font.width("Hi!") as u32 * 2, font.line_h() as u32 * 2]
        );
        let (iw, ih) = (w + 4, h + 4);
        let mut rgba = vec![0u8; (iw * ih * 4) as usize];
        font.draw_rgba(&mut rgba, iw, "Hi!", [2, 2], 2, [255, 255, 255, 255]);
        let mut lit = 0;
        for y in 0..ih {
            for x in 0..iw {
                if rgba[((y * iw + x) * 4 + 3) as usize] == 0 {
                    continue;
                }
                lit += 1;
                assert!(
                    (2..2 + w).contains(&x) && (2..2 + h).contains(&y),
                    "{x},{y}"
                );
            }
        }
        assert!(lit > 0, "the run drew something");
    }
}
