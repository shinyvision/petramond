use super::*;
use crate::doc::Document;
use crate::layout::LayoutEnv;

/// The palette every manifest must define, as JSON.
const PALETTE: &str = r##""palette": { "text": "#E8EDF2", "text_muted": "#9AA7B4",
    "text_disabled": "#5E6B78", "selection": "#3E6FD9" }"##;

#[test]
fn placeholder_theme_has_every_default_part() {
    let t = Theme::placeholder();
    for key in [
        "panel.large",
        "button.default",
        "button.danger",
        "checkbox",
        "toggle",
        "slot",
        "scrollbar.thumb",
        "slider.track",
        "slider.handle",
        "list.row",
        "input",
        "badge",
        "alert.info",
    ] {
        assert!(t.part(key).is_some(), "missing part '{key}'");
    }
    assert!(t.part("checkbox").unwrap().face(FaceState::On).is_some());
    assert!(
        t.part("checkbox").unwrap().face(FaceState::Focus).is_some(),
        "states a part lacks fall back to a face"
    );
    let (aw, ah) = t.pages()[0].size;
    assert_eq!(t.pages()[0].rgba.len(), (aw * ah * 4) as usize);
}

#[test]
fn only_compound_buttons_use_their_face_as_container_insets() {
    let t = Theme::placeholder();
    let leaf = Document::from_json(
        r#"{
        "format": 1, "kind": "petramond:x", "class": "screen",
        "root": { "type": "button", "id": "leaf", "text": "OK" }
    }"#,
    )
    .unwrap();
    assert_eq!(t.container_insets(&leaf.root), [0; 4]);

    let compound = Document::from_json(
        r#"{
        "format": 1, "kind": "petramond:x", "class": "screen",
        "root": { "type": "button", "id": "compound", "children": [
            { "type": "label", "text": "OK" }
        ] }
    }"#,
    )
    .unwrap();
    let authored = t
        .part("button.default")
        .and_then(|part| part.face(FaceState::Default))
        .and_then(|face| face.slice)
        .expect("placeholder compound button has sliced chrome");
    assert_eq!(t.container_insets(&compound.root), authored);
}

fn tiny_png() -> Vec<u8> {
    let img = image::RgbaImage::new(4, 4);
    let mut bytes = std::io::Cursor::new(Vec::new());
    img.write_to(&mut bytes, image::ImageFormat::Png).unwrap();
    bytes.into_inner()
}

fn shipped_font_bytes() -> Vec<u8> {
    std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../assets/ui/font/DepartureMono-Regular.otf"
    ))
    .expect("shipped font is vendored")
}

/// A theme whose font is the shipped face, loaded like the game does.
fn shipped_font_theme() -> Theme {
    let json = format!(
        r#"{{ "format": 1, "atlas": "kit.png", {PALETTE},
        "font": {{ "file": "ui.otf", "px": 11 }},
        "parts": {{ "panel.large": {{ "rect": [0, 0, 4, 4] }} }} }}"#
    );
    let (png, font) = (tiny_png(), shipped_font_bytes());
    Theme::load(&json, &|p| match p {
        "kit.png" => Some(png.clone()),
        "ui.otf" => Some(font.clone()),
        _ => None,
    })
    .expect("theme with the shipped font loads")
}

/// Measurement belongs to the theme, not the process: two themes with
/// different fonts coexist, and each one's tab widths (the hit-test
/// geometry) follow the font that same theme paints with.
#[test]
fn two_themes_measure_with_their_own_fonts() {
    let real = shipped_font_theme();
    let placeholder = Theme::placeholder();
    let tabs = [crate::doc::TabSpec {
        key: "world".into(),
        icon: None,
        label: Some("World".into()),
    }];
    let pad = |t: &Theme| t.metrics.button_pad * 2;
    let real_w = crate::widget::tab_widths(&real, &tabs)[0];
    let placeholder_w = crate::widget::tab_widths(&placeholder, &tabs)[0];
    assert_eq!(real_w, real.ui_font().width("World") + pad(&real));
    assert_eq!(
        placeholder_w,
        placeholder.ui_font().width("World") + pad(&placeholder)
    );
    assert_ne!(
        real.ui_font().width("World"),
        placeholder.ui_font().width("World"),
        "the two fonts genuinely differ"
    );
}

/// Coverage is manifest data: a primary limited to ASCII plus a fallback
/// face for the accents yields one font covering both.
#[test]
fn theme_font_ranges_and_fallback_faces_come_from_the_manifest() {
    let json = format!(
        r#"{{ "format": 1, "atlas": "kit.png", {PALETTE},
        "font": {{ "file": "ui.otf", "px": 11, "ranges": [[32, 126]],
                  "fallback": [ {{ "file": "ui.otf", "px": 11, "ranges": [[192, 255]] }} ] }},
        "parts": {{ "panel.large": {{ "rect": [0, 0, 4, 4] }} }} }}"#
    );
    let (png, font) = (tiny_png(), shipped_font_bytes());
    let t = Theme::load(&json, &|p| match p {
        "kit.png" => Some(png.clone()),
        "ui.otf" => Some(font.clone()),
        _ => None,
    })
    .expect("theme with a fallback face loads");
    let f = t.ui_font();
    assert!(f.has_glyph('A') && f.has_glyph('\u{c4}'));
    assert!(!f.has_glyph('\u{3a9}'), "outside every declared range");
    assert_eq!(t.font_atlas().size, f.atlas_size());
}

