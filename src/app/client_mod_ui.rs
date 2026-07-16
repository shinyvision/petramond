//! App-side bridge for presentation-only client mods: frame/key/UI dispatch,
//! client-owned GUI/canvas lifecycle, physical-pixel overlays, and document
//! composition for actual GUI screens.

use super::{App, AppScreen};
use crate::gui::{documents, DocImageSource, GuiKind};

pub(super) struct ClientCanvasState {
    owner: String,
    canvas_key: String,
    source_size: (u16, u16),
    rect: Option<[f32; 4]>,
    pointer_captured: bool,
    pending_move: Option<(f32, f32)>,
    /// Wheel notches accumulated this frame (positive = up); coalesced to one
    /// dispatch per frame like pointer moves.
    pending_scroll: f32,
}

impl App {
    pub(super) fn drive_client_mod_frame(&mut self, dt: f32, screen: (u32, u32)) {
        self.flush_client_canvas_move();
        self.flush_client_canvas_scroll();
        let open = match self.screen {
            AppScreen::ClientModGui(kind) => crate::gui::kind_key(kind),
            _ => None,
        };
        let open_canvas = self
            .client_canvas
            .as_ref()
            .filter(|_| self.screen == AppScreen::ClientCanvas)
            .map(|canvas| canvas.canvas_key.as_str());
        if let Some(game) = self.game.as_mut() {
            game.drive_client_mods(dt, screen, open, open_canvas);
        }
        self.apply_client_mod_commands();
    }

    pub fn release_client_mod_keys(&mut self) {
        if let Some(game) = self.game.as_mut() {
            game.release_client_mod_keys();
        }
        self.apply_client_mod_commands();
    }

    pub(super) fn drive_client_doc_ui(&mut self, kind: GuiKind, screen: (u32, u32), now: f64) {
        let Some(kind_key) = crate::gui::kind_key(kind) else {
            return;
        };
        let Some(view) = self
            .game
            .as_ref()
            .and_then(|game| game.client_mod_view(kind_key))
        else {
            return;
        };
        self.ui.ensure_active(kind);
        self.ui.replace_client_state(&view.state);
        self.ui.set_dynamic_images(view.images);
        self.ui
            .frame(kind, screen, now, Some([0.0, 0.0, 0.0, 0.55]));

        for event in self.ui.take_events() {
            let event = match event {
                petramond_ui::UiEvent::Click {
                    id,
                    button: petramond_ui::PointerButton::Primary,
                    ..
                } => Some(mod_api::ClientUiEvent::Click { id }),
                petramond_ui::UiEvent::TextChanged { id, text } => {
                    Some(mod_api::ClientUiEvent::TextChanged { id, text })
                }
                petramond_ui::UiEvent::Submit { id, text } => {
                    Some(mod_api::ClientUiEvent::Submit { id, text })
                }
                petramond_ui::UiEvent::ImagePointer {
                    id,
                    phase,
                    x,
                    y,
                    button,
                } => Some(mod_api::ClientUiEvent::ImagePointer {
                    id,
                    phase: match phase {
                        petramond_ui::PointerPhase::Down => mod_api::ClientPointerPhase::Down,
                        petramond_ui::PointerPhase::Move => mod_api::ClientPointerPhase::Move,
                        petramond_ui::PointerPhase::Up => mod_api::ClientPointerPhase::Up,
                    },
                    x,
                    y,
                    button: match button {
                        petramond_ui::PointerButton::Primary => {
                            mod_api::ClientPointerButton::Primary
                        }
                        petramond_ui::PointerButton::Secondary => {
                            mod_api::ClientPointerButton::Secondary
                        }
                    },
                }),
                _ => None,
            };
            if let Some(event) = event {
                if let Some(game) = self.game.as_mut() {
                    game.client_mod_ui_event(kind_key, event);
                }
                self.apply_client_mod_commands();
                if self.screen != AppScreen::ClientModGui(kind) {
                    break;
                }
            }
        }
    }

