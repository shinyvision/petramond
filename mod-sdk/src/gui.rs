//! Mod GUI documents: the session state map and programmatic open/close.
//!
//! Pack-local image art is DECLARED by the document: an `image` or
//! image-backed `button` node names a sheet beside its `*.gui.json` (plus an
//! optional `frames` grid), and the host resolves and uploads it — a mod
//! never ships pixels over the ABI. What the mod drives is the FRAME: publish
//! [`GuiValue::I32`] through [`gui_state_set`]/[`gui_state_set_for`] into the
//! key the node binds as `bind.frame` to hold that frame (a smelting
//! progress flame, a charge meter). A bound frame is authoritative over the
//! sheet's own `fps`, which cycles on its own while nothing is bound.

use mod_api::{ContainerAddress, GuiValue, GuiViewerData, PlayerId};

use crate::__rt::host_fn;

host_fn! {
    /// Write a key of the open GUI session's state map (labels bound to the key
    /// redraw; `rotimage` reads its angle in radians from an `F32`; a framed
    /// `image`/image-backed `button` sheet reads its held frame from an `I32`
    /// at its `bind.frame` key). Keys are
    /// mod-local — the map belongs to one GUI session and clears on open/close.
    ///
    /// WHICH session is the dispatch's actor (the clicking player for a
    /// `gui_click`); a TICK SYSTEM acts for nobody, so the call refuses there
    /// (and the mod is disabled). Anything publishing from a tick — which is
    /// every live gauge — wants [`gui_state_set_for`] over [`gui_viewers`].
    pub fn gui_state_set(key: &str, value: GuiValue) => GuiStateSet { key: key.into(), value }
}

host_fn! {
    /// Write a key of ONE session's GUI state map — the per-player form of
    /// [`gui_state_set`]. `false` = no such connected session.
    ///
    /// Address it with [`gui_viewers`]: publish a machine's gauges to the
    /// sessions that have THAT machine open, and two players at two machines
    /// stop overwriting each other's readings. It also settles the flat key
    /// space, because one session holds one open GUI.
    pub fn gui_state_set_for(player_id: PlayerId, key: &str, value: GuiValue) -> bool
        => GuiStateSetFor { player_id, key: key.into(), value } => Bool
}

host_fn! {
    /// Every connected session with a mod GUI open right now, in session
    /// order: who, which kind, and the block or mob it was opened on.
    ///
    /// This is the "who is looking" question, answered by the engine rather
    /// than tracked mod-side off `container_opened`/`container_closed` — those
    /// name no player, and a mod counting them has to be right about
    /// disconnects and unloads forever to stay in step.
    pub fn gui_viewers() -> Vec<GuiViewerData> => GuiViewers => GuiViewers
}

host_fn! {
    /// Read a key of the acting player's GUI state map (`None` = absent).
    /// Refused in an actor-less dispatch — use [`gui_state_get_for`] there.
    pub fn gui_state_get(key: &str) -> Option<GuiValue> => GuiStateGet { key: key.into() } => GuiValue
}

host_fn! {
    /// Read a key of ONE session's GUI state map. `None` = absent, or no such
    /// connected session.
    pub fn gui_state_get_for(player_id: PlayerId, key: &str) -> Option<GuiValue>
        => GuiStateGetFor { player_id, key: key.into() } => GuiValue
}

host_fn! {
    /// Ask the app shell to open the mod GUI registered under `kind_key` (a baked
    /// manifest or an `open_gui` block row must have registered it). The screen
    /// opens after this tick, replacing an existing menu when present.
    ///
    /// `at` anchors the session, and the anchor's storage backs the document's
    /// `container` slots: a block position (`Some([x, y, z])`) or a live mob
    /// (`Some(ContainerAddress::Mob(id))`, its carried slots; the session ends
    /// when the mob leaves the world). An unanchored open is
    /// `None::<ContainerAddress>`. `false` = unknown/non-mod kind or no such
    /// mob. Opens for the dispatch's actor; refused in an actor-less
    /// dispatch — use [`gui_open_for`] there.
    pub fn gui_open(kind_key: &str, at: Option<impl Into<ContainerAddress>>) -> bool
        => GuiOpen { kind_key: kind_key.into(), at: at.map(Into::into) } => Bool
}

host_fn! {
    /// [`gui_open`] for a NAMED session. `false` also when no such session
    /// is connected.
    pub fn gui_open_for(
        player_id: PlayerId,
        kind_key: &str,
        at: Option<impl Into<ContainerAddress>>,
    ) -> bool
        => GuiOpenFor { player_id, kind_key: kind_key.into(), at: at.map(Into::into) } => Bool
}

host_fn! {
    /// Close the acting player's open mod GUI (a no-op if none is open).
    /// Refused in an actor-less dispatch — use [`gui_close_for`] there.
    pub fn gui_close() => GuiClose
}

host_fn! {
    /// Close a NAMED session's open mod GUI. `false` = no such connected
    /// session.
    pub fn gui_close_for(player_id: PlayerId) -> bool => GuiCloseFor { player_id } => Bool
}