#[test]
fn theme_json_parses_shorthand_and_state_parts() {
    let json = r##"{
        "format": 1,
        "palette": { "text": "#E8EDF2", "accent": "#57C95680", "text_muted": "#9AA7B4",
                     "text_disabled": "#5E6B78", "selection": "#3E6FD9" },
        "atlas": "kit.png",
        "parts": {
            "panel.large": { "rect": [0,0,64,64], "slice": [8,8,8,8] },
            "button.default": {
                "states": {
                    "default": { "rect": [0,64,32,20], "slice": [4,4,4,4] },
                    "hover":   { "rect": [32,64,32,20], "slice": [4,4,4,4] }
                },
                "label_color": "text",
                "pressed_label_offset": [0, 1]
            }
        },
        "metrics": { "slot": 20 }
    }"##;
    // 1x1 transparent png.
    let png = {
        let img = image::RgbaImage::new(4, 4);
        let mut bytes = std::io::Cursor::new(Vec::new());
        img.write_to(&mut bytes, image::ImageFormat::Png).unwrap();
        bytes.into_inner()
    };
    let t = Theme::load(json, &|p| (p == "kit.png").then(|| png.clone())).unwrap();
    assert_eq!(t.part("panel.large").unwrap().natural(), (64, 64));
    let b = t.part("button.default").unwrap();
    assert_eq!(b.face(FaceState::Hover).unwrap().rect, [32, 64, 32, 20]);
    assert_eq!(
        b.face(FaceState::Pressed).unwrap().rect,
        [0, 64, 32, 20],
        "fallback to default"
    );
    assert_eq!(b.pressed_label_offset, [0, 1]);
    assert_eq!(t.metrics.slot, 20);
    assert_eq!(t.color("accent")[3], 128.0 / 255.0);
    assert_eq!(t.color("#FF0000"), [1.0, 0.0, 0.0, 1.0]);
    assert_eq!(
        t.color("nope"),
        [1.0, 0.0, 1.0, 1.0],
        "missing key is loud magenta"
    );
    // Font defaults to the builtin table, and the uploaded atlas is the
    // one built from the font that measures.
    assert_eq!(t.font_atlas().size, t.ui_font().atlas_size());
}

#[test]
fn alerts_wrap_when_width_constrained() {
    let t = Theme::placeholder();
    let env = ThemeEnv {
        theme: &t,
        gui_scale: 1,
        image_size: &|_| None,
    };
    let doc = Document::from_json(
        r#"{
        "format": 1, "kind": "petramond:x", "class": "screen",
        "root": { "type": "column", "children": [
            { "type": "alert", "level": "info", "text": "A rather long warning message" }
        ] }
    }"#,
    )
    .unwrap();
    let alert = &doc.root.children[0];
    let text = Some("A rather long warning message");
    let (w_free, h_free) = env.leaf_size(alert, text, None, None);
    let (w_tight, h_tight) = env.leaf_size(alert, text, None, Some(100));
    assert!(
        w_tight <= 100,
        "constrained alert fits its width: {w_tight}"
    );
    assert!(w_tight < w_free);
    assert!(
        h_tight > h_free,
        "wrapped alert grows taller instead of overflowing"
    );
}

#[test]
fn theme_env_supplies_widget_naturals() {
    let t = Theme::placeholder();
    let env = ThemeEnv {
        theme: &t,
        gui_scale: 1,
        image_size: &|name| (name == "wheel.png").then_some((32, 32)),
    };
    let doc = Document::from_json(
        r#"{
        "format": 1, "kind": "petramond:x", "class": "screen",
        "root": { "type": "column", "children": [
            { "type": "button", "id": "b", "text": "OK" },
            { "type": "checkbox", "id": "c" },
            { "type": "slot_grid", "role": "hotbar", "cols": 9, "rows": 1 },
            { "type": "image", "image": "wheel.png" },
            { "type": "image", "image": "missing.png" }
        ] }
    }"#,
    )
    .unwrap();
    let n = &doc.root.children;
    assert_eq!(
        env.leaf_size(&n[0], Some("OK"), None, None),
        (t.ui_font().width("OK") + 12, 20)
    );
    assert_eq!(env.leaf_size(&n[1], None, None, None), (10, 10));
    assert_eq!(env.leaf_size(&n[2], None, None, None), (18 * 9, 18));
    assert_eq!(
        env.leaf_size(&n[3], None, Some("wheel.png"), None),
        (32, 32)
    );
    assert_eq!(
        env.leaf_size(&n[4], None, Some("missing.png"), None),
        (0, 0)
    );
}

