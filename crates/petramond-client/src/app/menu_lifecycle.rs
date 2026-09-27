use super::{App, AppScreen};
use crate::game::GameEvents;
use petramond_world::gui_state::GuiKind;

impl App {
    pub(super) fn toggle_inventory(&mut self) {
        if self.screen.ui_open() {
            self.close_menu();
        } else {
            self.open_inventory();
        }
    }

    pub(super) fn handle_open_screen_events(&mut self, events: &GameEvents) {
        if let Some(reason) = events.connection_lost.clone() {
            self.enter_connection_lost(reason);
            return;
        }
        if let Some((kind, anchor)) = events.open_gui {
            if self.screen.gameplay_enabled() || self.screen.ui_open() {
                self.open_gui(kind, anchor);
            }
        }
        if events.close_document_gui
            && matches!(self.screen, AppScreen::Menu(k) if k.is_registered())
        {
            self.close_menu();
        }
        if events.open_sleep && self.screen.gameplay_enabled() {
            self.set_screen(AppScreen::Sleeping);
        }
        if events.sleep_ended && self.screen == AppScreen::Sleeping {
            self.set_screen(AppScreen::Game);
        }
        if events.player_died {
            if self.screen.ui_open() {
                if let Some(session) = self.session.as_mut() {
                    session.game.close_open_menu();
                }
            }
            self.set_screen(AppScreen::Dead);
        }
        if events.respawned && self.screen == AppScreen::Dead {
            self.set_screen(AppScreen::Game);
        }
    }

    pub(super) fn cancel_sleep(&mut self) {
        if let Some(session) = self.session.as_mut() {
            session.game.request_wake();
        }
        self.set_screen(AppScreen::Game);
    }

    fn open_inventory(&mut self) {
        let creative = self
            .session
            .as_ref()
            .is_some_and(|session| session.game.creative_mode());
        self.set_screen(AppScreen::Menu(if creative {
            GuiKind::Creative
        } else {
            GuiKind::Inventory
        }));
        if let Some(session) = self.session.as_mut() {
            session.game.request_open_inventory();
        }
    }

    fn open_gui(&mut self, kind: GuiKind, anchor: Option<petramond::menu::MenuAnchor>) {
        self.set_screen(AppScreen::Menu(kind));
        if let Some(session) = self.session.as_mut() {
            session.game.open_gui_screen(kind, anchor);
        }
    }

    pub(super) fn close_menu(&mut self) {
        if let Some(session) = self.session.as_mut() {
            session.game.cancel_pending_paste();
            session.game.close_open_menu();
        }
        self.set_screen(AppScreen::Game);
    }
}