    pub(super) fn dispatch_client_canvas_pointer(
        &mut self,
        phase: mod_api::ClientPointerPhase,
        button: mod_api::ClientPointerButton,
        x: f32,
        y: f32,
    ) {
        let Some(canvas) = self
            .client_canvas
            .as_mut()
            .filter(|_| self.screen == AppScreen::ClientCanvas)
        else {
            return;
        };
        let Some([left, top, width, height]) = canvas.rect else {
            return;
        };
        let inside = x >= left && y >= top && x < left + width && y < top + height;
        match phase {
            mod_api::ClientPointerPhase::Down if inside => canvas.pointer_captured = true,
            mod_api::ClientPointerPhase::Down => return,
            mod_api::ClientPointerPhase::Move | mod_api::ClientPointerPhase::Up
                if !canvas.pointer_captured =>
            {
                return;
            }
            _ => {}
        }
        let canvas_key = canvas.canvas_key.clone();
        if phase == mod_api::ClientPointerPhase::Up {
            canvas.pointer_captured = false;
        }
        let event = mod_api::ClientCanvasEvent {
            phase,
            x: (x - left) * canvas.source_size.0 as f32 / width,
            y: (y - top) * canvas.source_size.1 as f32 / height,
            button,
        };
        if let Some(game) = self.game.as_mut() {
            game.client_mod_canvas_event(&canvas_key, event);
        }
        self.apply_client_mod_commands();
    }

    pub(super) fn queue_client_canvas_scroll(&mut self, delta: f32) {
        if let Some(canvas) = self
            .client_canvas
            .as_mut()
            .filter(|_| self.screen == AppScreen::ClientCanvas)
        {
            canvas.pending_scroll += delta;
        }
    }

    pub(super) fn flush_client_canvas_scroll(&mut self) {
        let Some(canvas) = self.client_canvas.as_mut().filter(|canvas| {
            self.screen == AppScreen::ClientCanvas && canvas.pending_scroll != 0.0
        }) else {
            return;
        };
        let delta = std::mem::take(&mut canvas.pending_scroll);
        // Wheel travel with the cursor off the canvas is dropped, not queued:
        // canvas-local coordinates only exist inside the rect.
        let Some([left, top, width, height]) = canvas.rect else {
            return;
        };
        let (x, y) = self.pointer.cursor();
        if x < left || y < top || x >= left + width || y >= top + height {
            return;
        }
        let canvas_key = canvas.canvas_key.clone();
        let local_x = (x - left) * canvas.source_size.0 as f32 / width;
        let local_y = (y - top) * canvas.source_size.1 as f32 / height;
        if let Some(game) = self.game.as_mut() {
            game.client_mod_canvas_scroll(&canvas_key, local_x, local_y, delta);
        }
        self.apply_client_mod_commands();
    }

    pub(super) fn queue_client_canvas_move(&mut self, x: f32, y: f32) {
        if let Some(canvas) = self
            .client_canvas
            .as_mut()
            .filter(|canvas| self.screen == AppScreen::ClientCanvas && canvas.pointer_captured)
        {
            canvas.pending_move = Some((x, y));
        }
    }

    pub(super) fn flush_client_canvas_move(&mut self) {
        let Some((x, y)) = self
            .client_canvas
            .as_mut()
            .and_then(|canvas| canvas.pending_move.take())
        else {
            return;
        };
        self.dispatch_client_canvas_pointer(
            mod_api::ClientPointerPhase::Move,
            mod_api::ClientPointerButton::Primary,
            x,
            y,
        );
    }

