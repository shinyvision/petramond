//! Mod GUI documents: the session state map and programmatic open/close.

use mod_api::GuiValue;

use crate::__rt::host_fn;

host_fn! {
    /// Write a key of the open GUI session's state map (labels bound to the key
    /// redraw; `rotimage` reads its angle in radians from an `F32`). Keys are
    /// mod-local — the map belongs to one GUI session and clears on open/close.
    pub fn gui_state_set(key: &str, value: GuiValue) => GuiStateSet { key: key.into(), value }
}

host_fn! {
    /// Read a key of the GUI state map (`None` = absent).
    pub fn gui_state_get(key: &str) -> Option<GuiValue> => GuiStateGet { key: key.into() } => GuiValue
}

host_fn! {
    /// Ask the app shell to open the mod GUI registered under `kind_key` (a baked
    /// manifest or an `open_gui` block row must have registered it). The screen
    /// opens after this tick, only from gameplay. `false` = unknown/non-mod kind.
    pub fn gui_open(kind_key: &str) -> bool => GuiOpen { kind_key: kind_key.into() } => Bool
}

host_fn! {
    /// Close the open mod GUI (a no-op if none is open).
    pub fn gui_close() => GuiClose
}
