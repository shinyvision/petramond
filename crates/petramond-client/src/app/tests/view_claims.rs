//! View claims at the app: the chrome a claim hides, what it must never hide,
//! and a canvas's geometry and glyph rows on the overlay draw list.

use super::app;
use crate::app::client_mod_ui::{compose_canvas, CanvasPlacement};
use petramond::modding::client::view::{ViewChromeClaim, ViewClaims};
use petramond::modding::client::ClientCanvasElementView;
use petramond_input::controls::Control;
use petramond_render::{ClientOverlayItem, ClientOverlayLayer};
use petramond_world::gui_state::GuiKind;

const SCREEN: (u32, u32) = (1280, 720);

fn renderer() -> Option<petramond_render::Renderer> {
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
    if pollster::block_on(instance.request_adapter(&Default::default())).is_err() {
        eprintln!("[skip] no wgpu adapter; view-claim frames not rendered");
        return None;
    }
    Some(
        pollster::block_on(petramond_render::new_offscreen_renderer(
            SCREEN.0,
            SCREEN.1,
            wgpu::TextureFormat::Rgba8UnormSrgb,
        ))
        .expect("offscreen renderer"),
    )
}

/// Update then draw until a frame presents (a menu's first solve can stamp a
/// stale viewport).
fn present(app: &mut crate::app::App, renderer: &mut petramond_render::Renderer) {
    for _ in 0..4 {
        app.update(renderer);
        if app.render(renderer) {
            return;
        }
    }
    panic!("no frame presented");
}

fn chrome(hud: Option<bool>) -> ViewClaims {
    ViewClaims {
        chrome: ViewChromeClaim {
            hud,
            ..Default::default()
        },
        ..Default::default()
    }
}

/// Registered overlays of one kind (HUD or tool) that have an image to draw.
fn overlays_with_images(app: &crate::app::App, hud: bool) -> usize {
    let game = app.game();
    game.client_mod_overlays()
        .iter()
        .filter(|o| o.hud == hud && game.client_mod_image(&o.image_key).is_some())
        .count()
}

#[test]
fn a_hidden_hud_hides_the_hud_across_mods_but_never_an_open_menu() {
    let Some(mut renderer) = renderer() else {
        return;
    };
    let mut app = app();
    present(&mut app, &mut renderer);
    assert!(app.presented_view.hud_visible);
    assert_eq!(
        app.hud_ui.frame_stamp().map(|(k, _)| k),
        Some(GuiKind::Hotbar)
    );

    assert!(
        overlays_with_images(&app, true) > 0,
        "a HUD overlay to hide"
    );
    // One mod hides the HUD, another asks for it: hiding wins.
    let game = app.game_mut();
    assert!(game.claim_view_for_test("minimap", chrome(Some(false))));
    assert!(game.claim_view_for_test("weather", chrome(Some(true))));
    present(&mut app, &mut renderer);
    assert!(!app.presented_view.hud_visible);
    assert_eq!(
        app.hud_ui.frame_stamp(),
        None,
        "no hotbar document, no chat"
    );
    assert_eq!(
        app.client_overlays.items.len(),
        overlays_with_images(&app, false),
        "mod HUD overlays hide too; a tool's own overlay stays"
    );

    // A hidden HUD is not a hidden UI.
    app.handle_control(Control::ToggleInventory, true);
    present(&mut app, &mut renderer);
    assert_eq!(
        app.ui.frame_stamp().map(|(k, _)| k),
        Some(GuiKind::Inventory)
    );
    assert!(!app.composed_doc.vertices.is_empty());

    // Releasing the claims restores the HUD exactly.
    app.handle_control(Control::CloseScreen, true);
    let game = app.game_mut();
    game.claim_view_for_test("minimap", ViewClaims::default());
    game.claim_view_for_test("weather", ViewClaims::default());
    present(&mut app, &mut renderer);
    assert!(app.presented_view.hud_visible);
    assert_eq!(
        app.hud_ui.frame_stamp().map(|(k, _)| k),
        Some(GuiKind::Hotbar)
    );
}

