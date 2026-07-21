/// Secondary-use capability declared by a block's data row. This answers only
/// "what use action is available"; the tick-side gameplay code still applies the
/// concrete world mutation or menu request. Parsed from the row's
/// `interaction` field: a bare action name (engine GUI openers resolve to
/// their frozen `GuiKind`s), or `{"open_gui": "mod_id:name"}` for a
/// mod-defined GUI (see `block::load`).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum BlockInteraction {
    None,
    /// Right-click opens the GUI registered under this kind. Engine container
    /// kinds and mod kinds ride the same lane; what the session means per
    /// kind (block-entity storage, crafting station, machine gauges) lives
    /// behind the `GuiKind` contract lookup, not here.
    OpenGui(crate::gui::GuiKind),
    ToggleDoor,
    /// Right-click puts the player to sleep in this block (a bed): sets the
    /// spawn point beside it and starts the sleep fade (see `game::bed`).
    Sleep,
}

/// Whether a block's BUILT-IN interaction claims a use click made while
/// `sneaking`. The engine's built-in consumers (GUI openers, doors, beds) are
/// plain right-click consumers: they pass on sneak clicks, which is what
/// keeps sneak-to-place against a chest working. ONE rule shared by the
/// server's interact dispatch and the client's jab / place-ghost prediction —
/// the two sides must agree on who claims a click, or the ghost and the jab
/// drift from the authoritative outcome.
pub(crate) fn builtin_claims_click(block: crate::block::Block, sneaking: bool) -> bool {
    !sneaking && block.interaction() != BlockInteraction::None
}
