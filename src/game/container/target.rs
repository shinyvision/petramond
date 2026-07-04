use crate::mathh::IVec3;

/// What the open GUI is acting on — named for the thing being edited, not for the
/// screen. The app's `AppScreen` decides which screen is up; this decides which
/// block-entity (or the inventory-side craft grid) that screen reads and mutates.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub(in crate::game) enum ContainerTarget {
    /// No container GUI is editing anything (gameplay, or a screen that owns no
    /// block-entity yet).
    #[default]
    None,
    /// The 2×2 crafting grid embedded in the inventory screen.
    Inventory,
    /// The 3×3 crafting grid at a placed crafting table.
    Table,
    /// The furnace at this world position.
    Furnace(IVec3),
    /// The chest at this world position.
    Chest(IVec3),
    /// A furniture workbench. Like the crafting grid it owns no persistent block-entity
    /// — the single input block lives transiently on the menu and is returned to the
    /// inventory on close — so no position is needed.
    FurnitureWorkbench,
    /// A mod-defined GUI session (Phase 5). `pos` is the block it was opened
    /// from (`None` for a programmatic `GuiOpen`); it rides the session into
    /// every `gui_click` dispatch. The session's state lives in the world's
    /// GUI state map, cleared on open/close.
    ModGui {
        kind: crate::gui::GuiKind,
        pos: Option<IVec3>,
    },
}

impl ContainerTarget {
    /// The world position of the open chest, if a chest is the current target. The
    /// lid animation on `Game` reads this to know which chest to ease open.
    #[inline]
    pub(in crate::game) fn open_chest(self) -> Option<IVec3> {
        match self {
            ContainerTarget::Chest(pos) => Some(pos),
            _ => None,
        }
    }
}
