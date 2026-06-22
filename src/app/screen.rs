#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum AppScreen {
    Game,
    Inventory,
    /// The 3×3 crafting-table screen, opened by right-clicking a placed table.
    CraftingTable,
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
