mod kind;

use crate::item::ItemStack;
use std::collections::BTreeMap;
use std::sync::Arc;

pub use kind::GuiKind;
pub use kind::{engine_kind_keys, intern_kind, intern_str, kind_key, resolve_kind};

pub const MAX_MENU_DRAG_SLOTS: usize =
    crate::container::MAX_CONTAINER_SLOTS + crate::inventory::TOTAL_SLOTS;

#[derive(Clone, Debug, PartialEq)]
pub enum GuiValue {
    F32(f32),
    I32(i32),
    Str(String),
    List(Vec<std::collections::BTreeMap<String, Self>>),
}

pub type GuiStateMap = BTreeMap<String, GuiValue>;

pub type WidgetId = &'static str;

pub fn empty_gui_state() -> Arc<GuiStateMap> {
    static EMPTY: std::sync::OnceLock<Arc<GuiStateMap>> = std::sync::OnceLock::new();
    EMPTY.get_or_init(|| Arc::new(GuiStateMap::new())).clone()
}

pub fn gui_state_set(map: &mut Arc<GuiStateMap>, key: String, value: GuiValue) {
    Arc::make_mut(map).insert(key, value);
}

pub fn gui_state_clear(map: &mut Arc<GuiStateMap>) {
    if !map.is_empty() {
        *map = empty_gui_state();
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ContainerView {
    pub slots: Vec<Option<ItemStack>>,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct HealthView {
    pub current: i32,
    pub max: i32,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum PointerButton {
    Primary,
    Secondary,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum MenuSlot {
    Inventory(usize),
    OffHand,
    CraftResult,
    Container(usize),
    Widget(WidgetId),
}

#[cfg(test)]
mod state_tests {
    use super::*;

    #[test]
    fn gui_state_set_get_clear_and_snapshot_cow() {
        let mut map = empty_gui_state();
        assert!(map.get("wheel:angle").is_none());

        gui_state_set(&mut map, "wheel:angle".into(), GuiValue::F32(1.5));
        assert_eq!(map.get("wheel:angle"), Some(&GuiValue::F32(1.5)));

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

        let a = map.clone();
        let b = map.clone();
        assert!(Arc::ptr_eq(&a, &b));

        gui_state_clear(&mut map);
        assert!(map.get("wheel:angle").is_none());
        assert!(map.get("wheel:result").is_none());
    }
}
