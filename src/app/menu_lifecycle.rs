use super::{App, AppScreen};
use crate::game::GameEvents;
use crate::mathh::IVec3;

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
        // Right-clicking a placed crafting table opens its 3x3 screen.
        if events.open_crafting_table && self.screen.gameplay_enabled() {
            self.open_crafting_table();
        }
        // Right-clicking a placed furnace opens its screen at that position.
        if let Some(pos) = events.open_furnace {
            if self.screen.gameplay_enabled() {
                self.open_furnace(pos);
            }
        }
        // Right-clicking a placed chest opens its screen at that position.
        if let Some(pos) = events.open_chest {
            if self.screen.gameplay_enabled() {
                self.open_chest(pos);
            }
        }
        // Right-clicking a placed furniture workbench opens its screen.
        if events.open_furniture_workbench.is_some() && self.screen.gameplay_enabled() {
            self.open_furniture_workbench();
        }
        // A block's `open_gui` interaction or a mod's `GuiOpen` request.
        if let Some((kind, pos)) = events.open_mod_gui {
            if self.screen.gameplay_enabled() {
                self.open_mod_gui(kind, pos);
            }
        }
        // A mod's `GuiClose` closes only an open MOD GUI (engine containers
        // are not closable from mods).
        if events.close_mod_gui && matches!(self.screen, AppScreen::ModGui(_)) {
            self.close_menu();
        }
        // Right-clicking a bed starts the sleep overlay.
        if events.open_sleep && self.screen.gameplay_enabled() {
            self.screen = AppScreen::Sleeping;
            self.pointer.release_for_menu();
        }
        // The tick ended the sleep (completed or wake applied): drop the
        // overlay. A cancel via ESC/button already left the screen — this
        // then no-ops.
        if events.sleep_ended && matches!(self.screen, AppScreen::Sleeping) {
            self.screen = AppScreen::Game;
            self.pointer.grab_for_gameplay();
        }
        // Death overrides whatever is up (gameplay, a container, the sleep
        // overlay); an open container menu is closed properly first so its
        // cursor stack and edit target are cleaned up on the tick.
        if events.player_died {
            if self.screen.ui_open() {
                if let Some(game) = self.game.as_mut() {
                    game.close_open_menu();
                }
            }
            self.screen = AppScreen::Dead;
            self.pointer.release_for_menu();
        }
        // The tick applied the respawn: back to gameplay.
        if events.respawned && matches!(self.screen, AppScreen::Dead) {
            self.screen = AppScreen::Game;
            self.pointer.grab_for_gameplay();
        }
    }

    /// Cancel an in-progress sleep (ESC or the "Leave bed" button): ask the
    /// tick to wake the player beside the bed and drop the overlay now.
    pub(super) fn cancel_sleep(&mut self) {
        if let Some(game) = self.game.as_mut() {
            game.request_wake();
        }
        self.screen = AppScreen::Game;
        self.pointer.grab_for_gameplay();
    }

    fn open_inventory(&mut self) {
        self.enter_menu(AppScreen::Inventory);
        if let Some(game) = self.game.as_mut() {
            game.open_crafting(2);
        }
    }

    /// Open the 3x3 crafting-table screen (after right-clicking a placed table).
    fn open_crafting_table(&mut self) {
        self.enter_menu(AppScreen::CraftingTable);
        if let Some(game) = self.game.as_mut() {
            game.open_crafting(3);
        }
    }

    /// Open the furnace screen for the furnace at `pos` (after right-clicking it).
    fn open_furnace(&mut self, pos: IVec3) {
        self.enter_menu(AppScreen::Furnace);
        if let Some(game) = self.game.as_mut() {
            game.open_furnace_screen(pos);
        }
    }

    /// Open the chest screen for the chest at `pos` (after right-clicking it).
    fn open_chest(&mut self, pos: IVec3) {
        self.enter_menu(AppScreen::Chest);
        if let Some(game) = self.game.as_mut() {
            game.open_chest_screen(pos);
        }
    }

    /// Open the furniture-workbench screen (after right-clicking a placed workbench).
    fn open_furniture_workbench(&mut self) {
        self.enter_menu(AppScreen::FurnitureWorkbench);
        if let Some(game) = self.game.as_mut() {
            game.open_workbench_screen();
        }
    }

    /// Open a mod GUI screen (after a block's `open_gui` interaction or a
    /// mod's `GuiOpen` request). `pos` is the opening block, if any.
    fn open_mod_gui(&mut self, kind: crate::gui::GuiKind, pos: Option<IVec3>) {
        self.enter_menu(AppScreen::ModGui(kind));
        if let Some(game) = self.game.as_mut() {
            game.open_mod_gui_screen(kind, pos);
        }
    }

    /// Shared menu-open bookkeeping: release the pointer grab, show + recenter the
    /// cursor next tick, and clear any stale click streak so the first click
    /// can't register a phantom double.
    fn enter_menu(&mut self, screen: AppScreen) {
        self.screen = screen;
        self.pointer.release_for_menu();
        self.gui_router.reset_click_streak();
    }

    /// Close any open menu: return crafting-grid items to the inventory, drop back
    /// to gameplay, and re-grab the pointer. The chest-close SOUND is event-driven
    /// now: the server's viewer release emits a positional `ChestClosed` world
    /// event on the tick this close lands on (so every observer hears it, at
    /// the chest).
    fn close_menu(&mut self) {
        if let Some(game) = self.game.as_mut() {
            game.close_open_menu();
        }
        self.screen = AppScreen::Game;
        self.pointer.grab_for_gameplay();
    }

    pub(super) fn close_screen(&mut self) -> bool {
        if matches!(self.screen, AppScreen::Chat) {
            self.chat.clear_draft(super::now_seconds());
            self.screen = AppScreen::Game;
            self.pointer.grab_for_gameplay();
            true
        } else if self.screen.ui_open() {
            self.close_menu();
            true
        } else if matches!(self.screen, AppScreen::Sleeping) {
            self.cancel_sleep();
            true
        } else if matches!(self.screen, AppScreen::Dead) {
            // Death cannot be escaped — only the screen's buttons leave.
            true
        } else if matches!(self.screen, AppScreen::Game) {
            self.open_pause();
            true
        } else if matches!(self.screen, AppScreen::Pause) {
            self.resume_game();
            true
        } else if matches!(self.screen, AppScreen::CreateWorld | AppScreen::DeleteWorld) {
            self.screen = AppScreen::WorldSelect;
            self.pointer.release_for_menu();
            true
        } else if matches!(self.screen, AppScreen::WorldSettings) {
            self.world_settings = None;
            self.screen = AppScreen::WorldSelect;
            self.pointer.release_for_menu();
            true
        } else if matches!(self.screen, AppScreen::ConnectServer) {
            self.cancel_connect();
            self.screen = AppScreen::Title;
            self.pointer.release_for_menu();
            true
        } else if matches!(self.screen, AppScreen::ModsMissing) {
            // Back to the connect screen, attempted address preserved.
            self.reopen_connect_server();
            true
        } else if matches!(
            self.screen,
            AppScreen::ConnectionLost | AppScreen::WorldSelect
        ) {
            self.screen = AppScreen::Title;
            self.pointer.release_for_menu();
            true
        } else {
            false
        }
    }
}
