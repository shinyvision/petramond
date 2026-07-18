#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum AppScreen {
    Title,
    WorldSelect,
    /// Per-world mod enablement for the selected world (opened from
    /// world-select; also hosts the relocated Delete World button).
    WorldSettings,
    CreateWorld,
    DeleteWorld,
    /// The "Connect to Server" screen (address + player name).
    /// The connect worker runs while this screen is up.
    ConnectServer,
    /// The refused-join screen listing the server mods this client lacks;
    /// Back returns to [`ConnectServer`](AppScreen::ConnectServer).
    ModsMissing,
    /// The "Disconnected" screen: the session was torn down after a
    /// connection loss / server close; OK returns to the title.
    ConnectionLost,
    /// The Options root (Sound / Controls / Graphics), entered from the title
    /// or the pause menu (`App::options_from_pause` remembers which).
    Options,
    OptionsSound,
    /// The controls remap screen; while a binding is armed the App captures
    /// raw input (`App::remap`).
    OptionsControls,
    OptionsGraphics,
    Game,
    /// Chat input overlay: the world keeps ticking and the hotbar HUD stays
    /// visible, but gameplay controls are disabled while text is entered.
    Chat,
    Pause,
    /// A slot-bearing game menu over a live tick: the engine containers
    /// (inventory, crafting table, furnace, chest, furniture workbench) and
    /// mod GUIs, one screen variant for all of them. Carries which registered
    /// kind it draws; the open server session (`ContainerTarget`) speaks the
    /// same kind.
    Menu(crate::gui::GuiKind),
    /// A presentation-only client mod document. It releases the cursor and
    /// receives client-WASM UI events while the replicated world keeps
    /// running; no server menu session exists.
    ClientModGui(crate::gui::GuiKind),
    /// A presentation-only client mod's centered physical-pixel canvas. The
    /// concrete owner/image lives in `App::client_canvas`; this screen gates
    /// gameplay and releases the cursor without selecting a GUI document.
    ClientCanvas,
    /// The sleep overlay (bed interaction): the simulation KEEPS TICKING under
    /// it — the tick-owned sleep timer drives the fade and the wake — unlike
    /// `Pause`, which freezes the world.
    Sleeping,
    /// The death screen. The simulation keeps ticking (the world does not
    /// freeze around a corpse); ESC cannot close it — only respawn or
    /// save-and-quit leave.
    Dead,
}

impl AppScreen {
    #[inline]
    pub(super) fn gameplay_enabled(self) -> bool {
        matches!(self, AppScreen::Game)
    }

    #[inline]
    pub(super) fn shell_open(self) -> bool {
        matches!(
            self,
            AppScreen::Title
                | AppScreen::WorldSelect
                | AppScreen::WorldSettings
                | AppScreen::CreateWorld
                | AppScreen::DeleteWorld
                | AppScreen::ConnectServer
                | AppScreen::ModsMissing
                | AppScreen::ConnectionLost
                | AppScreen::Options
                | AppScreen::OptionsSound
                | AppScreen::OptionsControls
                | AppScreen::OptionsGraphics
                | AppScreen::Pause
        )
    }

    /// One of the Options screens (root or a category) is open.
    #[inline]
    pub(super) fn options_open(self) -> bool {
        matches!(
            self,
            AppScreen::Options
                | AppScreen::OptionsSound
                | AppScreen::OptionsControls
                | AppScreen::OptionsGraphics
        )
    }

    #[inline]
    #[cfg(test)]
    pub(super) fn inventory_open(self) -> bool {
        self == AppScreen::Menu(crate::gui::GuiKind::Inventory)
    }

    /// A gameplay overlay screen is up (sleep fade / death): the document UI
    /// owns input like a shell screen (controller-dispatched buttons), but the
    /// simulation keeps ticking underneath.
    #[inline]
    pub(super) fn overlay_open(self) -> bool {
        matches!(self, AppScreen::Sleeping | AppScreen::Dead)
    }

    /// Any slot-based menu (container or mod GUI) is open — drives click
    /// routing and whether the panel UI is drawn.
    #[inline]
    pub(super) fn ui_open(self) -> bool {
        matches!(self, AppScreen::Menu(_))
    }

    #[inline]
    pub(super) fn client_ui_open(self) -> bool {
        matches!(self, AppScreen::ClientModGui(_))
    }

    #[inline]
    pub(super) fn client_canvas_open(self) -> bool {
        matches!(self, AppScreen::ClientCanvas)
    }

    /// Which GUI this screen draws: the open menu's kind, or `Hotbar` for the
    /// HUD (gameplay). The single source of "which screen" for the data-driven
    /// GUI — it selects the document the runtime draws and hit-tests.
    #[inline]
    pub(super) fn gui_kind(self) -> crate::gui::GuiKind {
        use crate::gui::GuiKind;
        match self {
            AppScreen::Game => GuiKind::Hotbar,
            AppScreen::Chat => GuiKind::Hotbar,
            AppScreen::Menu(kind) => kind,
            AppScreen::ClientModGui(kind) => kind,
            AppScreen::Sleeping => GuiKind::Sleep,
            AppScreen::Dead => GuiKind::Death,
            AppScreen::Title
            | AppScreen::WorldSelect
            | AppScreen::WorldSettings
            | AppScreen::CreateWorld
            | AppScreen::DeleteWorld
            | AppScreen::ConnectServer
            | AppScreen::ModsMissing
            | AppScreen::ConnectionLost
            | AppScreen::Options
            | AppScreen::OptionsSound
            | AppScreen::OptionsControls
            | AppScreen::OptionsGraphics
            | AppScreen::ClientCanvas
            | AppScreen::Pause => GuiKind::Other,
        }
    }
}

/// Known polish gap: document text inputs don't request a Text (I-beam)
/// cursor yet, so Default is currently the only icon.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum CursorIcon {
    Default,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct CursorPolicy {
    pub grabbed: bool,
    pub visible: bool,
    pub icon: CursorIcon,
}

impl CursorPolicy {
    pub(super) fn for_screen(screen: AppScreen) -> Self {
        let grabbed = screen.gameplay_enabled();
        Self {
            grabbed,
            visible: !grabbed,
            icon: CursorIcon::Default,
        }
    }
}
