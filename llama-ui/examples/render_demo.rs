//! Render a demo document to a PNG with the software rasterizer:
//! `cargo run -p llama-ui --features raster --example render_demo [out.png]`
//!
//! The demo exercises the whole widget catalog against the placeholder (or a
//! real) theme — the same DrawList the game uploads, so this PNG is what the
//! game shows.

use llama_ui::raster::TextureSet;
use llama_ui::{
    Document, FrameArgs, FrameOutput, FrameState, InputEvent, NoImages, Theme, UiMap, UiRuntime,
    UiState, UiValue,
};
use std::sync::Arc;

const DEMO: &str = r#"{
    "format": 1, "kind": "llama:demo", "class": "screen",
    "root": { "type": "frame", "style": "panel.large",
        "layout": { "pad": [10,10,10,10], "gap": 6 },
        "children": [
            { "type": "label", "text": "COMPONENT DEMO" },
            { "type": "row", "layout": { "gap": 6, "align": "center" }, "children": [
                { "type": "button", "id": "default", "text": "BUTTON" },
                { "type": "button", "id": "confirm", "text": "CONFIRM", "style": "button.success" },
                { "type": "button", "id": "delete", "text": "DELETE", "style": "button.danger" },
                { "type": "button", "id": "disabled", "text": "OFF", "bind": { "enabled": "never" } }
            ] },
            { "type": "row", "layout": { "gap": 8, "align": "center" }, "children": [
                { "type": "checkbox", "id": "c1", "bind": { "value": "on" } },
                { "type": "checkbox", "id": "c2", "bind": { "value": "off" } },
                { "type": "toggle", "id": "t1", "bind": { "value": "on" } },
                { "type": "toggle", "id": "t2", "bind": { "value": "off" } },
                { "type": "slider", "id": "vol", "min": 0.0, "max": 100.0,
                  "bind": { "value": "volume" }, "layout": { "w": 90 } },
                { "type": "badge", "text": "v0.1.0" }
            ] },
            { "type": "alert", "level": "info", "text": "This is an informational message." },
            { "type": "alert", "level": "danger", "text": "Something went wrong!" },
            { "type": "row", "layout": { "gap": 6 }, "children": [
                { "type": "text_input", "id": "name", "placeholder": "World name", "layout": { "w": 110 } },
                { "type": "gauge", "id": "arrow", "mode": "grow_lr", "style": "gauge.arrow",
                  "bind": { "value": "cook" }, "layout": { "w": 24, "h": 17 } },
                { "type": "gauge", "id": "flame", "mode": "deplete_td", "style": "gauge.flame",
                  "bind": { "value": "burn" }, "layout": { "w": 14, "h": 14 } }
            ] },
            { "type": "scroll", "id": "list_scroll", "layout": { "h": 62 }, "children": [
                { "type": "list", "id": "mods", "bind": { "items": "mods", "selected": "mod_sel" },
                  "children": [
                    { "type": "row", "style": "list.row", "layout": { "pad": [4,4,4,4], "gap": 6, "align": "center" },
                      "children": [
                        { "type": "label", "bind": { "text": "name" } },
                        { "type": "badge", "bind": { "text": "version" } },
                        { "type": "spacer", "layout": { "w": { "grow": 1 } } },
                        { "type": "toggle", "id": "mod_on", "bind": { "value": "enabled" } }
                    ] }
                ] }
            ] },
            { "type": "slot_grid", "role": "hotbar", "cols": 9, "rows": 1 }
        ] }
}"#;

fn main() {
    let out_path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "render_demo.png".to_owned());
    let doc = Arc::new(Document::from_json(DEMO).expect("demo doc parses"));
    // LLAMA_UI_THEME_DIR previews a real theme kit; unset = placeholder.
    let theme = Arc::new(match std::env::var("LLAMA_UI_THEME_DIR") {
        Ok(dir) => {
            let dir = std::path::PathBuf::from(dir);
            let json = std::fs::read_to_string(dir.join("theme.json")).expect("read theme.json");
            Theme::load(&json, &|rel| std::fs::read(dir.join(rel)).ok()).expect("theme loads")
        }
        Err(_) => Theme::placeholder(),
    });
    let issues = doc.validate(Some(theme.as_ref()), None);
    assert!(issues.is_empty(), "demo doc validates: {issues:?}");

    let mut state = UiState::new();
    state.set("on", UiValue::Bool(true));
    state.set("off", UiValue::Bool(false));
    state.set("never", UiValue::Bool(false));
    state.set("volume", UiValue::F32(75.0));
    state.set("cook", UiValue::F32(0.6));
    state.set("burn", UiValue::F32(0.4));
    state.set("mod_sel", UiValue::I32(1));
    let mods: Vec<UiMap> = [
        ("Smoke Test", "v0.1.0", true),
        ("Zombies", "v0.1.0", false),
        ("Wheel", "v0.2.3", true),
    ]
    .iter()
    .map(|(n, v, e)| {
        let mut m = UiMap::new();
        m.insert("name".into(), UiValue::Str((*n).into()));
        m.insert("version".into(), UiValue::Str((*v).into()));
        m.insert("enabled".into(), UiValue::Bool(*e));
        m
    })
    .collect();
    state.set("mods", UiValue::List(Arc::new(mods)));

    let rt = UiRuntime::new(doc, theme.clone());
    let mut fs = FrameState::new();
    let mut out = FrameOutput::default();
    let screen = (720, 560);
    // Park the cursor over the CONFIRM button so a hover state renders.
    let input = [InputEvent::PointerMove { x: 260.0, y: 74.0 }];
    rt.frame(
        FrameArgs {
            screen,
            scale: 2,
            now: 0.0,
            state: &state,
            input: &input,
            clipboard: None,
            images: &NoImages,
            dim: Some([0.0, 0.0, 0.0, 0.55]),
            preview: None,
        },
        &mut fs,
        &mut out,
    );

    let tex = TextureSet {
        theme_atlas: &theme.atlas,
        font: &theme.font,
        doc_images: &[],
    };
    let mut rgba = Vec::new();
    llama_ui::raster::rasterize(&out.draw, &tex, screen, [28, 34, 40, 255], &mut rgba);
    image::save_buffer(
        &out_path,
        &rgba,
        screen.0,
        screen.1,
        image::ColorType::Rgba8,
    )
    .expect("write png");
    println!(
        "wrote {out_path} ({}x{}, {} vertices, {} batches, {} slots)",
        screen.0,
        screen.1,
        out.draw.vertices.len(),
        out.draw.batches.len(),
        out.slots.len()
    );
}
