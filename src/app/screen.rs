#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum AppScreen {
    Game,
    Inventory,
    /// The 3×3 crafting-table screen, opened by right-clicking a placed table.
    CraftingTable,
    /// The furnace screen, opened by right-clicking a placed furnace.
    Furnace,
    /// The chest screen (3×9 storage), opened by right-clicking a placed chest.
    Chest,
}

impl AppScreen {
    #[inline]
    pub fn gameplay_enabled(self) -> bool {
        matches!(self, AppScreen::Game)
    }

    #[inline]
    pub fn inventory_open(self) -> bool {
        matches!(self, AppScreen::Inventory)
    }

    /// Any slot-based menu (inventory or crafting table) is open — drives click
    /// routing and whether the panel UI is drawn.
    #[inline]
    pub fn ui_open(self) -> bool {
        !matches!(self, AppScreen::Game)
    }

    /// The crafting layout shown by the open menu. Defaults to the inventory
    /// layout when no menu is open (harmless: craft drawing/routing is gated on
    /// [`ui_open`](Self::ui_open)).
    #[inline]
    pub fn craft_kind(self) -> crate::render::CraftKind {
        match self {
            AppScreen::CraftingTable => crate::render::CraftKind::Table,
            _ => crate::render::CraftKind::Inventory,
        }
    }

    /// Whether the open menu is the furnace screen — drives furnace-specific click
    /// routing and the furnace panel/slots/gauges in place of the crafting grid.
    #[inline]
    pub fn is_furnace(self) -> bool {
        matches!(self, AppScreen::Furnace)
    }

    /// Whether the open menu is the chest screen — drives chest-specific click
    /// routing and the chest panel + storage grid in place of the crafting grid.
    #[inline]
    pub fn is_chest(self) -> bool {
        matches!(self, AppScreen::Chest)
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct CursorPolicy {
    pub grabbed: bool,
    pub visible: bool,
}

impl CursorPolicy {
    pub fn for_screen(screen: AppScreen) -> Self {
        let grabbed = screen.gameplay_enabled();
        Self {
            grabbed,
            visible: !grabbed,
        }
    }
}
