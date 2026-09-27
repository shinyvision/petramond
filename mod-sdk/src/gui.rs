use mod_api::{ContainerAddress, GuiValue, GuiViewerData, PlayerId};

use crate::__rt::host_fn;

host_fn! {
    /// Writes a key in the open GUI session's state map. Labels bound to the key redraw. `rotimage`
    /// reads angle in radians from an `F32`. A framed image/button reads its frame from an `I32` at
    /// `bind.frame`. Keys are mod-local to one session, cleared on open/close.
    ///
    /// Session is whoever the dispatch is for, the clicking player on `gui_click`. Tick systems
    /// act for nobody, so this call refuses and disables the mod there. Live gauges and other
    /// tick-driven publishers need [`gui_state_set_for`] on each of [`gui_viewers`].
    pub fn gui_state_set(key: &str, value: GuiValue) => GuiStateSet { key: key.into(), value }
}

host_fn! {
    pub fn gui_state_set_for(player_id: PlayerId, key: &str, value: GuiValue) -> bool
        => GuiStateSetFor { player_id, key: key.into(), value } => Bool
}

host_fn! {
    pub fn gui_viewers() -> Vec<GuiViewerData> => GuiViewers => GuiViewers
}

host_fn! {
    pub fn gui_state_get(key: &str) -> Option<GuiValue> => GuiStateGet { key: key.into() } => GuiValue
}

host_fn! {
    pub fn gui_state_get_for(player_id: PlayerId, key: &str) -> Option<GuiValue>
        => GuiStateGetFor { player_id, key: key.into() } => GuiValue
}

host_fn! {
    pub fn gui_open(kind_key: &str, at: Option<impl Into<ContainerAddress>>) -> bool
        => GuiOpen { kind_key: kind_key.into(), at: at.map(Into::into) } => Bool
}

host_fn! {
    pub fn gui_open_for(
        player_id: PlayerId,
        kind_key: &str,
        at: Option<impl Into<ContainerAddress>>,
    ) -> bool
        => GuiOpenFor { player_id, kind_key: kind_key.into(), at: at.map(Into::into) } => Bool
}

host_fn! {
    pub fn gui_close() => GuiClose
}

host_fn! {
    pub fn gui_close_for(player_id: PlayerId) -> bool => GuiCloseFor { player_id } => Bool
}
