//! The crafting-station registry.
//!
//! A station is the context a player-crafting recipe requires, identified by
//! the [`GuiKind`] whose open menu session admits it. The engine ships two
//! (`petramond:inventory`, `petramond:crafting_table`); packs ADD stations by
//! naming a namespaced key in a `recipes.json` `station` field — the same
//! key an `open_gui` block interaction uses, so a pack workbench is pure
//! data: block row opens the kind, recipe rows require it, and the engine
//! runs the ordinary crafting session (browser, planner, output slot) for it.
//! Like every interning registry, registration is process-wide and ids are
//! session-scoped; the stable identity is the key string.

use std::sync::Mutex;

use crate::gui::GuiKind;

/// The minimum context a player-crafting recipe requires.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct CraftingStation(GuiKind);

/// Pack-registered station kinds (the engine pair lives in the consts).
static MOD_STATIONS: Mutex<Vec<GuiKind>> = Mutex::new(Vec::new());

#[allow(non_upper_case_globals)]
impl CraftingStation {
    pub const Inventory: CraftingStation = CraftingStation(GuiKind::Inventory);
    pub const CraftingTable: CraftingStation = CraftingStation(GuiKind::CraftingTable);

    pub const INVENTORY_KEY: &'static str = "petramond:inventory";
    pub const CRAFTING_TABLE_KEY: &'static str = "petramond:crafting_table";

    /// Resolve `key` to its station, REGISTERING a namespaced non-engine key
    /// on first sight (the recipe-loading path — a recipe declaring a station
    /// is what brings it into existence, on server and joining client alike).
    pub fn from_key(key: &str) -> Option<Self> {
        match key {
            Self::INVENTORY_KEY => Some(Self::Inventory),
            Self::CRAFTING_TABLE_KEY => Some(Self::CraftingTable),
            _ => {
                let kind = crate::gui::intern_kind(key)?;
                let mut stations = MOD_STATIONS.lock().unwrap();
                if !stations.contains(&kind) {
                    stations.push(kind);
                }
                Some(Self(kind))
            }
        }
    }

    /// The station whose menu session `kind` opens, if `kind` is one —
    /// engine pair or a registered pack station. Never registers.
    pub fn of_kind(kind: GuiKind) -> Option<Self> {
        match kind {
            GuiKind::Inventory => Some(Self::Inventory),
            GuiKind::CraftingTable => Some(Self::CraftingTable),
            _ => MOD_STATIONS
                .lock()
                .unwrap()
                .contains(&kind)
                .then_some(Self(kind)),
        }
    }

    /// The GUI kind whose open session admits this station's recipes.
    pub fn gui_kind(self) -> GuiKind {
        self.0
    }

    pub fn key(self) -> &'static str {
        match self {
            Self::Inventory => Self::INVENTORY_KEY,
            Self::CraftingTable => Self::CRAFTING_TABLE_KEY,
            Self(kind) => crate::gui::kind_key(kind).unwrap_or("?"),
        }
    }

    /// Whether this open context admits a recipe with minimum station
    /// `required`. A station admits exactly its own tier; the crafting table
    /// — the engine's general station — also admits the bare-hands inventory
    /// tier. A pack workbench deliberately does NOT (per Rachel): its
    /// browser lists only its own recipes.
    pub fn admits(self, required: Self) -> bool {
        self == required || (self == Self::CraftingTable && required == Self::Inventory)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn engine_keys_resolve_and_unknown_or_bare_keys_are_refused() {
        assert_eq!(
            CraftingStation::from_key("petramond:inventory"),
            Some(CraftingStation::Inventory)
        );
        assert_eq!(
            CraftingStation::from_key("petramond:crafting_table"),
            Some(CraftingStation::CraftingTable)
        );
        assert_eq!(CraftingStation::from_key("petramond:not_a_station"), None);
        assert_eq!(CraftingStation::from_key("bare_name"), None);
    }

    #[test]
    fn pack_stations_register_once_and_resolve_by_kind() {
        let a = CraftingStation::from_key("stationtest:bench").expect("namespaced key registers");
        let b = CraftingStation::from_key("stationtest:bench").unwrap();
        assert_eq!(a, b);
        assert_eq!(a.key(), "stationtest:bench");
        assert_eq!(CraftingStation::of_kind(a.gui_kind()), Some(a));
        // A GUI kind never declared as a station is not one.
        let plain = crate::gui::intern_kind("stationtest:not_a_bench").unwrap();
        assert_eq!(CraftingStation::of_kind(plain), None);
    }

    #[test]
    fn pack_stations_admit_only_their_own_tier() {
        let bench = CraftingStation::from_key("stationtest:admits_bench").unwrap();
        assert!(bench.admits(bench));
        assert!(!bench.admits(CraftingStation::Inventory));
        assert!(!bench.admits(CraftingStation::CraftingTable));
        assert!(!CraftingStation::CraftingTable.admits(bench));
        assert!(!CraftingStation::Inventory.admits(bench));
        // The engine pair keeps its shipped behavior: the table admits both
        // engine tiers, the inventory only its own.
        assert!(CraftingStation::CraftingTable.admits(CraftingStation::Inventory));
        assert!(CraftingStation::CraftingTable.admits(CraftingStation::CraftingTable));
        assert!(CraftingStation::Inventory.admits(CraftingStation::Inventory));
        assert!(!CraftingStation::Inventory.admits(CraftingStation::CraftingTable));
    }
}
