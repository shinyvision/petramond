use serde::{Deserialize, Serialize};

use crate::mathh::IVec3;

use super::ItemSlotWire;

/// A container-menu slot identity on the wire — the message twin of
/// [`crate::gui::MenuSlot`], self-contained (widget ids travel as strings; the
/// server re-interns them).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum MenuSlotWire {
    Inventory(u32),
    CraftResult,
    FurnaceInput,
    FurnaceFuel,
    FurnaceOutput,
    Chest(u32),
    WorkbenchInput,
    WorkbenchResult(u32),
    Container(u32),
    Widget(String),
}

impl MenuSlotWire {
    pub(crate) fn from_menu_slot(slot: &crate::gui::MenuSlot) -> Self {
        use crate::gui::{FurnaceHit, MenuSlot, WorkbenchHit};
        match slot {
            MenuSlot::Inventory(i) => Self::Inventory(*i as u32),
            MenuSlot::CraftResult => Self::CraftResult,
            MenuSlot::Furnace(FurnaceHit::Input) => Self::FurnaceInput,
            MenuSlot::Furnace(FurnaceHit::Fuel) => Self::FurnaceFuel,
            MenuSlot::Furnace(FurnaceHit::Output) => Self::FurnaceOutput,
            MenuSlot::Chest(i) => Self::Chest(*i as u32),
            MenuSlot::Workbench(WorkbenchHit::Input) => Self::WorkbenchInput,
            MenuSlot::Workbench(WorkbenchHit::Result(i)) => Self::WorkbenchResult(*i as u32),
            MenuSlot::Container(i) => Self::Container(*i as u32),
            MenuSlot::Widget(id) => Self::Widget((*id).to_string()),
        }
    }

    pub(crate) fn to_menu_slot(&self) -> crate::gui::MenuSlot {
        use crate::gui::{FurnaceHit, MenuSlot, WorkbenchHit};
        match self {
            Self::Inventory(i) => MenuSlot::Inventory(*i as usize),
            Self::CraftResult => MenuSlot::CraftResult,
            Self::FurnaceInput => MenuSlot::Furnace(FurnaceHit::Input),
            Self::FurnaceFuel => MenuSlot::Furnace(FurnaceHit::Fuel),
            Self::FurnaceOutput => MenuSlot::Furnace(FurnaceHit::Output),
            Self::Chest(i) => MenuSlot::Chest(*i as usize),
            Self::WorkbenchInput => MenuSlot::Workbench(WorkbenchHit::Input),
            Self::WorkbenchResult(i) => MenuSlot::Workbench(WorkbenchHit::Result(*i as usize)),
            Self::Container(i) => MenuSlot::Container(*i as usize),
            Self::Widget(id) => MenuSlot::Widget(crate::gui::intern_str(id)),
        }
    }
}

/// One [`crate::gui::GuiValue`] on the wire.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) enum GuiValueWire {
    F32(f32),
    I32(i32),
    Str(String),
}

impl GuiValueWire {
    pub(crate) fn from_value(v: &crate::gui::GuiValue) -> Self {
        match v {
            crate::gui::GuiValue::F32(x) => Self::F32(*x),
            crate::gui::GuiValue::I32(x) => Self::I32(*x),
            crate::gui::GuiValue::Str(s) => Self::Str(s.clone()),
        }
    }

    pub(crate) fn into_value(self) -> crate::gui::GuiValue {
        match self {
            Self::F32(x) => crate::gui::GuiValue::F32(x),
            Self::I32(x) => crate::gui::GuiValue::I32(x),
            Self::Str(s) => crate::gui::GuiValue::Str(s),
        }
    }
}

/// The recipient's open menu-session target, with everything its screen
/// renders. Item slots are wire ids ([`ItemSlotWire`]), remapped at the
/// transport boundary.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub(crate) enum MenuTargetWire {
    /// No menu session is open (sent once when a session closes).
    #[default]
    None,
    /// Player crafting from the inventory screen. The result is transient
    /// server-owned menu state, not an inventory slot.
    Inventory { output: Option<ItemSlotWire> },
    /// Player crafting from a crafting table. Its recipe catalog includes
    /// both inventory and table recipes.
    Table { output: Option<ItemSlotWire> },
    Furnace {
        pos: IVec3,
        /// `[input, fuel, output]`.
        slots: [Option<ItemSlotWire>; 3],
        cook01: f32,
        burn01: f32,
    },
    Chest {
        pos: IVec3,
        slots: Vec<Option<ItemSlotWire>>,
    },
    Workbench {
        input: Option<ItemSlotWire>,
        /// `(wire item id, craftable now)` per offered recipe, row-major.
        results: Vec<(u8, bool)>,
    },
    ModGui {
        kind_key: String,
        pos: Option<IVec3>,
        /// The backing container's slots, `None` for a slot-less GUI.
        slots: Option<Vec<Option<ItemSlotWire>>>,
        /// The session's full state map — present ONLY when it changed since
        /// the last sync (`Arc` identity check server-side); `None` = keep.
        gui_state: Option<Vec<(String, GuiValueWire)>>,
    },
}

/// The recipient's menu-session view, sent inside a `TickUpdate` only when it
/// changed since the last one this session was sent (value compare; the
/// `gui_state` map compares by `Arc` identity).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct MenuSyncMsg {
    pub target: MenuTargetWire,
}
