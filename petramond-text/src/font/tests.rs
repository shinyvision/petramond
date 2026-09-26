use super::*;

fn shipped_font_bytes() -> Vec<u8> {
    std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../assets/ui/font/DepartureMono-Regular.otf"
    ))
    .expect("shipped font is vendored")
}

/// The line-relative rows `(first, one past last)` holding `ch`'s ink.
fn ink_rows(font: &Font, ch: char) -> (i32, i32) {
    let [_, dy, _, h] = font.glyph(ch).bounds();
    (dy, dy + h)
}

#[test]
fn builtin_font_is_fixed_pitch_ascii_with_a_visible_fallback() {
    let f = Font::builtin();
    assert_eq!(f.width("AB"), f.advance('A') * 2);
    assert!(f.has_glyph('A') && f.has_glyph('~'));
    assert!(!f.has_glyph('\u{d7}'), "the built-in table is ASCII only");
    // Unknown codepoints share one glyph and still draw something.
    assert_eq!(f.atlas_rect('\u{d7}'), f.atlas_rect('🙂'));
    assert!(f.glyph('🙂').ink().next().is_some());
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

/// A caret sized to the whole line box fills the input box, because the box
/// reserves headroom for accented capitals that ordinary text leaves empty.
/// The body span is what a caret or selection should cover.
#[test]
fn the_body_span_is_the_text_band_not_the_whole_line() {
    let font = Font::from_ttf(&shipped_font_bytes(), 11.0).expect("shipped font rasterizes");
    let (top, h) = font.body_span();
    assert!(top > 0, "accented capitals sit above the body");
    assert!(
        h < font.line_h(),
        "body {h} should be shorter than the line {}",
        font.line_h()
    );
    assert!(top + h <= font.line_h(), "the body stays inside the line");

    // It spans exactly cap-top to descender-bottom.
    assert_eq!(top, ink_rows(&font, 'M').0, "body starts at the cap line");
    assert_eq!(
        top + h,
        ink_rows(&font, 'g').1,
        "body ends at the descender"
    );
    assert!(
        ink_rows(&font, '\u{c4}').0 < top,
        "the accent is above the body"
    );

    // The built-in table has no accents or descenders: body IS the line.
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
fn atlas_pixels_match_every_glyph_bitmap() {
    for f in [
        Font::builtin(),
        Font::from_ttf(&shipped_font_bytes(), 11.0).unwrap(),
    ] {
        for ch in ['A', 'g', '\u{c4}', '🙂'] {
            let _ = f.glyph(ch);
        }
        let (rgba, (w, h)) = f.build_atlas();
        assert_eq!(rgba.len(), (w * h * 4) as usize);
        for ch in ['A', 'g', '\u{c4}', '🙂'] {
            let glyph = f.glyph(ch);
            let [ax, ay, gw, gh] = glyph.atlas_rect();
            assert!(ax + gw <= w && ay + gh <= h, "{ch:?} leaves the atlas");
            for y in 0..gh as i32 {
                for x in 0..gw as i32 {
                    let i = (((ay + y as u32) * w + ax + x as u32) * 4) as usize;
                    assert_eq!(rgba[i + 3] == 255, glyph.lit(x, y), "{ch:?} at {x},{y}");
                }
            }
        }
    }
}

/// Coverage is whatever the face maps, not a hard-coded Latin list: every
/// codepoint the shipped face carries beyond Latin resolves to a real glyph.
#[test]
fn coverage_comes_from_the_face_not_a_latin_list() {
    let bytes = shipped_font_bytes();
    let font = Font::from_ttf(&bytes, 11.0).unwrap();
    // Everything the old hard-coded Latin table held is still there...
    for ch in "AZaz09 ×·—…'\"Ööäëéèñçßæø".chars() {
        assert!(font.has_glyph(ch), "missing glyph {ch:?}");
    }
    // ...and codepoints outside it resolve whenever the face maps them.
    let face = ab_glyph::FontRef::try_from_slice(&bytes).unwrap();
    let mapped_beyond_latin: Vec<char> = {
        use ab_glyph::Font as _;
        face.codepoint_ids()
            .map(|(_, ch)| ch)
            .filter(|ch| (*ch as u32) >= 0x250 && !ch.is_control())
            .collect()
    };
    assert!(
        !mapped_beyond_latin.is_empty(),
        "the face maps non-Latin codepoints"
    );
    for ch in mapped_beyond_latin {
        assert!(font.has_glyph(ch), "{ch:?} is in the face but not the font");
    }
}

/// Declared ranges limit a face to exactly those codepoints (a pack can keep
/// a huge face's atlas small).
#[test]
fn declared_ranges_limit_what_a_face_contributes() {
    let bytes = shipped_font_bytes();
    let font = Font::from_faces(&[FaceSource {
        bytes: &bytes,
        px: 11.0,
        ranges: &[[0x41, 0x5A]],
    }])
    .unwrap();
    assert!(font.has_glyph('Q'));
    // Outside the range the chain falls through to the built-in table...
    let builtin = Font::builtin();
    assert_eq!(font.advance('q'), builtin.advance('q'));
    // ...and past it to the fallback box.
    assert!(!font.has_glyph('\u{c4}'));
}

/// A later face fills only what earlier faces lack, positioned on the
/// primary's baseline; the primary alone sets the line box.
#[test]
fn a_fallback_face_fills_gaps_without_changing_the_line_box() {
    let bytes = shipped_font_bytes();
    let latin_only = [[0x20, 0x7E]];
    let accents = [[0xC0, 0xFF]];
    let primary = FaceSource {
        bytes: &bytes,
        px: 11.0,
        ranges: &latin_only,
    };
    let alone = Font::from_faces(&[primary]).unwrap();
    // The same face at twice the size stands in for a taller fallback script.
    let chain = Font::from_faces(&[
        primary,
        FaceSource {
            bytes: &bytes,
            px: 22.0,
            ranges: &accents,
        },
    ])
    .unwrap();
    assert!(!alone.has_glyph('\u{c4}') && chain.has_glyph('\u{c4}'));
    assert_eq!(
        chain.line_h(),
        alone.line_h(),
        "the fallback never grows the line"
    );
    assert_eq!(
        chain.advance('A'),
        alone.advance('A'),
        "the primary wins its codepoints"
    );
    // The big fallback glyph keeps its own size instead of every glyph
    // growing to it.
    assert!(chain.glyph('\u{c4}').bounds()[3] > chain.glyph('A').bounds()[3] * 3 / 2);
    assert_eq!(chain.glyph('A').bounds(), alone.glyph('A').bounds());
}

/// Glyph bitmaps are not limited to 32 px (the old bitmask rows): a large
/// size rasterizes instead of erroring.
#[test]
fn large_sizes_rasterize() {
    let bytes = shipped_font_bytes();
    let font = Font::from_faces(&[FaceSource {
        bytes: &bytes,
        px: 88.0,
        ranges: &[[0x57, 0x57]],
    }])
    .expect("a big size rasterizes");
    assert!(font.glyph('W').bounds()[2] > 32);
}

/// Wraps are memoised per (width, text), and a hit is indistinguishable
/// from shaping afresh — measure and paint read the same lines.
#[test]
fn memoised_wraps_match_fresh_shaping() {
    let f = Font::builtin();
    let s = "the quick brown fox jumps over the lazy dog";
    for w in [1, 20, f.width("the quick"), 400] {
        let fresh = f.wrap_uncached(s, w);
        assert_eq!(f.wrap(s, w), fresh, "first call at {w}");
        assert_eq!(f.wrap(s, w), fresh, "memoised call at {w}");
        let (mw, mh) = f.measure(s, Some(w));
        let widest = fresh.iter().map(|r| f.width(&s[r.clone()])).max().unwrap();
        assert_eq!(mw, widest);
        assert_eq!(mh, f.line_h() + (fresh.len() as i32 - 1) * f.line_advance());
    }
    assert_eq!(f.wraps.lock().unwrap().entries, 4);
}

/// With no U+FFFD anywhere in the chain, unknown codepoints draw a box over
/// the text body — never a blank, never a 5×7 speck.
#[test]
fn a_missing_glyph_draws_a_body_sized_box() {
    let font = Font::from_ttf(&shipped_font_bytes(), 11.0).unwrap();
    if font.has_glyph('\u{FFFD}') || font.has_glyph('\u{E000}') {
        return; // the face brings its own replacement (or maps the probe)
    }
    let unknown = font.glyph('\u{E000}');
    let (top, h) = font.body_span();
    assert_eq!(unknown.bounds()[1], top);
    assert_eq!(unknown.bounds()[3], h);
    assert!(unknown.ink().next().is_some());
}

/// Measurement reads eager advances and leaves glyph bitmaps untouched;
/// painting one new glyph adds it to the fixed atlas exactly once.
#[test]
fn glyph_bitmaps_are_lazy_but_advances_are_available() {
    let font = Font::from_ttf(&shipped_font_bytes(), 11.0).unwrap();
    let size = font.atlas_size();
    let before = font.atlas_revision();
    assert!(font.width("Q") > 0);
    assert_eq!(font.atlas_revision(), before);
    assert_eq!(font.atlas_size(), size);
    let _ = font.glyph('Q');
    assert!(font.atlas_revision() > before);
    let after = font.atlas_revision();
    let _ = font.glyph('Q');
    assert_eq!(font.atlas_revision(), after);
    assert_eq!(font.atlas_size(), size);
}
