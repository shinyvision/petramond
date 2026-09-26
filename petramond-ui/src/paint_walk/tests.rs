use super::*;
use crate::doc::Document;
use crate::input::{InputEvent, PointerButton};
use crate::paint::Batch;
use crate::runtime::{FrameArgs, FrameOutput, UiRuntime};
use crate::state::{UiState, UiValue};
use crate::theme::Theme;
use std::sync::Arc;

#[test]
fn bound_frame_is_authoritative_truncates_and_clamps() {
    // 3x2 sheet = 6 frames.
    let g = Some([3, 2]);
    assert_eq!(frame_index(g, Some(2), None, 0.0), 2);
    assert_eq!(
        frame_index(g, Some(0), Some(9.0), 100.0),
        0,
        "bound beats fps"
    );
    // Truncation happens when the binding resolves (f32 -> i32), so the
    // walk only ever sees integers; clamping happens here.
    assert_eq!(frame_index(g, Some(99), None, 0.0), 5, "clamps into sheet");
    assert_eq!(
        frame_index(g, Some(-3), None, 0.0),
        0,
        "negative clamps to 0"
    );
    assert_eq!(frame_index(None, Some(4), None, 0.0), 0, "unframed sheet");
}

#[test]
fn fps_cycles_row_major_and_invalid_rates_rest_on_frame_zero() {
    let g = Some([4, 1]);
    assert_eq!(frame_index(g, None, Some(4.0), 0.0), 0);
    assert_eq!(frame_index(g, None, Some(4.0), 0.3), 1, "floor(1.2)");
    assert_eq!(frame_index(g, None, Some(4.0), 0.6), 2, "floor(2.4)");
    assert_eq!(frame_index(g, None, Some(4.0), 1.1), 0, "floor(4.4) wraps");
    assert_eq!(frame_index(g, None, Some(0.0), 10.0), 0);
    assert_eq!(frame_index(g, None, Some(-1.0), 10.0), 0);
    assert_eq!(frame_index(g, None, None, 10.0), 0);
}

#[test]
fn frame_src_is_row_major_one_cell() {
    // 64x64 sheet, 2x2 grid: frame 2 = row 1, col 0.
    assert_eq!(
        frame_src((64, 64), Some([2, 2]), Some(2), None, 0.0),
        [0, 32, 32, 32]
    );
    assert_eq!(
        frame_src((64, 64), Some([2, 2]), Some(3), None, 0.0),
        [32, 32, 32, 32]
    );
    assert_eq!(frame_src((64, 64), None, None, None, 0.0), [0, 0, 64, 64]);
}

// ---- runtime-driven behavior -------------------------------------------------

struct Sheets(&'static [(&'static str, u16, (u32, u32))]);

impl DocImages for Sheets {
    fn resolve(&self, name: &str) -> Option<(u16, (u32, u32))> {
        self.0
            .iter()
            .find(|(n, _, _)| *n == name)
            .map(|(_, t, s)| (*t, *s))
    }
}

fn paint(doc_json: &str, state: &UiState, images: &dyn DocImages, now: f64) -> FrameOutput {
    let rt = UiRuntime::new(
        Arc::new(Document::from_json(doc_json).unwrap()),
        Arc::new(Theme::placeholder()),
    );
    let mut fs = crate::input::FrameState::new();
    let mut out = FrameOutput::default();
    rt.frame(
        FrameArgs {
            screen: (200, 200),
            scale: 1,
            now,
            state,
            input: &[],
            clipboard: None,
            images,
            dim: None,
            preview: None,
        },
        &mut fs,
        &mut out,
    );
    out
}

fn doc_image_batch(out: &FrameOutput, tex: u16) -> Option<&Batch> {
    out.draw
        .batches
        .iter()
        .find(|b| b.tex == TexId::DocImage(tex))
}

/// The top-left UV of the first quad drawn from document image `tex`.
fn doc_image_uv0(out: &FrameOutput, tex: u16) -> Option<[f32; 2]> {
    let b = doc_image_batch(out, tex)?;
    Some(out.draw.vertices[b.start as usize].uv)
}

const FRAMED_IMAGE_DOC: &str = r#"{
    "format": 1, "kind": "petramond:anim", "class": "screen",
    "root": { "type": "frame", "children": [
        { "type": "image", "image": "flame", "frames": [2, 2], "fps": 4.0,
          "bind": { "frame": "f" } }
    ] }
}"#;

