use petramond_world::item::{variant, ItemStack};
use serde::Deserialize;

/// Rules carried with an item through flight, lodging, dropping and save restoration.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub(super) struct ItemRules {
    pub pickup: bool,
    pub lifetime_ticks: Option<u32>,
}

impl Default for ItemRules {
    fn default() -> Self {
        Self {
            pickup: true,
            lifetime_ticks: None,
        }
    }
}

impl ItemRules {
    pub fn of(stack: ItemStack) -> Self {
        variant::get(stack.variant)
            .and_then(|data| {
                data.get("petramond:item_entity")
                    .and_then(|raw| serde_json::from_slice(raw).ok())
            })
            .unwrap_or_default()
    }
}
