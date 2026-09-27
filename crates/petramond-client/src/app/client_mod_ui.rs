//! App-side bridge for presentation-only client mods: frame/key/UI dispatch,
//! client-owned GUI/canvas lifecycle, physical-pixel overlays, and document
//! composition for actual GUI screens.

use super::{App, AppScreen};
use petramond::gui::{documents, DocImageSource};
use petramond_world::gui_state::GuiKind;

#[derive(Clone)]
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

impl ClientCanvasState {
    pub(super) fn key(&self) -> &str {
        &self.canvas_key
    }
}

impl App {
    pub(super) fn drive_client_mod_frame(
        &mut self,
        dt: f32,
        wall_dt: f32,
        viewport: petramond::gui::UiViewport,
        frozen: bool,
    ) {
        let screen = viewport.size;
        let gui_scale = viewport.scale.clamp(1, 255) as u8;
        self.publish_client_screen();
        self.flush_client_canvas_move();
        self.flush_client_canvas_scroll();
        let open = match self.screen {
            AppScreen::ClientModGui(kind) => petramond_world::gui_state::kind_key(kind),
            _ => None,
        };
        let open_canvas = self
            .client_canvas
            .as_ref()
            .filter(|_| self.screen == AppScreen::ClientCanvas)
            .map(|canvas| canvas.canvas_key.as_str());
        // The player fields stay zero on the shell; a world's game fills them.
        let frame = mod_api::ClientFrameData {
            dt: dt.max(0.0),
            player_pos: [0.0; 3],
            yaw: 0.0,
            pitch: 0.0,
            screen: [screen.0, screen.1],
            open_gui: open.map(str::to_owned),
            open_canvas: open_canvas.map(str::to_owned),
            gui_scale,
            frozen,
            wall_dt: wall_dt.max(0.0),
        };
        let presented = self.presented_view;
        if let Some(session) = self.session.as_mut() {
            session.game.drive_client_mods(frame, presented);
        } else if let Some(runtime) = self.shell_mods_mut() {
            runtime.frame_detached(frame);
        }
        self.apply_client_mod_commands();
        self.drive_shell_presentation();
    }

    pub fn release_client_mod_keys(&mut self) {
        if let Some(session) = self.session.as_mut() {
            session.game.release_client_mod_keys();
        }
        self.apply_client_mod_commands();
    }

    pub(super) fn drive_client_doc_ui(&mut self, kind: GuiKind, screen: (u32, u32), now: f64) {
        let Some(kind_key) = petramond_world::gui_state::kind_key(kind) else {
            return;
        };
        let Some(view) = self.client_mod_view(kind_key) else {
            return;
        };
        self.ui.ensure_active(kind);
        self.ui.replace_client_state(&view.state);
        self.ui.set_dynamic_images(view.images);
        self.ui.set_scenes(&view.scenes);
        let dim = (!Self::doc_presents_world(kind)).then_some([0.0, 0.0, 0.0, 0.55]);
        self.ui.frame(kind, screen, now, dim);

        let mut events: Vec<_> = self
            .ui
            .take_events()
            .into_iter()
            .filter_map(super::client_doc_events::client_ui_event)
            .collect();
        events.extend(self.client_doc_watch.changes(kind, self.ui.out()));
        for event in events {
            self.client_mod_ui_event(kind_key, event);
            self.apply_client_mod_commands();
            if self.screen != AppScreen::ClientModGui(kind) {
                break;
            }
        }
    }

    /// Tell every client-mod runtime the rendering device's frame limits,
    /// which a frame capture and a frame-size claim meet. Called once, before
    /// the first runtime starts.
    pub fn publish_device_frame_limits(renderer: &petramond_render::Renderer) {
        let (max_side, max_bytes) = renderer.frame_limits();
        petramond::modding::client::presented::FrameLimits {
            max_side,
            max_bytes,
        }
        .publish();
    }