    pub(super) fn apply_client_mod_commands(&mut self) {
        let commands = self
            .game
            .as_mut()
            .map(|game| game.take_client_mod_commands())
            .unwrap_or_default();
        for command in commands {
            match command {
                crate::modding::ClientCommand::OpenGui { owner, kind: key } => {
                    if !client_gui_open_permitted(self.screen, &owner, &self.client_canvas) {
                        log::warn!(
                            "client mod '{owner}' cannot open '{key}' over {:?}",
                            self.screen
                        );
                        continue;
                    }
                    let Some(kind) = crate::gui::intern_kind(&key) else {
                        log::warn!("client mod requested invalid GUI kind '{key}'");
                        continue;
                    };
                    let Some(doc) = documents::doc_for(kind) else {
                        log::warn!("client mod requested missing GUI document '{key}'");
                        continue;
                    };
                    if doc.doc.class != petramond_ui::DocClass::Screen {
                        log::warn!("client mod GUI document '{key}' must have class 'screen'");
                        continue;
                    }
                    self.client_canvas = None;
                    self.screen = AppScreen::ClientModGui(kind);
                    self.pointer.release_for_menu();
                    self.gui_router.reset_click_streak();
                }
                crate::modding::ClientCommand::CloseGui { owner } => {
                    if client_gui_owned_by(self.screen, &owner) {
                        self.screen = AppScreen::Game;
                        self.pointer.grab_for_gameplay();
                    }
                }
                crate::modding::ClientCommand::OpenCanvas {
                    owner,
                    canvas_key,
                    size,
                } => {
                    if !client_canvas_open_permitted(self.screen, &owner, &self.client_canvas) {
                        log::warn!(
                            "client mod '{owner}' cannot open canvas '{canvas_key}' over {:?}",
                            self.screen
                        );
                        continue;
                    }
                    self.client_canvas = Some(ClientCanvasState {
                        owner,
                        canvas_key,
                        source_size: (size[0], size[1]),
                        rect: None,
                        pointer_captured: false,
                        pending_move: None,
                        pending_scroll: 0.0,
                    });
                    self.screen = AppScreen::ClientCanvas;
                    self.pointer.release_for_menu();
                    self.gui_router.reset_click_streak();
                }
                crate::modding::ClientCommand::CloseCanvas { owner } => {
                    if client_canvas_owned_by(self.screen, &owner, &self.client_canvas) {
                        self.client_canvas = None;
                        self.screen = AppScreen::Game;
                        self.pointer.grab_for_gameplay();
                    }
                }
            }
        }
    }

    pub(super) fn compose_document_ui(&mut self, include_main: bool) {
        self.composed_doc.clear();
        self.composed_doc_images.clear();
        if include_main {
            append_layer(
                &mut self.composed_doc,
                &mut self.composed_doc_images,
                &self.ui.out().draw,
                self.ui.image_sources(),
            );
        }
    }

    pub(super) fn compose_client_overlays(&mut self, screen: (u32, u32)) {
        self.client_overlay_images.clear();
        if matches!(self.screen, AppScreen::Game | AppScreen::Chat) && self.game.is_some() {
            let game = self.game.as_ref().unwrap();
            for overlay in game.client_mod_overlays() {
                let Some(image) = game.client_mod_image(&overlay.image_key) else {
                    continue;
                };
                let rect = overlay_rect(
                    screen,
                    (overlay.display_size[0], overlay.display_size[1]),
                    overlay.anchor,
                    overlay.margin,
                );
                self.client_overlay_images
                    .push(render_image(image, rect, [0.0, 0.0, 1.0, 1.0]));
            }
        }

        let canvas = self
            .client_canvas
            .as_ref()
            .filter(|_| self.screen == AppScreen::ClientCanvas)
            .map(|canvas| (canvas.canvas_key.clone(), canvas.source_size));
        if let Some((canvas_key, source_size)) = canvas {
            let rect = canvas_rect(screen, source_size);
            if let Some(canvas) = self.client_canvas.as_mut() {
                canvas.rect = Some(rect);
            }
            if let Some(view) = self
                .game
                .as_ref()
                .and_then(|game| game.client_mod_canvas_view(&canvas_key))
            {
                for element in view.elements {
                    let element_rect = match element.element {
                        mod_api::ClientCanvasElement::Image {
                            rect: source_rect, ..
                        } => canvas_image_rect(rect, source_size, source_rect, view.offset),
                        mod_api::ClientCanvasElement::Sprite { center, .. } => canvas_sprite_rect(
                            rect,
                            source_size,
                            center,
                            view.offset,
                            (element.image.width, element.image.height),
                        ),
                    };
                    if let Some((element_rect, uv)) = clip_rect_uv(element_rect, rect) {
                        self.client_overlay_images.push(render_image(
                            element.image,
                            element_rect,
                            uv,
                        ));
                    }
                }
            }
        }
    }
}