fn png_of(w: u32, h: u32) -> Vec<u8> {
    let img = image::RgbaImage::new(w, h);
    let mut bytes = std::io::Cursor::new(Vec::new());
    img.write_to(&mut bytes, image::ImageFormat::Png).unwrap();
    bytes.into_inner()
}

/// A pack overlay adds and replaces parts by key on its own atlas page,
/// extends the palette and overrides single metrics, while the base keeps
/// everything the overlay leaves alone.
#[test]
fn an_overlay_layer_composes_over_the_base_kit() {
    let base = format!(
        r#"{{ "format": 1, "atlas": "kit.png", {PALETTE},
        "parts": {{
            "panel.large": {{ "rect": [0, 0, 8, 8] }},
            "button.default": {{ "states": {{ "default": {{ "rect": [0, 8, 8, 8] }} }} }}
        }},
        "metrics": {{ "slot": 20, "button_h": 22 }} }}"#
    );
    let overlay = r##"{ "format": 1, "atlas": "mod.png",
        "palette": { "rust": "#AA4400" },
        "parts": {
            "button.default": { "rect": [1, 1, 6, 6] },
            "mymod.gear": { "rect": [0, 0, 16, 16], "label_color": "rust" }
        },
        "metrics": { "slot": 24 } }"##;
    let (kit, modpng) = (png_of(16, 16), png_of(32, 32));
    let read_base = |p: &str| (p == "kit.png").then(|| kit.clone());
    let read_mod = |p: &str| (p == "mod.png").then(|| modpng.clone());
    let t = Theme::load_stack(&[
        ThemeLayer {
            json: &base,
            read: &read_base,
        },
        ThemeLayer {
            json: overlay,
            read: &read_mod,
        },
    ])
    .expect("stack loads");
    assert_eq!(t.pages().len(), 2);
    assert_eq!(t.page_size(1), (32, 32));
    let panel = t
        .part("panel.large")
        .unwrap()
        .face(FaceState::Default)
        .unwrap();
    assert_eq!(panel.page, 0, "untouched base part stays on the base page");
    let button = t
        .part("button.default")
        .unwrap()
        .face(FaceState::Default)
        .unwrap();
    assert_eq!(
        (button.page, button.rect),
        (1, [1, 1, 6, 6]),
        "overlay replaces by key"
    );
    assert_eq!(
        t.part("mymod.gear")
            .unwrap()
            .face(FaceState::Default)
            .unwrap()
            .page,
        1
    );
    assert!(t.has_color("rust") && t.has_color("text"));
    assert_eq!(
        (t.metrics.slot, t.metrics.button_h),
        (24, 22),
        "metrics merge by key"
    );
}

/// Typos in a manifest are errors that name the offending key, not silent
/// no-ops: an unknown field, an unknown face state, a missing required
/// palette entry and a label colour that resolves to nothing.
#[test]
fn manifest_mistakes_are_load_errors() {
    let png = tiny_png();
    let read = |p: &str| (p == "kit.png").then(|| png.clone());
    let load = |parts: &str, extra: &str| {
        let json = format!(
            r#"{{ "format": 1, "atlas": "kit.png", {PALETTE}, "parts": {{ {parts} }} {extra} }}"#
        );
        Theme::load(&json, &read).map(|_| ()).map_err(|e| e.0)
    };
    assert_eq!(load(r#""p": { "rect": [0,0,1,1] }"#, ""), Ok(()));
    let err = load(r#""p": { "rect": [0,0,1,1], "slcie": [1,1,1,1] }"#, "").unwrap_err();
    assert!(err.contains("slcie"), "{err}");
    let err = load(
        r#""p": { "states": { "hovered": { "rect": [0,0,1,1] } } }"#,
        "",
    )
    .unwrap_err();
    assert!(err.contains("unknown face state 'hovered'"), "{err}");
    let err = load(r#""p": { "rect": [0,0,1,1], "label_color": "txet" }"#, "").unwrap_err();
    assert!(err.contains("label_color 'txet'"), "{err}");
    let err = load(
        r#""p": { "rect": [0,0,1,1] }"#,
        r#", "metrics": { "slto": 3 }"#,
    )
    .unwrap_err();
    assert!(err.contains("slto"), "{err}");
    let no_selection = r##"{ "format": 1, "palette": { "text": "#FFFFFF" } }"##;
    let err = Theme::load(no_selection, &read).map(|_| ()).unwrap_err().0;
    assert!(err.contains("text_muted"), "{err}");
}

#[test]
fn face_state_names_round_trip() {
    for state in FaceState::ALL {
        assert_eq!(FaceState::parse(state.name()), Some(state));
    }
    assert_eq!(FaceState::parse("bogus"), None);
}