    /// Tell the client mods which of their screens is up, and which of its
    /// text inputs could take focus — as of the last drawn frame.
    fn publish_client_screen(&mut self) {
        let screen = match self.screen {
            AppScreen::ClientModGui(kind) => {
                petramond_world::gui_state::kind_key(kind).map(str::to_owned)
            }
            _ => self.client_canvas_key().map(str::to_owned),
        };
        let text_inputs = match self.screen {
            AppScreen::ClientModGui(_) => self
                .ui
                .out()
                .text_inputs
                .iter()
                .map(|k| (k.id.clone(), k.item))
                .collect(),
            _ => Vec::new(),
        };
        if screen.is_none() {
            self.client_doc_watch.forget();
        }
        if let Some(runtime) = self.client_mods_now() {
            let mut presented = runtime.presented().lock();
            presented.screen = screen;
            presented.text_inputs = text_inputs;
        }
    }

    /// Escape over a client document that unwinds its own layers: its owner
    /// hears `Dismiss` and the document stays. `false` = Escape closes it.
    pub(super) fn dismiss_client_doc(&mut self) -> bool {
        let AppScreen::ClientModGui(kind) = self.screen else {
            return false;
        };
        let asks = documents::doc_for(kind)
            .is_some_and(|doc| doc.doc.dismiss == petramond_ui::Dismiss::Event);
        asks && self.send_client_doc_dismiss()
    }