fn client_gui_open_permitted(
    screen: AppScreen,
    owner: &str,
    canvas: &Option<ClientCanvasState>,
) -> bool {
    screen == AppScreen::Game
        || client_gui_owned_by(screen, owner)
        || client_canvas_owned_by(screen, owner, canvas)
}

pub(super) fn client_key_dispatch_permitted(
    pressed: bool,
    screen: AppScreen,
    text_focused: bool,
) -> bool {
    !pressed
        || (!text_focused
            && (screen.gameplay_enabled()
                || screen.client_ui_open()
                || screen.client_canvas_open()))
}

fn client_canvas_open_permitted(
    screen: AppScreen,
    owner: &str,
    canvas: &Option<ClientCanvasState>,
) -> bool {
    screen == AppScreen::Game
        || client_gui_owned_by(screen, owner)
        || client_canvas_owned_by(screen, owner, canvas)
}

fn client_canvas_owned_by(
    screen: AppScreen,
    owner: &str,
    canvas: &Option<ClientCanvasState>,
) -> bool {
    screen == AppScreen::ClientCanvas && canvas.as_ref().is_some_and(|canvas| canvas.owner == owner)
}

fn overlay_rect(
    screen: (u32, u32),
    size: (u16, u16),
    anchor: mod_api::ClientOverlayAnchor,
    margin: [u16; 2],
) -> [f32; 4] {
    let w = size.0 as f32;
    let h = size.1 as f32;
    let mx = margin[0] as f32;
    let my = margin[1] as f32;
    let x = match anchor {
        mod_api::ClientOverlayAnchor::TopLeft | mod_api::ClientOverlayAnchor::BottomLeft => mx,
        mod_api::ClientOverlayAnchor::TopRight | mod_api::ClientOverlayAnchor::BottomRight => {
            screen.0 as f32 - mx - w
        }
    };
    let y = match anchor {
        mod_api::ClientOverlayAnchor::TopLeft | mod_api::ClientOverlayAnchor::TopRight => my,
        mod_api::ClientOverlayAnchor::BottomLeft | mod_api::ClientOverlayAnchor::BottomRight => {
            screen.1 as f32 - my - h
        }
    };
    [x.floor(), y.floor(), w, h]
}

fn canvas_rect(screen: (u32, u32), source_size: (u16, u16)) -> [f32; 4] {
    const MARGIN: f32 = 32.0;
    let source_w = source_size.0 as f32;
    let source_h = source_size.1 as f32;
    let available_w = (screen.0 as f32 - MARGIN * 2.0).max(1.0);
    let available_h = (screen.1 as f32 - MARGIN * 2.0).max(1.0);
    let scale = (available_w / source_w)
        .min(available_h / source_h)
        .min(1.0);
    let w = source_w * scale;
    let h = source_h * scale;
    [
        ((screen.0 as f32 - w) * 0.5).floor(),
        ((screen.1 as f32 - h) * 0.5).floor(),
        w,
        h,
    ]
}

fn canvas_sprite_rect(
    canvas_rect: [f32; 4],
    source_size: (u16, u16),
    center: [f32; 2],
    offset: [f32; 2],
    sprite_size: (u16, u16),
) -> [f32; 4] {
    let center_x = canvas_rect[0] + (center[0] + offset[0]) * canvas_rect[2] / source_size.0 as f32;
    let center_y = canvas_rect[1] + (center[1] + offset[1]) * canvas_rect[3] / source_size.1 as f32;
    [
        (center_x - sprite_size.0 as f32 * 0.5).round(),
        (center_y - sprite_size.1 as f32 * 0.5).round(),
        sprite_size.0 as f32,
        sprite_size.1 as f32,
    ]
}

