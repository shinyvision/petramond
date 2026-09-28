use serde::{Deserialize, Serialize};

use super::ItemSlotWire;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum MenuSlotWire {
    Inventory(u32),
    OffHand,
    CraftResult,
    Container(u32),
    Widget(String),
}

impl MenuSlotWire {
    pub fn from_menu_slot(slot: &petramond_world::gui_state::MenuSlot) -> Self {
        use petramond_world::gui_state::MenuSlot;
        match slot {
            MenuSlot::Inventory(i) => Self::Inventory(*i as u32),
            MenuSlot::OffHand => Self::OffHand,
            MenuSlot::CraftResult => Self::CraftResult,
            MenuSlot::Container(i) => Self::Container(*i as u32),
            MenuSlot::Widget(id) => Self::Widget((*id).to_string()),
        }
    }

    /// `None` for a widget id no loaded document declares: the id arrives from
    /// the network, and interning it would leak memory per distinct string.
    pub fn to_menu_slot(&self) -> Option<petramond_world::gui_state::MenuSlot> {
        use petramond_world::gui_state::MenuSlot;
        Some(match self {
            Self::Inventory(i) => MenuSlot::Inventory(*i as usize),
            Self::OffHand => MenuSlot::OffHand,
            Self::CraftResult => MenuSlot::CraftResult,
            Self::Container(i) => MenuSlot::Container(*i as usize),
            Self::Widget(id) => MenuSlot::Widget(crate::menu::slots::declared_widget(id)?),
        })
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum GuiValueWire {
    F32(f32),
    I32(i32),
    Str(String),
    List(Vec<std::collections::BTreeMap<String, Self>>),
}

impl GuiValueWire {
    pub fn from_value(v: &petramond_world::gui_state::GuiValue) -> Self {
        match v {
            petramond_world::gui_state::GuiValue::F32(x) => Self::F32(*x),
            petramond_world::gui_state::GuiValue::I32(x) => Self::I32(*x),
            petramond_world::gui_state::GuiValue::Str(s) => Self::Str(s.clone()),
            petramond_world::gui_state::GuiValue::List(rows) => Self::List(
                rows.iter()
                    .map(|row| {
                        row.iter()
                            .map(|(k, v)| (k.clone(), Self::from_value(v)))
                            .collect()
                    })
                    .collect(),
            ),
        }
    }

    pub fn into_value(self) -> petramond_world::gui_state::GuiValue {
        match self {
            Self::F32(x) => petramond_world::gui_state::GuiValue::F32(x),
            Self::I32(x) => petramond_world::gui_state::GuiValue::I32(x),
            Self::Str(s) => petramond_world::gui_state::GuiValue::Str(s),
            Self::List(rows) => petramond_world::gui_state::GuiValue::List(
                rows.into_iter()
                    .map(|row| row.into_iter().map(|(k, v)| (k, v.into_value())).collect())
                    .collect(),
            ),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub enum MenuTargetWire {
    #[default]
    None,
    Crafting {
        output: Option<ItemSlotWire>,
    },
    Container {
        kind_key: String,
        anchor: Option<crate::menu::MenuAnchor>,
        slots: Option<Vec<Option<ItemSlotWire>>>,
        gui_state: Option<Vec<(String, GuiValueWire)>>,
    },
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct MenuSyncMsg {
    pub target: MenuTargetWire,
}

#[cfg(test)]
mod tests {
    use super::MenuSlotWire;
    use petramond_world::gui_state::MenuSlot;

    #[test]
    fn a_widget_id_no_document_declares_is_refused_without_interning() {
        let wire = MenuSlotWire::Widget("never-declared:flood-0001".to_string());
        assert_eq!(wire.to_menu_slot(), None);
    }

    #[test]
    fn a_declared_widget_id_and_plain_slots_convert() {
        let wire = MenuSlotWire::Widget("ok".to_string());
        assert_eq!(wire.to_menu_slot(), Some(MenuSlot::Widget("ok")));
        assert_eq!(
            MenuSlotWire::Container(3).to_menu_slot(),
            Some(MenuSlot::Container(3))
        );
    }
}
