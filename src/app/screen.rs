#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum AppScreen {
    Game,
    Inventory,
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
