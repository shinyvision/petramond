use crate::input::FrameState;
use crate::paint_walk::NoImages;
use crate::runtime::{CacheStats, FrameArgs, FrameOutput, UiRuntime};
use crate::state::{UiMap, UiState, UiValue};
use crate::theme::Theme;
use crate::Document;
use std::sync::Arc;

fn doc() -> Arc<Document> {
    Arc::new(
        Document::from_json(
            r#"{
            "format": 1, "kind": "petramond:test_screen", "class": "screen",
            "root": { "type": "column", "layout": { "pad": [4, 4, 4, 4], "gap": 2 }, "children": [
                { "type": "label", "id": "status", "wrap": true, "layout": { "w": 80 },
                  "bind": { "text": "status" } },
                { "type": "scroll", "id": "sc", "layout": { "h": 40 }, "children": [
                    { "type": "list", "id": "rows", "bind": { "items": "rows" }, "children": [
                        { "type": "label", "bind": { "text": "name" }, "layout": { "h": 12 } }
                    ] }
                ] },
                { "type": "button", "id": "ok", "text": "OK" }
            ] }
        }"#,
        )
        .unwrap(),
    )
}

fn state() -> UiState {
    let mut s = UiState::new();
    s.set("status", UiValue::Str("a long status line that wraps".into()));
    let rows: Vec<UiMap> = (0..8)
        .map(|i| {
            let mut m = UiMap::new();
            m.insert("name".into(), UiValue::Str(format!("Row {i}")));
            m
        })
        .collect();
    s.set("rows", UiValue::List(Arc::new(rows)));
    s
}

fn frame(
    rt: &UiRuntime,
    state: &UiState,
    fs: &mut FrameState,
    out: &mut FrameOutput,
) -> CacheStats {
    rt.frame(
        FrameArgs {
            screen: (320, 240),
            scale: 1,
            now: 0.0,
            state,
            input: &[],
            clipboard: None,
            images: &NoImages,
            dim: None,
            preview: None,
        },
        fs,
        out,
    );
    fs.cache_stats()
}

/// What a frame hands the host, for comparing a cached frame to a fresh one.
fn visible(out: &FrameOutput) -> (Vec<[f32; 2]>, Vec<(String, crate::RectI)>) {
    (
        out.draw.vertices.iter().map(|v| v.pos).collect(),
        out.named.iter().map(|(k, r)| (k.id.clone(), *r)).collect(),
    )
}

#[test]
fn an_unchanged_frame_reuses_the_arena_and_the_layout() {
    let rt = UiRuntime::new(doc(), Arc::new(Theme::placeholder()));
    let state = state();
    let (mut fs, mut out) = (FrameState::new(), FrameOutput::default());
    let first = frame(&rt, &state, &mut fs, &mut out);
    assert_eq!(first.reused, 0);
    assert!(!first.layout_reused);
    let drawn = visible(&out);

    let second = frame(&rt, &state, &mut fs, &mut out);
    assert_eq!(second.expanded, 0, "{second:?}");
    assert_eq!(second.reused, first.expanded);
    assert!(second.layout_reused);
    assert_eq!(visible(&out), drawn, "a cached frame draws what a fresh one did");
}

#[test]
fn a_changed_key_re_expands_only_its_readers_and_matches_a_fresh_frame() {
    let rt = UiRuntime::new(doc(), Arc::new(Theme::placeholder()));
    let mut state = state();
    let (mut fs, mut out) = (FrameState::new(), FrameOutput::default());
    let first = frame(&rt, &state, &mut fs, &mut out);

    state.set("status", UiValue::Str("short".into()));
    let second = frame(&rt, &state, &mut fs, &mut out);
    assert!(!second.layout_reused, "a changed label re-solves");
    // The root re-homes its children and the status label re-resolves; the
    // scroll, the list with its eight rows, and the button move over.
    assert_eq!(second.expanded, 2, "{second:?}");
    assert_eq!(second.reused, first.expanded - 2);

    let (mut fresh_fs, mut fresh_out) = (FrameState::new(), FrameOutput::default());
    frame(&rt, &state, &mut fresh_fs, &mut fresh_out);
    assert_eq!(visible(&out), visible(&fresh_out));
}

#[test]
fn a_new_theme_or_state_origin_starts_over() {
    let doc = doc();
    let state = state();
    let (mut fs, mut out) = (FrameState::new(), FrameOutput::default());
    let rt = UiRuntime::new(doc.clone(), Arc::new(Theme::placeholder()));
    frame(&rt, &state, &mut fs, &mut out);

    // Same document, new theme: the arena survives, the layout does not.
    let rethemed = UiRuntime::new(doc, Arc::new(Theme::placeholder()));
    let stats = frame(&rethemed, &state, &mut fs, &mut out);
    assert_eq!(stats.expanded, 0);
    assert!(!stats.layout_reused);

    // A clone is a different origin: nothing carries over.
    let stats = frame(&rethemed, &state.clone(), &mut fs, &mut out);
    assert_eq!(stats.reused, 0);
}
