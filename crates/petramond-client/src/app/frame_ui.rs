//! The frame's two UI layers.
//!
//! The SCENE layer is the HUD the view claims leave up: the hotbar document,
//! hearts and effects, the hurt vignette and the passive chat log. It draws
//! into the frame over the world, so a scene capture holds it. The WINDOW
//! layer is everything else on screen — every menu and shell screen, a mod's
//! document, canvas or overlays, the chat being typed — and is composited
//! onto the window only, after the frame's capture point. Which layer a piece
//! of UI belongs to is decided here and nowhere else.

use petramond::gui::{UiSnapshot, UiViewport};
use petramond_render::{ClientOverlayLayer, DocumentUiFrame, Renderer, UiFrame, UiLayers};
use petramond_world::gui_state::GuiKind;

use super::screen::AppScreen;
use super::session::Session;
use super::{client_mod_ui, App};

impl App {
    /// The screen as the scene sees it. On the window a screen hides the HUD
    /// and the aim marks it covers. A capture holds the scene the view claims
    /// describe, so while one is armed, or a presentation presents, a screen
    /// that is only UI reaches the window alone.
    pub(super) fn scene_screen(&self) -> AppScreen {
        let presenting = self
            .session
            .as_ref()
            .is_some_and(|session| session.game.in_presentation());
        if (self.capture_armed() || presenting) && self.screen.window_only() {
            AppScreen::Game
        } else {
            self.screen
        }
    }

    /// Solve the HUD document into the scene layer at `viewport`, with the
    /// passive chat log over it, or drop both when the HUD is hidden.
    pub(super) fn solve_scene_ui(&mut self, hud_visible: bool, viewport: UiViewport, now: f64) {
        self.composed_hud.clear();
        self.composed_hud_images.clear();
        if !hud_visible {
            self.hud_ui.deactivate();
            return;
        }
        let kind = GuiKind::Hotbar;
        self.hud_ui.ensure_active(kind);
        if let Some(Session {
            game,
            hotbar_notice,
            ..
        }) = self.session.as_mut()
        {
            let active = game.menu_read_model().inventory.active_slot();
            hotbar_notice.populate(
                game.held_tool_setting().map(|label| (active, label)),
                &mut game.notice,
                now,
                self.hud_ui.state_mut(),
            );
            self.hud_ui
                .state_mut()
                .set("active_slot", petramond_ui::UiValue::I32(active as i32));
        }
        self.hud_ui.frame_in(kind, viewport, now, None);
        self.hud_ui.refresh_doc_geometry();
        client_mod_ui::append_layer(
            &mut self.composed_hud,
            &mut self.composed_hud_images,
            &self.hud_ui.out().draw,
            self.hud_ui.image_sources(),
        );
        if self.scene_screen() != AppScreen::Chat {
            if let Some(session) = self.session.as_mut() {
                session
                    .chat
                    .draw(&mut self.composed_hud, viewport, false, now);
            }
        }
    }

    /// Compose the window's draw list: the open screen's document and the
    /// chat being typed, in that order. The window
    /// document's `(kind, viewport)`, or `None` when it has nothing to draw.
    pub(super) fn compose_window_ui(
        &mut self,
        doc_kind: Option<GuiKind>,
        viewport: UiViewport,
        now: f64,
    ) -> Option<(GuiKind, UiViewport)> {
        self.compose_document_ui(doc_kind.is_some());
        if self.screen == AppScreen::Chat {
            if let Some(session) = self.session.as_mut() {
                session
                    .chat
                    .draw(&mut self.composed_doc, viewport, true, now);
            }
        }
        match doc_kind {
            Some(kind) => Some((kind, viewport)),
            None if !self.composed_doc.is_empty() => Some((self.screen.gui_kind(), viewport)),
            None => None,
        }
    }

    /// Hand both layers to the renderer. `window` is the window's content,
    /// `scene` the HUD's; `false` = a layer's document is stale.
    pub(super) fn prepare_ui_layers(
        &self,
        renderer: &mut Renderer,
        mut scene: UiSnapshot,
        mut window: UiSnapshot,
        window_doc: Option<(GuiKind, UiViewport)>,
    ) -> bool {
        let none: (&[petramond::gui::DocSlot], &[petramond::gui::DocHook]) = (&[], &[]);
        let hud_doc = self.hud_ui.frame_stamp();
        let (hud_slots, hud_hooks) = match hud_doc {
            Some(_) => self.hud_ui.doc_geometry(),
            None => none,
        };
        scene.kind = GuiKind::Hotbar;
        scene.open = false;
        scene.cursor = None;
        let window_menu = window_doc.is_some() && self.doc_ui_kind().is_some();
        let (doc_slots, doc_hooks) = if window_menu {
            self.ui.doc_geometry()
        } else {
            none
        };
        if let Some((kind, _)) = window_doc {
            window.kind = kind;
        }
        // HUD content is the scene's, whatever else the window shows.
        window.health = None;
        window.effects.clear();
        window.hurt_flash = 0.0;
        window.heart_wiggle = None;
        let no_overlays = ClientOverlayLayer::default();
        renderer.prepare_ui_frame(UiLayers {
            scene: UiFrame {
                viewport: renderer.scene_ui_viewport(),
                document: hud_doc.map(|(kind, viewport)| DocumentUiFrame {
                    viewport,
                    kind,
                    draw: &self.composed_hud,
                    images: &self.composed_hud_images,
                    slots: hud_slots,
                    hooks: hud_hooks,
                }),
                content: &scene,
                client_overlays: &no_overlays,
                client_overlay_dim: false,
            },
            window: UiFrame {
                viewport: renderer.window_ui_viewport(),
                document: window_doc.map(|(kind, viewport)| DocumentUiFrame {
                    viewport,
                    kind,
                    draw: &self.composed_doc,
                    images: &self.composed_doc_images,
                    slots: doc_slots,
                    hooks: doc_hooks,
                }),
                content: &window,
                client_overlays: &self.client_overlays,
                client_overlay_dim: self.screen.client_canvas_open(),
            },
        })
    }
}
