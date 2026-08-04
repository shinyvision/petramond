//! GUI session-state vocabulary shared by the sim, server, net protocol, and
//! modding host.
//!
//! These types name logical GUI identities, session state values, and slot
//! roles a click resolves to. Nothing here owns renderer resources or layout;
//! the GUI-document runtime maps pixels to these identities, and the game menu
//! applies them on the tick.

mod kind;

use crate::item::ItemStack;
use std::collections::BTreeMap;
use std::sync::Arc;

pub use kind::GuiKind;
pub use kind::{intern_kind, intern_str, kind_key, resolve_kind};

/// Maximum distinct destination cells one pointer gesture can ship. The
/// largest supported menu is a 54-slot generic container plus all 36 player
/// inventory slots.
pub const MAX_MENU_DRAG_SLOTS: usize =
    crate::container::MAX_CONTAINER_SLOTS + crate::inventory::TOTAL_SLOTS;

/// One value of the open GUI session's state map: written by mods on the tick
/// (`GuiStateSet`), read per frame by the renderer for `label` text, `rotimage`
/// angles (radians, `F32`), and mod overlay fractions.
#[derive(Clone, Debug, PartialEq)]
pub enum GuiValue {
    F32(f32),
    I32(i32),
    Str(String),
}

/// The open GUI session's state map. `BTreeMap` for deterministic iteration;
/// snapshotted behind an `Arc` per frame (copy-on-write on the tick side).
pub type GuiStateMap = BTreeMap<String, GuiValue>;

/// A widget's stable id within its GUI manifest (interned — see
/// [`kind::intern_str`]) so [`MenuSlot`] stays `Copy`.
pub type WidgetId = &'static str;

/// An empty shared state map (the per-frame default when no mod GUI is open).
pub fn empty_gui_state() -> Arc<GuiStateMap> {
    static EMPTY: std::sync::OnceLock<Arc<GuiStateMap>> = std::sync::OnceLock::new();
    EMPTY.get_or_init(|| Arc::new(GuiStateMap::new())).clone()
}

/// Write a session state key (tick-side; copy-on-write against any
/// outstanding snapshot — at most one clone per snapshot taken). The map lives
/// per player session (`ConnectedPlayer::gui_state`).
pub fn gui_state_set(map: &mut Arc<GuiStateMap>, key: String, value: GuiValue) {
    Arc::make_mut(map).insert(key, value);
}

/// Reset a session state map for a fresh GUI session (the menu funnels call
/// this on open AND close, so a session can never read a predecessor's
/// values).
pub fn gui_state_clear(map: &mut Arc<GuiStateMap>) {
    if !map.is_empty() {
        *map = empty_gui_state();
    }
}

/// An open container's view: its slots row-major in document order (the
/// in-role `container` index). Owned (slot counts vary per document), rebuilt
/// per frame from the world. EVERY container uses this — engine chest, engine
/// furnace, and a pack's alike.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ContainerView {
    pub slots: Vec<Option<ItemStack>>,
}

/// The player's health for the HUD hearts: `current` and `max` in half-heart points
/// (a full heart is 2). `None` in a `UiSnapshot` hides the bar (spectator).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct HealthView {
    pub current: i32,
    pub max: i32,
}

/// Which pointer button a menu click used. Menu-click vocabulary (it rides the
/// wire in `PlayerAction::MenuClick` and drives slot mechanics), so it lives
/// with the session vocabulary rather than the client input layer.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum PointerButton {
    Primary,
    Secondary,
}

/// A click hit-tested to a concrete logical slot identity, the unit the App routes
/// through the deterministic container menu on the next tick.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum MenuSlot {
    /// A main inventory/hotbar slot (the 36-slot grid drawn under every panel).
    Inventory(usize),
    /// The real, take-only output of an accepted player-crafting request.
    CraftResult,
    /// A container's `container` role slot index, backed by the
    /// [`Container`](crate::container::Container) at the session's opening
    /// block. Slot semantics (filters, take-only) come from the document's
    /// slot nodes — so the engine chest/furnace and a pack's container are
    /// one identity here, and no engine content is named in this vocabulary.
    Container(usize),
    /// A manifest `button` widget (mod GUIs). Latches like every slot click;
    /// the tick dispatches it to the kind's owning mod as a `gui_click`.
    Widget(WidgetId),
}

#[cfg(test)]
mod state_tests {
    use super::*;

    /// The GUI state session contract (one state map per player session):
    /// set/get round-trips, clear resets to the shared empty map, and a held
    /// snapshot is a refcount bump that tick-side writes never mutate in place
    /// (copy-on-write) — which is also what makes the menu-sync `Arc`
    /// identity change detection sound.
    #[test]
    fn gui_state_set_get_clear_and_snapshot_cow() {
        let mut map = empty_gui_state();
        assert!(map.get("wheel:angle").is_none());

        gui_state_set(&mut map, "wheel:angle".into(), GuiValue::F32(1.5));
        assert_eq!(map.get("wheel:angle"), Some(&GuiValue::F32(1.5)));

        // A held snapshot keeps its values across later writes, and the write
        // lands on a FRESH allocation (identity change = "changed" on the wire).
        let snap = map.clone();
        gui_state_set(&mut map, "wheel:angle".into(), GuiValue::F32(2.0));
        gui_state_set(
            &mut map,
            "wheel:result".into(),
            GuiValue::Str("stick".into()),
        );
        assert_eq!(snap.get("wheel:angle"), Some(&GuiValue::F32(1.5)));
        assert_eq!(snap.get("wheel:result"), None);
        assert_eq!(map.get("wheel:angle"), Some(&GuiValue::F32(2.0)));
        assert!(
            !Arc::ptr_eq(&snap, &map),
            "a write under a snapshot re-allocates"
        );

        // Unchanged between snapshots = the same allocation (no per-frame copy).
        let a = map.clone();
        let b = map.clone();
        assert!(Arc::ptr_eq(&a, &b));

        gui_state_clear(&mut map);
        assert!(map.get("wheel:angle").is_none());
        assert!(map.get("wheel:result").is_none());
    }
}