#[test]
fn painted_uvs_follow_the_bound_frame() {
    let images = Sheets(&[("flame", 0, (64, 64))]);
    let mut state = UiState::new();

    state.set("f", UiValue::I32(1));
    let out = paint(FRAMED_IMAGE_DOC, &state, &images, 0.0);
    assert_eq!(
        doc_image_uv0(&out, 0),
        Some([0.5, 0.0]),
        "frame 1 = col 1, row 0"
    );

    // A fractional bound frame truncates (2.9 -> 2 = row 1, col 0).
    state.set("f", UiValue::F32(2.9));
    let out = paint(FRAMED_IMAGE_DOC, &state, &images, 0.0);
    assert_eq!(doc_image_uv0(&out, 0), Some([0.0, 0.5]));

    // Past the end clamps to the last frame instead of wrapping.
    state.set("f", UiValue::I32(99));
    let out = paint(FRAMED_IMAGE_DOC, &state, &images, 0.0);
    assert_eq!(
        doc_image_uv0(&out, 0),
        Some([0.5, 0.5]),
        "clamped to frame 3"
    );

    // Unbound: fps animates from the clock (now 0.6, 4 fps -> frame 2).
    let out = paint(FRAMED_IMAGE_DOC, &UiState::new(), &images, 0.6);
    assert_eq!(doc_image_uv0(&out, 0), Some([0.0, 0.5]));
}

const IMAGE_BUTTON_DOC: &str = r#"{
    "format": 1, "kind": "petramond:anim_button", "class": "screen",
    "root": { "type": "frame", "children": [
        { "type": "button", "id": "go", "image": "go_btn", "frames": [2, 2] }
    ] }
}"#;

#[test]
fn image_backed_button_sizes_to_one_frame_and_still_clicks() {
    let images = Sheets(&[("go_btn", 0, (64, 64))]);
    let rt = UiRuntime::new(
        Arc::new(Document::from_json(IMAGE_BUTTON_DOC).unwrap()),
        Arc::new(Theme::placeholder()),
    );
    let state = UiState::new();
    let mut fs = crate::input::FrameState::new();
    let mut out = FrameOutput::default();
    let frame =
        |input: &[InputEvent], fs: &mut crate::input::FrameState, out: &mut FrameOutput| {
            rt.frame(
                FrameArgs {
                    screen: (200, 200),
                    scale: 1,
                    now: 0.0,
                    state: &state,
                    input,
                    clipboard: None,
                    images: &images,
                    dim: None,
                    preview: None,
                },
                fs,
                out,
            );
        };
    frame(&[], &mut fs, &mut out);

    // Natural size is ONE frame of the 2x2 sheet, not the whole 64x64.
    let r = out.rect("go").expect("button rect");
    assert_eq!((r.w, r.h), (32, 32));

    // It draws its document image and NOT the theme button face: with an
    // unstyled root frame the draw list holds only the image batch.
    assert!(doc_image_batch(&out, 0).is_some());
    assert!(
        out.draw.batches.iter().all(|b| b.tex == TexId::DocImage(0)),
        "no theme chrome behind an image-backed button: {:?}",
        out.draw.batches
    );

    // Click behavior is the ordinary button one (press in, release in).
    let (cx, cy) = ((r.x + r.w / 2) as f32, (r.y + r.h / 2) as f32);
    let down = InputEvent::PointerDown {
        x: cx,
        y: cy,
        button: PointerButton::Primary,
        shift: false,
        slot_drag: false,
    };
    let up = InputEvent::PointerUp {
        x: cx,
        y: cy,
        button: PointerButton::Primary,
    };
    frame(&[down, up], &mut fs, &mut out);
    assert!(
        out.events
            .iter()
            .any(|e| matches!(e, crate::UiEvent::Click { id, .. } if id == "go")),
        "{:?}",
        out.events
    );
}

#[test]
fn bound_image_overrides_an_image_backed_buttons_sheet() {
    let images = Sheets(&[("go_btn", 0, (64, 64)), ("alt_btn", 1, (32, 32))]);
    let doc = r#"{
        "format": 1, "kind": "petramond:anim_button", "class": "screen",
        "root": { "type": "frame", "children": [
            { "type": "button", "id": "go", "image": "go_btn", "frames": [2, 2],
              "bind": { "image": "face" } }
        ] }
    }"#;
    let mut state = UiState::new();
    state.set("face", UiValue::Str("alt_btn".into()));
    let out = paint(doc, &state, &images, 0.0);

    // The override sheet is the one drawn (tex 1) and measured: one frame
    // of a 32x32 2x2 sheet is 16x16.
    assert!(doc_image_batch(&out, 1).is_some());
    assert!(doc_image_batch(&out, 0).is_none());
    let r = out.rect("go").expect("button rect");
    assert_eq!((r.w, r.h), (16, 16));
}
