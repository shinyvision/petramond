use serde::Deserialize;
use std::collections::BTreeMap;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Template {
    pub size: [u16; 3],
    #[serde(default)]
    pub pivot: [i32; 3],
    pub palette: BTreeMap<String, Material>,
    #[serde(default)]
    pub fills: Vec<Fill>,
    #[serde(default)]
    pub blocks: Vec<Block>,
    #[serde(default)]
    pub markers: Vec<Marker>,
    #[serde(default)]
    pub connectors: Vec<Connector>,
    #[serde(default)]
    pub requirements: Vec<mod_api::StructureRequirementData>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Material {
    pub block: String,
    #[serde(default)]
    pub state: BTreeMap<String, String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Fill {
    pub from: [i32; 3],
    pub to: [i32; 3],
    pub palette: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Block {
    pub pos: [i32; 3],
    pub palette: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Marker {
    pub pos: [i32; 3],
    pub key: String,
    pub value: serde_json::Value,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Connector {
    pub name: String,
    pub kind: String,
    pub pos: [i32; 3],
    pub facing: String,
}
