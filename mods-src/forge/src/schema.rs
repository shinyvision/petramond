use std::collections::{BTreeMap, HashMap};

use mod_sdk::*;
use serde::de::DeserializeOwned;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MouldSpec {
    pub class: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MetalSpec {
    pub melt_ticks: Option<f64>,
    pub molten: Option<[u8; 3]>,
    pub solid: Option<[u8; 3]>,
    pub name: Option<String>,
    pub melts_to: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum NondestructiveSpec {
    Flag(bool),
    Tags(TagsSpec),
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TagsSpec {
    #[serde(default)]
    pub blocks: Vec<String>,
}

impl NondestructiveSpec {
    pub fn enabled(&self) -> bool {
        match self {
            NondestructiveSpec::Flag(enabled) => *enabled,
            NondestructiveSpec::Tags(_) => true,
        }
    }

    pub fn tags(&self) -> &[String] {
        match self {
            NondestructiveSpec::Flag(_) => &[],
            NondestructiveSpec::Tags(tags) => &tags.blocks,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FitSpec {
    pub tool: String,
    #[serde(default)]
    pub tier: u8,
    pub speed_mult: Option<f32>,
    pub damage_mult: Option<f32>,
    pub knockback_mult: Option<f32>,
    pub cost: Option<u8>,
    pub overlay: String,
    #[serde(default)]
    pub overlays: BTreeMap<String, String>,
    pub gentle: Option<GentleSpec>,
    pub wear: Option<WearSpec>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GentleSpec {
    pub chance: Option<u8>,
    #[serde(default)]
    pub blocks: Vec<String>,
}

impl GentleSpec {
    pub fn chance(&self) -> u8 {
        self.chance.unwrap_or(100).clamp(1, 100)
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WearSpec {
    pub on: WearOnSpec,
    pub max: f64,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WearOnSpec {
    Break,
    Hit,
    Proc,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SlotsSpec {
    pub family: Option<String>,
    pub lockable: Option<u8>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpgradeSpec {
    pub kind: String,
    pub name: String,
    pub info: String,
    pub icon: String,
    pub cost: Vec<CostSpec>,
    pub set_ticks: Option<f64>,
    pub dwell_ticks: Option<f64>,
    pub feed_every: Option<f64>,
    pub feed_keep: Option<f64>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CostSpec {
    pub item: String,
    pub count: u8,
}

pub fn read_rows<T: DeserializeOwned>(key: &str) -> HashMap<String, T> {
    let rows = items_with_data_as::<T>(key);
    if rows.is_empty() {
        return HashMap::new();
    }
    let names = item_names(rows.iter().map(|(id, _)| *id).collect());
    rows.into_iter()
        .zip(names)
        .filter_map(|((_, value), name)| Some((name?, value)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keys;

    const ITEM_DOCUMENTS: [&str; 4] = [
        include_str!("../pack/items.json"),
        include_str!("../../combat/pack/items.json"),
        include_str!("../../combat/pack/integrations/forge/items.json"),
        include_str!("../../monsters/pack/items.json"),
    ];

    fn each_item<T: DeserializeOwned>(key: &str) -> usize {
        let mut count = 0;
        for document in ITEM_DOCUMENTS {
            for (row, entry) in pack_rows_with_data(document, "items", key) {
                parse_row_data::<T>(&entry).unwrap_or_else(|e| panic!("{key} on {row}: {e}"));
                count += 1;
            }
        }
        count
    }

    #[test]
    fn every_shipped_entry_matches_its_schema() {
        assert!(each_item::<MouldSpec>(keys::MOULD_DATA) > 0);
        assert!(each_item::<MetalSpec>(keys::METAL_DATA) > 0);
        assert!(each_item::<NondestructiveSpec>(keys::NONDESTRUCTIVE_DATA) > 0);
        assert!(each_item::<Vec<FitSpec>>(keys::AUGMENT_DATA) > 0);
        assert!(each_item::<SlotsSpec>(keys::AUGMENT_SLOTS_DATA) > 0);
        let blocks = include_str!("../pack/blocks.json");
        let upgrades = pack_rows_with_data(blocks, "blocks", keys::UPGRADES_DATA);
        assert_eq!(upgrades.len(), 1);
        parse_row_data::<Vec<UpgradeSpec>>(&upgrades[0].1).unwrap_or_else(|e| panic!("{e}"));
    }

    #[test]
    fn a_misspelt_field_is_an_error_not_a_default() {
        assert!(parse_row_data::<MouldSpec>(r#"{"clas": "forge:cast"}"#).is_err());
        assert!(parse_row_data::<SlotsSpec>(r#"{"famly": "stone"}"#).is_err());
        let spec: NondestructiveSpec = parse_row_data("true").unwrap();
        assert!(spec.tags().is_empty());
        let spec: NondestructiveSpec = parse_row_data(r#"{"blocks": ["a:b"]}"#).unwrap();
        assert_eq!(spec.tags(), ["a:b"]);
    }
}