fn canvas_image_rect(
    canvas_rect: [f32; 4],
    source_size: (u16, u16),
    rect: [f32; 4],
    offset: [f32; 2],
) -> [f32; 4] {
    let sx = canvas_rect[2] / source_size.0 as f32;
    let sy = canvas_rect[3] / source_size.1 as f32;
    [
        canvas_rect[0] + (rect[0] + offset[0]) * sx,
        canvas_rect[1] + (rect[1] + offset[1]) * sy,
        rect[2] * sx,
        rect[3] * sy,
    ]
}

fn clip_rect_uv(rect: [f32; 4], clip: [f32; 4]) -> Option<([f32; 4], [f32; 4])> {
    let left = rect[0].max(clip[0]);
    let top = rect[1].max(clip[1]);
    let right = (rect[0] + rect[2]).min(clip[0] + clip[2]);
    let bottom = (rect[1] + rect[3]).min(clip[1] + clip[3]);
    if left >= right || top >= bottom {
        return None;
    }
    let uv = [
        (left - rect[0]) / rect[2],
        (top - rect[1]) / rect[3],
        (right - rect[0]) / rect[2],
        (bottom - rect[1]) / rect[3],
    ];
    Some(([left, top, right - left, bottom - top], uv))
}

fn render_image(
    image: crate::modding::ClientImageData,
    rect: [f32; 4],
    uv: [f32; 4],
) -> crate::render::ClientOverlayImage {
    crate::render::ClientOverlayImage {
        key: image.key,
        size: (image.width, image.height),
        rgba: image.rgba,
        revision: image.revision,
        recent_blits: image.recent_blits,
        rect,
        uv,
    }
}

fn client_gui_owned_by(screen: AppScreen, owner: &str) -> bool {
    let AppScreen::ClientModGui(kind) = screen else {
        return false;
    };
    crate::gui::kind_key(kind)
        .and_then(|key| key.split_once(':'))
        .is_some_and(|(namespace, _)| namespace == owner)
}