    /// Tell the client document on screen that Escape was pressed over it;
    /// it stays open. `false` = no client document is up.
    pub(super) fn send_client_doc_dismiss(&mut self) -> bool {
        let AppScreen::ClientModGui(kind) = self.screen else {
            return false;
        };
        let Some(kind_key) = petramond_world::gui_state::kind_key(kind) else {
            return false;
        };
        self.client_mod_ui_event(kind_key, mod_api::ClientUiEvent::Dismiss);
        self.apply_client_mod_commands();
        true
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
        self.client_mod_canvas_event(&canvas_key, event);
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
        let (x, y) = self.controls.pointer.cursor();
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
        if x < left || y < top || x >= left + width || y >= top + height {
            return;
        }
        let canvas_key = canvas.canvas_key.clone();
        let local_x = (x - left) * canvas.source_size.0 as f32 / width;
        let local_y = (y - top) * canvas.source_size.1 as f32 / height;
        self.client_mod_canvas_scroll(&canvas_key, local_x, local_y, delta);
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
        let commands = self.take_client_mod_commands();
        // A client mod's UI replaces the world's view, or on the shell the
        // title it was launched from.
        let base = if self.session.is_some() {
            AppScreen::Game
        } else {
            AppScreen::Title
        };
        for command in commands {
            match command {
                petramond::modding::ClientCommand::OpenGui { owner, kind: key } => {
                    if !client_ui_open_permitted(self.screen, base, &owner, &self.client_canvas) {
                        log::warn!(
                            "client mod '{owner}' cannot open '{key}' over {:?}",
                            self.screen
                        );
                        continue;
                    }
                    let Some(kind) = petramond_world::gui_state::intern_kind(&key) else {
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
                    // Leaving an open canvas for the GUI drops the canvas.
                    self.set_screen(AppScreen::ClientModGui(kind));
                }
                petramond::modding::ClientCommand::CloseGui { owner } => {
                    if client_gui_owned_by(self.screen, &owner) {
                        self.leave_client_screen();
                    }
                }
                petramond::modding::ClientCommand::OpenCanvas {
                    owner,
                    canvas_key,
                    size,
                } => {
                    if !client_ui_open_permitted(self.screen, base, &owner, &self.client_canvas) {
                        log::warn!(
                            "client mod '{owner}' cannot open canvas '{canvas_key}' over {:?}",
                            self.screen
                        );
                        continue;
                    }
                    // Enter the screen FIRST: leaving a previous canvas
                    // screen drops that canvas, never this one.
                    self.set_screen(AppScreen::ClientCanvas);
                    self.client_canvas = Some(ClientCanvasState {
                        owner,
                        canvas_key,
                        source_size: (size[0], size[1]),
                        rect: None,
                        pointer_captured: false,
                        pending_move: None,
                        pending_scroll: 0.0,
                    });
                }
                petramond::modding::ClientCommand::CloseCanvas { owner } => {
                    if client_canvas_owned_by(self.screen, &owner, &self.client_canvas) {
                        self.leave_client_screen();
                    }
                }
                petramond::modding::ClientCommand::FocusInput { owner, id, item } => {
                    if client_gui_owned_by(self.screen, &owner) {
                        self.ui.request_focus(petramond_ui::InstKey { id, item });
                    }
                }
                petramond::modding::ClientCommand::OpenPause { owner } => {
                    let own = client_gui_owned_by(self.screen, &owner)
                        || client_canvas_owned_by(self.screen, &owner, &self.client_canvas);
                    if own && self.session.is_some() {
                        let back = (self.screen, self.client_canvas.clone());
                        self.open_pause();
                        self.pause_return = Some(back);
                    }
                }
            }
        }
        self.settle_shell();
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
        self.client_overlays.clear();
        // A HUD overlay follows the HUD (a hidden HUD hides it with the
        // rest); a tool's own overlay does not. Both draw on the window only,
        // like every mod surface.
        let on_game = matches!(self.screen, AppScreen::Game | AppScreen::Chat);
        let hud = self.hud_claim_allows();
        if let Some(game) = self
            .session
            .as_ref()
            .filter(|_| on_game)
            .map(|session| &session.game)
        {
            for overlay in game.client_mod_overlays().iter().filter(|o| hud || !o.hud) {
                let Some(image) = game.client_mod_image(&overlay.image_key) else {
                    continue;
                };
                let rect = overlay_rect(
                    screen,
                    (overlay.display_size[0], overlay.display_size[1]),
                    overlay.anchor,
                    overlay.margin,
                );
                self.client_overlays
                    .push_image(render_image(image, rect, [0.0, 0.0, 1.0, 1.0]));
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
            if let Some(view) = self.client_mod_canvas_view(&canvas_key) {
                let placement = CanvasPlacement {
                    rect,
                    source_size,
                    offset: view.offset,
                    gui_scale: petramond::gui::gui_scale(screen) as i32,
                };
                compose_canvas(&mut self.client_overlays, &placement, view.elements);
            }
        }
    }
}

/// Where a canvas sits on screen: its display rect, its logical size, the
/// scene's view offset, and the host's GUI scale (the glyph size of its text).
pub(super) struct CanvasPlacement {
    pub(super) rect: [f32; 4],
    pub(super) source_size: (u16, u16),
    pub(super) offset: [f32; 2],
    pub(super) gui_scale: i32,
}

/// Append a canvas scene to the overlay layer in its retained order, every
/// element clipped to the canvas rect.
pub(super) fn compose_canvas(
    layer: &mut petramond_render::ClientOverlayLayer,
    at: &CanvasPlacement,
    elements: Vec<petramond::modding::client::ClientCanvasElementView>,
) {
    let theme = petramond::gui::doc_theme::theme();
    for row in elements {
        let (image_rect, image) = match (&row.element, row.image) {
            (mod_api::ClientCanvasElement::Image { rect, .. }, Some(image)) => (
                canvas_image_rect(at.rect, at.source_size, *rect, at.offset),
                image,
            ),
            (mod_api::ClientCanvasElement::Sprite { center, .. }, Some(image)) => (
                canvas_sprite_rect(
                    at.rect,
                    at.source_size,
                    *center,
                    at.offset,
                    (image.width, image.height),
                ),
                image,
            ),
            (element, _) => {
                layer.paint(|list| {
                    let mut painter = petramond_ui::Painter {
                        list,
                        scale: 1,
                        font: theme.ui_font(),
                    };
                    paint_canvas_element(&mut painter, at, element);
                });
                continue;
            }
        };
        if let Some((image_rect, uv)) = clip_rect_uv(image_rect, at.rect) {
            layer.push_image(render_image(image, image_rect, uv));
        }
    }
}

/// Paint one geometry or glyph row. The painter works in physical px
/// (scale 1): the canvas transform places the row, and the canvas rect
/// clips it — solids exactly, glyph runs by ellipsis plus scissor, the way
/// document labels are fitted.
fn paint_canvas_element(
    painter: &mut petramond_ui::Painter<'_>,
    at: &CanvasPlacement,
    element: &mod_api::ClientCanvasElement,
) {
    let clip = pixel_rect(at.rect);
    let rgba = |c: [u8; 4]| c.map(|v| f32::from(v) / 255.0);
    match element {
        mod_api::ClientCanvasElement::Rect {
            rect,
            color,
            filled,
        } => {
            let r = pixel_rect(canvas_image_rect(at.rect, at.source_size, *rect, at.offset));
            let color = rgba(*color);
            let strips = if *filled || r.w <= 2 || r.h <= 2 {
                vec![r]
            } else {
                let edge = |x, y, w, h| petramond_ui::RectI { x, y, w, h };
                vec![
                    edge(r.x, r.y, r.w, 1),
                    edge(r.x, r.y + r.h - 1, r.w, 1),
                    edge(r.x, r.y + 1, 1, r.h - 2),
                    edge(r.x + r.w - 1, r.y + 1, 1, r.h - 2),
                ]
            };
            for strip in strips {
                let strip = strip.intersect(clip);
                if strip.w > 0 && strip.h > 0 {
                    painter.solid(strip, color, Some(clip));
                }
            }
        }
        mod_api::ClientCanvasElement::Text {
            pos,
            text,
            color,
            small,
            max_w,
        } => {
            let scale = at.gui_scale.max(1);
            let k = if *small { (scale - 1).max(1) } else { scale };
            let [x, y, ..] = canvas_image_rect(
                at.rect,
                at.source_size,
                [pos[0], pos[1], 0.0, 0.0],
                at.offset,
            );
            let (x, y) = (x.round() as i32, y.round() as i32);
            let right = match max_w {
                Some(w) => {
                    let scaled = w * at.rect[2] / f32::from(at.source_size.0.max(1));
                    (x + scaled.round() as i32).min(clip.x + clip.w)
                }
                None => clip.x + clip.w,
            };
            let line = petramond_ui::RectI {
                x,
                y,
                w: (right - x).max(0),
                h: painter.font.line_h() * k,
            };
            painter.ellipsized_at(text, line, k, rgba(*color), Some(clip));
        }
        mod_api::ClientCanvasElement::Image { .. }
        | mod_api::ClientCanvasElement::Sprite { .. } => {}
    }
}

/// A float screen rect snapped to whole pixels by its edges.
fn pixel_rect(r: [f32; 4]) -> petramond_ui::RectI {
    let (x0, y0) = (r[0].round() as i32, r[1].round() as i32);
    let (x1, y1) = ((r[0] + r[2]).round() as i32, (r[1] + r[3]).round() as i32);
    petramond_ui::RectI {
        x: x0,
        y: y0,
        w: (x1 - x0).max(0),
        h: (y1 - y0).max(0),
    }
}

/// Whether `owner` may open a document or canvas over `screen`: over `base`
/// (the world, or on the shell the title), or in place of its own.
fn client_ui_open_permitted(
    screen: AppScreen,
    base: AppScreen,
    owner: &str,
    canvas: &Option<ClientCanvasState>,
) -> bool {
    screen == base
        || client_gui_owned_by(screen, owner)
        || client_canvas_owned_by(screen, owner, canvas)
}

pub(super) fn client_canvas_owned_by(
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
    image: petramond::modding::ClientImageData,
    rect: [f32; 4],
    uv: [f32; 4],
) -> petramond_render::ClientOverlayImage {
    petramond_render::ClientOverlayImage {
        key: image.key,
        size: (image.width, image.height),
        rgba: image.rgba,
        revision: image.revision,
        recent_blits: image.recent_blits,
        rect,
        uv,
    }
}

pub(super) fn client_gui_owned_by(screen: AppScreen, owner: &str) -> bool {
    let AppScreen::ClientModGui(kind) = screen else {
        return false;
    };
    petramond_world::gui_state::kind_key(kind)
        .and_then(|key| key.split_once(':'))
        .is_some_and(|(namespace, _)| namespace == owner)
}

pub(super) fn append_layer(
    dst: &mut petramond_ui::DrawList,
    dst_images: &mut Vec<DocImageSource>,
    src: &petramond_ui::DrawList,
    images: &[DocImageSource],
) {
    // Overlay-tier batches must stay last in the composed list, so a layer can
    // only be appended while nothing has contributed an overlay tier yet. One
    // document is composed per list; a second would have to splice its base
    // tier in ahead of the first's overlay tier.
    debug_assert_eq!(
        dst.overlay_start,
        dst.batches.len(),
        "appending under an existing overlay tier would reorder the draw"
    );
    let vertex_base = dst.vertices.len() as u32;
    let image_base = dst_images.len() as u16;
    let overlay_start = dst.batches.len() + src.overlay_start;
    dst.vertices.extend_from_slice(&src.vertices);
    dst.batches.extend(src.batches.iter().map(|batch| {
        let mut batch = *batch;
        batch.start += vertex_base;
        if let petramond_ui::TexId::DocImage(index) = batch.tex {
            batch.tex = petramond_ui::TexId::DocImage(image_base + index);
        }
        batch
    }));
    dst.overlay_start = overlay_start;
    dst_images.extend_from_slice(images);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_gui_commands_are_context_and_owner_scoped() {
        let map = petramond_world::gui_state::intern_kind("map:screen").unwrap();
        let other = petramond_world::gui_state::intern_kind("other:screen").unwrap();
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

        assert!(client_ui_open_permitted(
            AppScreen::Game,
            AppScreen::Game,
            "map",
            &no_canvas
        ));
        // On the shell a launched mod opens over the title, and only there.
        assert!(client_ui_open_permitted(
            AppScreen::Title,
            AppScreen::Title,
            "map",
            &no_canvas
        ));
        assert!(!client_ui_open_permitted(
            AppScreen::WorldSelect,
            AppScreen::Title,
            "map",
            &no_canvas
        ));
        assert!(client_ui_open_permitted(
            AppScreen::ClientModGui(map),
            AppScreen::Game,
            "map",
            &no_canvas,
        ));
        assert!(!client_ui_open_permitted(
            AppScreen::ClientModGui(other),
            AppScreen::Game,
            "map",
            &no_canvas,
        ));
        assert!(client_ui_open_permitted(
            AppScreen::ClientCanvas,
            AppScreen::Game,
            "map",
            &canvas,
        ));
        assert!(!client_ui_open_permitted(
            AppScreen::ClientCanvas,
            AppScreen::Game,
            "other",
            &canvas,
        ));
        for screen in [
            AppScreen::Pause,
            AppScreen::Menu(petramond_world::gui_state::GuiKind::Inventory),
            AppScreen::Sleeping,
            AppScreen::Dead,
        ] {
            assert!(
                !client_ui_open_permitted(screen, AppScreen::Game, "map", &no_canvas),
                "{screen:?}"
            );
        }
        assert!(client_gui_owned_by(AppScreen::ClientModGui(map), "map"));
        assert!(!client_gui_owned_by(AppScreen::ClientModGui(map), "other"));
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
