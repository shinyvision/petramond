use serde::{Deserialize, Serialize};

use crate::mathh::IVec3;

use super::ItemSlotWire;

/// A container-menu slot identity on the wire — the message twin of
/// [`crate::gui_state::MenuSlot`], self-contained (widget ids travel as strings; the
/// server re-interns them).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum MenuSlotWire {
    Inventory(u32),
    CraftResult,
    Container(u32),
    Widget(String),
}

impl MenuSlotWire {
    pub fn from_menu_slot(slot: &crate::gui_state::MenuSlot) -> Self {
        use crate::gui_state::MenuSlot;
        match slot {
            MenuSlot::Inventory(i) => Self::Inventory(*i as u32),
            MenuSlot::CraftResult => Self::CraftResult,
            MenuSlot::Container(i) => Self::Container(*i as u32),
            MenuSlot::Widget(id) => Self::Widget((*id).to_string()),
        }
    }

    pub fn to_menu_slot(&self) -> crate::gui_state::MenuSlot {
        use crate::gui_state::MenuSlot;
        match self {
            Self::Inventory(i) => MenuSlot::Inventory(*i as usize),
            Self::CraftResult => MenuSlot::CraftResult,
            Self::Container(i) => MenuSlot::Container(*i as usize),
            Self::Widget(id) => MenuSlot::Widget(crate::gui_state::intern_str(id)),
        }
    }
}

/// One [`crate::gui_state::GuiValue`] on the wire.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum GuiValueWire {
    F32(f32),
    I32(i32),
    Str(String),
}

impl GuiValueWire {
    pub fn from_value(v: &crate::gui_state::GuiValue) -> Self {
        match v {
            crate::gui_state::GuiValue::F32(x) => Self::F32(*x),
            crate::gui_state::GuiValue::I32(x) => Self::I32(*x),
            crate::gui_state::GuiValue::Str(s) => Self::Str(s.clone()),
        }
    }

    pub fn into_value(self) -> crate::gui_state::GuiValue {
        match self {
            Self::F32(x) => crate::gui_state::GuiValue::F32(x),
            Self::I32(x) => crate::gui_state::GuiValue::I32(x),
            Self::Str(s) => crate::gui_state::GuiValue::Str(s),
        }
    }
}

/// The recipient's open menu-session target, with everything its screen
/// renders. Item slots are wire ids ([`ItemSlotWire`]), remapped at the
/// transport boundary.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub enum MenuTargetWire {
    /// No menu session is open (sent once when a session closes).
    #[default]
    None,
    /// A player-crafting session at any station (inventory screen, crafting
    /// table, or a pack workbench — the open-GUI event already carried the
    /// station's kind). The result is transient server-owned menu state, not
    /// an inventory slot.
    Crafting { output: Option<ItemSlotWire> },
    Container {
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
pub struct MenuSyncMsg {
    pub target: MenuTargetWire,
}