fn append_layer(
    dst: &mut petramond_ui::DrawList,
    dst_images: &mut Vec<DocImageSource>,
    src: &petramond_ui::DrawList,
    images: &[DocImageSource],
) {
    let vertex_base = dst.vertices.len() as u32;
    let image_base = dst_images.len() as u16;
    dst.vertices.extend_from_slice(&src.vertices);
    dst.batches.extend(src.batches.iter().map(|batch| {
        let mut batch = *batch;
        batch.start += vertex_base;
        if let petramond_ui::TexId::DocImage(index) = batch.tex {
            batch.tex = petramond_ui::TexId::DocImage(image_base + index);
        }
        batch
    }));
    dst_images.extend_from_slice(images);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_gui_commands_are_context_and_owner_scoped() {
        let map = crate::gui::intern_kind("map:screen").unwrap();
        let other = crate::gui::intern_kind("other:screen").unwrap();
        let no_canvas = None;
        let canvas = Some(ClientCanvasState {
            owner: "map".into(),
            canvas_key: "map:canvas".into(),
            source_size: (320, 320),
            rect: None,
            pointer_captured: false,
            pending_move: None,
            pending_scroll: 0.0,
        });

        assert!(client_gui_open_permitted(
            AppScreen::Game,
            "map",
            &no_canvas
        ));
        assert!(client_gui_open_permitted(
            AppScreen::ClientModGui(map),
            "map",
            &no_canvas,
        ));
        assert!(!client_gui_open_permitted(
            AppScreen::ClientModGui(other),
            "map",
            &no_canvas,
        ));
        assert!(client_gui_open_permitted(
            AppScreen::ClientCanvas,
            "map",
            &canvas,
        ));
        assert!(!client_gui_open_permitted(
            AppScreen::ClientCanvas,
            "other",
            &canvas,
        ));
        for screen in [
            AppScreen::Pause,
            AppScreen::Inventory,
            AppScreen::Sleeping,
            AppScreen::Dead,
        ] {
            assert!(
                !client_gui_open_permitted(screen, "map", &no_canvas),
                "{screen:?}"
            );
        }
        assert!(client_gui_owned_by(AppScreen::ClientModGui(map), "map"));
        assert!(!client_gui_owned_by(AppScreen::ClientModGui(map), "other"));

        assert!(client_key_dispatch_permitted(
            false,
            AppScreen::Pause,
            false
        ));
        assert!(client_key_dispatch_permitted(false, AppScreen::Pause, true));
        assert!(!client_key_dispatch_permitted(
            true,
            AppScreen::Pause,
            false
        ));
        assert!(!client_key_dispatch_permitted(true, AppScreen::Game, true));
        assert!(client_key_dispatch_permitted(true, AppScreen::Game, false));
        assert!(client_key_dispatch_permitted(
            true,
            AppScreen::ClientCanvas,
            false
        ));
    }

    #[test]
    fn client_overlay_rect_uses_explicit_physical_display_size() {
        let rect = overlay_rect(
            (1900, 1000),
            (256, 256),
            mod_api::ClientOverlayAnchor::TopRight,
            [8, 8],
        );
        assert_eq!(rect[2..], [256.0, 256.0]);
        assert_eq!(rect[0] + rect[2] + 8.0, 1900.0);
        assert_eq!(rect[1], 8.0);
    }

    #[test]
    fn client_canvas_keeps_native_resolution_when_it_fits() {
        let rect = canvas_rect((1900, 1034), (640, 640));
        assert_eq!(rect[2..], [640.0, 640.0]);
        assert_eq!(rect[0], 630.0);
        assert_eq!(rect[1], 197.0);
    }

    #[test]
    fn client_canvas_sprites_keep_native_resolution_under_the_view_transform() {
        let canvas = canvas_rect((1900, 1034), (320, 320));
        let sprite = canvas_sprite_rect(canvas, (320, 320), [160.0, 160.0], [8.0, -4.0], (48, 48));
        assert_eq!(
            sprite[2..],
            [48.0, 48.0],
            "the sprite keeps its native pixel size, whatever the canvas transform"
        );
        assert!(
            sprite[0] >= canvas[0]
                && sprite[1] >= canvas[1]
                && sprite[0] + sprite[2] <= canvas[0] + canvas[2]
                && sprite[1] + sprite[3] <= canvas[1] + canvas[3],
            "a near-centre sprite lands inside the canvas: {sprite:?} vs {canvas:?}"
        );
    }

    #[test]
    fn client_canvas_images_scale_and_translate_in_logical_canvas_space() {
        let canvas = canvas_rect((384, 384), (640, 640));
        assert_eq!(canvas, [32.0, 32.0, 320.0, 320.0]);
        let image = canvas_image_rect(
            canvas,
            (640, 640),
            [160.0, 80.0, 160.0, 160.0],
            [-80.0, 40.0],
        );
        assert_eq!(image, [72.0, 92.0, 80.0, 80.0]);
    }

    #[test]
    fn client_canvas_clipping_preserves_the_matching_texture_region() {
        let full = [0.0, 0.0, 100.0, 100.0];
        let (rect, uv) =
            clip_rect_uv(full, [25.0, 10.0, 50.0, 80.0]).expect("the rectangles overlap");
        assert_eq!(rect, [25.0, 10.0, 50.0, 80.0]);
        // The two outputs must agree: mapping the UVs back through the full
        // rect reproduces the clipped rectangle.
        assert!(uv[0] < uv[2] && uv[1] < uv[3], "uv ordering: {uv:?}");
        let roundtrip = [
            full[0] + uv[0] * full[2],
            full[1] + uv[1] * full[3],
            (uv[2] - uv[0]) * full[2],
            (uv[3] - uv[1]) * full[3],
        ];
        for (got, want) in roundtrip.iter().zip(rect) {
            assert!((got - want).abs() < 1e-3, "{roundtrip:?} vs {rect:?}");
        }
    }
}
