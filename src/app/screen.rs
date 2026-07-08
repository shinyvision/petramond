#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum AppScreen {
    Title,
    WorldSelect,
    /// Per-world mod enablement for the selected world (opened from
    /// world-select; also hosts the relocated Delete World button).
    WorldSettings,
    CreateWorld,
    DeleteWorld,
    /// The "Connect to Server" screen (address + player name; multiplayer
    /// Phase E2). The connect worker runs while this screen is up.
    ConnectServer,
    /// The refused-join screen listing the server mods this client lacks;
    /// Back returns to [`ConnectServer`](AppScreen::ConnectServer).
    ModsMissing,
    /// The "Disconnected" screen: the session was torn down after a
    /// connection loss / server close; OK returns to the title.
    ConnectionLost,
    Game,
    Pause,
    Inventory,
    /// The 3×3 crafting-table screen, opened by right-clicking a placed table.
    CraftingTable,
    /// The furnace screen, opened by right-clicking a placed furnace.
    Furnace,
    /// The chest screen (3×9 storage), opened by right-clicking a placed chest.
    Chest,
    /// The furniture-workbench screen (one input block + a grid of craftable results),
    /// opened by right-clicking a placed workbench.
    FurnitureWorkbench,
    /// A mod-defined GUI screen (widgets-only in Phase 5), opened by a block's
    /// `open_gui` interaction or a mod's `GuiOpen` call. Carries which
    /// registered kind it draws.
    ModGui(crate::gui::GuiKind),
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
                | AppScreen::Pause
        )
    }

    #[inline]
    #[cfg(test)]
    pub(super) fn inventory_open(self) -> bool {
        matches!(self, AppScreen::Inventory)
    }

    /// A gameplay overlay screen is up (sleep fade / death): the document UI
    /// owns input like a shell screen (controller-dispatched buttons), but the
    /// simulation keeps ticking underneath.
    #[inline]
    pub(super) fn overlay_open(self) -> bool {
        matches!(self, AppScreen::Sleeping | AppScreen::Dead)
    }

    /// Any slot-based menu (inventory or crafting table) is open — drives click
    /// routing and whether the panel UI is drawn.
    #[inline]
    pub(super) fn ui_open(self) -> bool {
        matches!(
            self,
            AppScreen::Inventory
                | AppScreen::CraftingTable
                | AppScreen::Furnace
                | AppScreen::Chest
                | AppScreen::FurnitureWorkbench
                | AppScreen::ModGui(_)
        )
    }

    /// Which GUI this screen draws: the open menu's kind, or `Hotbar` for the
    /// HUD (gameplay). The single source of "which screen" for the data-driven
    /// GUI — it selects the document the runtime draws and hit-tests.
    #[inline]
    pub(super) fn gui_kind(self) -> crate::gui::GuiKind {
        use crate::gui::GuiKind;
        match self {
            AppScreen::Game => GuiKind::Hotbar,
            AppScreen::Inventory => GuiKind::Inventory,
            AppScreen::CraftingTable => GuiKind::CraftingTable,
            AppScreen::Furnace => GuiKind::Furnace,
            AppScreen::Chest => GuiKind::Chest,
            AppScreen::FurnitureWorkbench => GuiKind::FurnitureWorkbench,
            AppScreen::ModGui(kind) => kind,
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
