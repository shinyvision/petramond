/// Secondary-use capability declared by a block's data row. This answers only
/// "what use action is available"; the tick-side gameplay code still applies the
/// concrete world mutation or menu request. Parsed from the row's
/// `interaction` field: a bare action name, or `{"open_gui": "mod_id:name"}`
/// for a mod-defined GUI (see `block::load`).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum BlockInteraction {
    None,
    OpenCraftingTable,
    OpenFurnace,
    OpenChest,
    OpenFurnitureWorkbench,
    ToggleDoor,
    /// Right-click puts the player to sleep in this block (a bed): sets the
    /// spawn point beside it and starts the sleep fade (see `game::bed`).
    Sleep,
    /// Right-click opens the mod GUI registered under this kind.
    OpenModGui(crate::gui::GuiKind),
}