#[test]
fn canvas_rules_and_labels_paint_in_order_and_inside_the_canvas() {
    let canvas = [100.0, 50.0, 400.0, 300.0];
    let at = CanvasPlacement {
        rect: canvas,
        source_size: (200, 150),
        offset: [0.0, 0.0],
        gui_scale: 2,
    };
    let image = petramond::modding::ClientImageData {
        key: "test:img".into(),
        width: 2,
        height: 2,
        rgba: vec![255; 16].into(),
        revision: 1,
        recent_blits: Vec::new(),
    };
    let row = |element, image| ClientCanvasElementView { element, image };
    let mut layer = ClientOverlayLayer::default();
    compose_canvas(
        &mut layer,
        &at,
        vec![
            // Hangs off the canvas's bottom-right corner.
            row(
                mod_api::ClientCanvasElement::Rect {
                    rect: [150.0, 100.0, 100.0, 100.0],
                    color: [255, 0, 0, 255],
                    filled: true,
                },
                None,
            ),
            row(
                mod_api::ClientCanvasElement::Image {
                    image_key: "test:img".into(),
                    rect: [0.0, 0.0, 10.0, 10.0],
                },
                Some(image),
            ),
            // Starts inside, runs far past the right edge.
            row(
                mod_api::ClientCanvasElement::Text {
                    pos: [150.0, 10.0],
                    text: "a label far too long to fit in what is left".repeat(4),
                    color: [255; 4],
                    small: false,
                    max_w: None,
                },
                None,
            ),
            row(
                mod_api::ClientCanvasElement::Rect {
                    rect: [10.0, 10.0, 20.0, 20.0],
                    color: [0, 255, 0, 255],
                    filled: false,
                },
                None,
            ),
        ],
    );

    let kinds: Vec<&str> = layer
        .items
        .iter()
        .map(|item| match item {
            ClientOverlayItem::Image(_) => "image",
            ClientOverlayItem::Paint { .. } => "paint",
        })
        .collect();
    assert_eq!(
        kinds,
        ["paint", "image", "paint", "paint"],
        "retained order"
    );

    let inside = |p: [f32; 2]| {
        p[0] >= canvas[0]
            && p[1] >= canvas[1]
            && p[0] <= canvas[0] + canvas[2]
            && p[1] <= canvas[1] + canvas[3]
    };
    let mut glyphs = 0;
    for batch in &layer.paint.batches {
        let [x, y, w, h] = batch.clip.expect("every painted batch scissors");
        assert!(
            inside([x as f32, y as f32]) && inside([(x + w) as f32, (y + h) as f32]),
            "the scissor stays inside the canvas"
        );
        let verts = &layer.paint.vertices[batch.start as usize..][..batch.count as usize];
        match batch.tex {
            petramond_ui::TexId::Solid => {
                assert!(verts.iter().all(|v| inside(v.pos)), "solids clip exactly");
            }
            petramond_ui::TexId::Font => glyphs += verts.len() / 6,
            other => panic!("a canvas paints no {other:?}"),
        }
    }
    let label = "a label far too long to fit in what is left".repeat(4);
    assert!(
        glyphs > 0 && glyphs < label.len(),
        "the label ellipsizes at the canvas edge ({glyphs} glyphs)"
    );
}

/// A frame-size claim renders the world and its HUD at exactly that size,
/// whatever the window's, with the HUD laid out at the FRAME's UI scale
/// (uncapped: a 4K frame's HUD keeps a 1080p frame's proportions); the
/// window's own UI keeps the window's. Releasing it gives the window back.
#[test]
fn a_frame_size_claim_renders_the_world_and_its_hud_at_that_size() {
    let Some(mut renderer) = renderer() else {
        return;
    };
    let mut app = app();
    present(&mut app, &mut renderer);
    assert_eq!(app.presented_view.frame_size, [SCREEN.0, SCREEN.1]);

    let claim = ViewClaims {
        frame_size: Some([3840, 2160]),
        ..Default::default()
    };
    assert!(app.game_mut().claim_view_for_test("minimap", claim));
    present(&mut app, &mut renderer);
    present(&mut app, &mut renderer);
    assert_eq!(app.presented_view.frame_size, [3840, 2160]);
    let scene = renderer.scene_ui_viewport();
    assert_eq!(scene.size, (3840, 2160));
    assert_eq!(scene.scale, 9, "the frame's scale is not capped at 4");
    assert_eq!(
        app.hud_ui.frame_stamp().map(|(_, viewport)| viewport.scale),
        Some(9),
        "the HUD laid out for the frame"
    );
    assert_eq!(renderer.window_ui_viewport().size, SCREEN);

    app.game_mut()
        .claim_view_for_test("minimap", ViewClaims::default());
    present(&mut app, &mut renderer);
    present(&mut app, &mut renderer);
    assert_eq!(app.presented_view.frame_size, [SCREEN.0, SCREEN.1]);
    assert!(!renderer.sized_frames_active());
}
