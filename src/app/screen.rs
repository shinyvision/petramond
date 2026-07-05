#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum AppScreen {
    Title,
    WorldSelect,
    /// Per-world mod enablement for the selected world (opened from
    /// world-select; also hosts the relocated Delete World button).
    WorldSettings,
    CreateWorld,
    DeleteWorld,
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
                | AppScreen::Pause
        )
    }

    #[inline]
    #[cfg(test)]
    pub(super) fn inventory_open(self) -> bool {
        matches!(self, AppScreen::Inventory)
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
            AppScreen::Title
            | AppScreen::WorldSelect
            | AppScreen::WorldSettings
            | AppScreen::CreateWorld
            | AppScreen::DeleteWorld
            | AppScreen::Pause => GuiKind::Other,
        }
    }

    /// Whether the open menu is the chest screen — drives chest-specific click
    /// routing and the chest panel + storage grid in place of the crafting grid.
    #[inline]
    pub(super) fn is_chest(self) -> bool {
        matches!(self, AppScreen::Chest)
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
