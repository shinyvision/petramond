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
        // The server became unreachable (host thread crash, remote server
        // close / connection loss): tear the session down WITHOUT saving and
        // land on the Disconnected screen. Nothing else this frame's events
        // carry can matter — the world they refer to is gone.
        if let Some(reason) = events.connection_lost.clone() {
            self.enter_connection_lost(reason);
            return;
        }
        // A GUI session the server opened for us this frame — a block
        // interaction (engine container or mod `open_gui` row) or a mod's
        // `GuiOpen` request; one lane for every kind.
        if let Some((kind, anchor)) = events.open_gui {
            if self.screen.gameplay_enabled() || self.screen.ui_open() {
                self.open_gui(kind, anchor);
            }
        }
        // A mod's `GuiClose` closes only an open MOD GUI (engine containers
        // are not closable from mods).
        if events.close_document_gui
            && matches!(self.screen, AppScreen::Menu(k) if k.is_registered())
        {
            self.close_menu();
        }
        // Right-clicking a bed starts the sleep overlay.
        if events.open_sleep && self.screen.gameplay_enabled() {
            self.set_screen(AppScreen::Sleeping);
        }
        // The tick ended the sleep (completed or wake applied): drop the
        // overlay. A cancel via ESC/button already left the screen — this
        // then no-ops.
        if events.sleep_ended && self.screen == AppScreen::Sleeping {
            self.set_screen(AppScreen::Game);
        }
        // Death overrides whatever is up (gameplay, a container, the sleep
        // overlay); an open container menu is closed properly first so its
        // cursor stack and edit target are cleaned up on the tick.
        if events.player_died {
            if self.screen.ui_open() {
                if let Some(session) = self.session.as_mut() {
                    session.game.close_open_menu();
                }
            }
            self.set_screen(AppScreen::Dead);
        }
        // The tick applied the respawn: back to gameplay.
        if events.respawned && self.screen == AppScreen::Dead {
            self.set_screen(AppScreen::Game);
        }
    }

    /// Cancel an in-progress sleep (ESC or the "Leave bed" button): ask the
    /// tick to wake the player beside the bed and drop the overlay now.
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

    /// Open the screen for a server-opened GUI session — any kind, engine
    /// container or mod GUI. `anchor` is the block or mob it opened on, if any.
    fn open_gui(&mut self, kind: GuiKind, anchor: Option<petramond::menu::MenuAnchor>) {
        self.set_screen(AppScreen::Menu(kind));
        if let Some(session) = self.session.as_mut() {
            session.game.open_gui_screen(kind, anchor);
        }
    }

    /// Close any open menu: recover transient cursor/output/input stacks and
    /// drop back to gameplay. The chest-close SOUND is event-driven now: the
    /// server's viewer release emits a positional `ChestClosed` world event
    /// on the tick this close lands on (so every observer hears it, at the
    /// chest).
    pub(super) fn close_menu(&mut self) {
        if let Some(session) = self.session.as_mut() {
            session.game.cancel_pending_paste();
            session.game.close_open_menu();
        }
        self.set_screen(AppScreen::Game);
    }
}
