use crate::gui::GuiKind;
use crate::mathh::IVec3;

/// What the open GUI is acting on — named for the thing being edited, not for the
/// screen. The app's `AppScreen` decides which screen is up; this decides which
/// block-entity or transient station session that screen reads and mutates.
///
/// ONE shape for every kind: engine containers and mod GUIs ride the same
/// `kind + pos` session identity. What the session means per kind (a
/// block-entity container, a transient crafting station, machine gauges) is
/// resolved by the per-kind lookups beside the menu, never by growing this
/// enum.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub(crate) enum ContainerTarget {
    /// No container GUI is editing anything (gameplay, or a screen that owns no
    /// block-entity yet).
    #[default]
    None,
    /// The GUI session for `kind`. `pos` is the world cell the session is
    /// anchored on: the block it was opened from for block-backed kinds
    /// (canonicalized to the container anchor for mod kinds), `None` for
    /// transient stations (inventory/table crafting, the furniture workbench)
    /// and programmatic `GuiOpen`s. It rides the session into every
    /// `gui_click` dispatch.
    Gui { kind: GuiKind, pos: Option<IVec3> },
}

impl ContainerTarget {
    /// The open session's GUI kind, if any.
    #[inline]
    pub(crate) fn kind(self) -> Option<GuiKind> {
        match self {
            ContainerTarget::None => None,
            ContainerTarget::Gui { kind, .. } => Some(kind),
        }
    }

    /// Whether kind `kind`'s session edits a block-backed `Container` at its
    /// `pos` (the chest/furnace block entities, and mod GUIs opened from a
    /// block). Transient station kinds keep their stacks on the menu itself.
    #[inline]
    pub(crate) fn kind_block_backed(kind: GuiKind) -> bool {
        kind == GuiKind::Chest || kind == GuiKind::Furnace || kind.is_mod()
    }
}
