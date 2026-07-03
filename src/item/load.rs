//! Load item definitions from `assets/items.json` (serde).
//!
//! Mirror of `block::load`: every item's data row (stable recipe `key`, display
//! `name`, stack size, held pose, tags) lives on disk, editable — and moddable —
//! without a rebuild. The item table is load-bearing (recipes resolve by key,
//! inventories index by id), so the loader validates the file covers EVERY
//! `ItemType` exactly once — with unique keys — and fails loudly otherwise.

use serde::{Deserialize, Serialize};

use crate::atlas::Tile;

use super::definition::ItemDef;
use super::{HeldPose, ItemTag, ItemType};

#[derive(Serialize, Deserialize)]
pub(super) struct RawFile {
    pub items: Vec<RawItemDef>,
}

/// One item row as written in `items.json`: a mirror of [`ItemDef`] with owned
/// strings/Vecs. Pose floats ride as `f64` (JSON's native width) and narrow
/// back to the exact `f32` their shortest decimal representation denotes.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RawItemDef {
    pub item: ItemType,
    pub key: String,
    pub name: String,
    pub max_stack_size: u8,
    pub held_pose: RawPose,
    /// Atlas tile name of the flat billboard sprite, for the items drawn as one
    /// (tools, raw drops, door/torch icons). Absent for items whose icon comes
    /// from their block or bbmodel.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sprite: Option<String>,
    pub tags: Vec<ItemTag>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RawPose {
    pub pitch: f64,
    pub yaw: f64,
    pub roll: f64,
}

/// Load the item table from every `items.json` layer (base + mod packs, later
/// packs replacing rows by item), panicking with a precise message if the
/// table is missing or inconsistent.
pub(super) fn table() -> &'static [ItemDef] {
    let layers = crate::assets::read_layers("items.json");
    if layers.is_empty() {
        panic!(
            "items.json not found (searched {:?}); the game cannot run without its item table",
            crate::assets::candidate_paths("items.json")
        );
    }
    for (_, path) in &layers {
        log::info!("item defs layer: {}", path.display());
    }
    let texts: Vec<&str> = layers.iter().map(|(s, _)| s.as_str()).collect();
    parse_layers(&texts).unwrap_or_else(|e| panic!("items.json: {e}"))
}

#[cfg(test)]
pub(super) fn parse(text: &str) -> Result<&'static [ItemDef], String> {
    parse_layers(&[text])
}

pub(super) fn parse_layers(texts: &[&str]) -> Result<&'static [ItemDef], String> {
    // Merge layers by item: a later layer's row REPLACES the earlier one.
    let mut merged: Vec<RawItemDef> = Vec::new();
    for (li, text) in texts.iter().enumerate() {
        let raw: RawFile =
            serde_json::from_str(text).map_err(|e| format!("layer #{li}: invalid JSON: {e}"))?;
        for r in raw.items {
            match merged.iter_mut().find(|m| m.item == r.item) {
                Some(slot) => *slot = r,
                None => merged.push(r),
            }
        }
    }
    let expected = ItemType::ALL.len();
    let mut rows: Vec<Option<ItemDef>> = (0..expected).map(|_| None).collect();
    let mut keys = std::collections::HashSet::new();
    for r in merged {
        let item = r.item;
        let id = item.id() as usize;
        if !keys.insert(r.key.clone()) {
            return Err(format!(
                "item {item:?}: duplicate key '{}' — recipes resolve by key, so keys must be unique",
                r.key
            ));
        }
        rows[id] = Some(convert(r).map_err(|e| format!("item {item:?}: {e}"))?);
    }
    let mut defs = Vec::with_capacity(expected);
    for (id, row) in rows.into_iter().enumerate() {
        defs.push(row.ok_or_else(|| format!("missing row for item {:?}", ItemType::ALL[id]))?);
    }
    Ok(Box::leak(defs.into_boxed_slice()))
}

fn convert(r: RawItemDef) -> Result<ItemDef, String> {
    let sprite = match &r.sprite {
        Some(name) => {
            Some(Tile::from_name(name).ok_or_else(|| format!("unknown sprite tile '{name}'"))?)
        }
        None => None,
    };
    Ok(ItemDef {
        item: r.item,
        key: Box::leak(r.key.into_boxed_str()),
        name: Box::leak(r.name.into_boxed_str()),
        max_stack_size: r.max_stack_size,
        held_pose: HeldPose {
            pitch: r.held_pose.pitch as f32,
            yaw: r.held_pose.yaw as f32,
            roll: r.held_pose.roll as f32,
        },
        sprite,
        tags: Box::leak(r.tags.into_boxed_slice()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shipped `assets/items.json` must load fully — the same gate the game
    /// applies at startup, surfaced as a test so a bad edit fails CI, not a launch.
    #[test]
    fn shipped_items_json_loads_fully() {
        let (text, path) =
            crate::assets::read_text("items.json").expect("assets/items.json must ship");
        parse(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    }

    #[test]
    fn pack_layer_overrides_rows_by_item() {
        let (base, _) =
            crate::assets::read_text("items.json").expect("assets/items.json must ship");
        let layer = r#"{"items": [{"item": "stone", "key": "stone", "name": "Modded Stone", "max_stack_size": 16, "held_pose": {"pitch": 0, "yaw": 1.8, "roll": 0}, "tags": []}]}"#;
        let defs = parse_layers(&[&base, layer]).expect("layered table loads");
        let stone = &defs[ItemType::Stone.id() as usize];
        assert_eq!(stone.name, "Modded Stone");
        assert_eq!(stone.max_stack_size, 16);
    }

    #[test]
    fn loader_rejects_incomplete_tables_and_duplicate_keys() {
        let row = r#"{"item": "air", "key": "air", "name": "Air", "max_stack_size": 64, "held_pose": {"pitch": 0, "yaw": 1.8, "roll": 0}, "tags": []}"#;
        // One valid row is not a full table.
        let partial = format!("{{\"items\": [{row}]}}");
        assert!(parse(&partial).err().unwrap().contains("missing row"));
        // Two DIFFERENT items sharing one key: rejected (recipes resolve by key).
        let (base, _) =
            crate::assets::read_text("items.json").expect("assets/items.json must ship");
        let clash = r#"{"items": [{"item": "grass", "key": "stone", "name": "Grass", "max_stack_size": 64, "held_pose": {"pitch": 0, "yaw": 1.8, "roll": 0}, "tags": []}]}"#;
        assert!(parse_layers(&[&base, clash])
            .err()
            .unwrap()
            .contains("duplicate key"));
    }
}
