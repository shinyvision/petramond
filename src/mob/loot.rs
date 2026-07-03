//! Mob loot tables loaded from `assets/loot_tables.json` (serde).
//!
//! Mirrors the crafting recipe loader ([`crate::crafting`]): the on-disk file is
//! preferred so loot can be tuned without a rebuild, with an embedded copy as the
//! fallback when the game runs outside the project tree. Items are referenced by their
//! stable snake_case [`key`](crate::item::ItemType::key); tables are keyed by a mob's
//! [`key`](crate::mob::MobDef::key). Malformed entries are logged and skipped rather
//! than aborting the world load.

use std::collections::HashMap;

use serde::Deserialize;

use crate::item::{ItemStack, ItemType};

/// Embedded fallback so the game always has loot tables, even run outside the project
/// tree. The on-disk copy, when found, takes priority.
const EMBEDDED: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/assets/loot_tables.json"
));

#[derive(Deserialize)]
struct RawFile {
    tables: HashMap<String, Vec<RawDrop>>,
}

#[derive(Deserialize)]
struct RawDrop {
    item: String,
    #[serde(default = "one")]
    min: u8,
    #[serde(default = "one")]
    max: u8,
    #[serde(default = "always")]
    chance: f32,
}

fn one() -> u8 {
    1
}
fn always() -> f32 {
    1.0
}

/// One resolved loot entry: an item, an inclusive count range, and a drop chance.
#[derive(Copy, Clone, Debug)]
pub struct LootEntry {
    pub item: ItemType,
    pub min: u8,
    pub max: u8,
    pub chance: f32,
}

/// A mob species' loot table — the entries rolled when one of its mobs dies.
#[derive(Clone, Debug, Default)]
pub struct LootTable {
    pub entries: Vec<LootEntry>,
}

impl LootTable {
    /// Roll this table deterministically from `seed`: each entry independently passes
    /// its `chance`, then rolls a uniform count in `[min, max]`. Returns the stacks to
    /// drop (empty if nothing passed). Deterministic so a given kill is reproducible
    /// and so it's unit-testable without a `Game`.
    pub fn roll(&self, seed: u64) -> Vec<ItemStack> {
        let mut out = Vec::new();
        for (i, e) in self.entries.iter().enumerate() {
            // Decorrelate each entry's rolls from the others via the entry index.
            let s = seed.wrapping_add((i as u64 + 1).wrapping_mul(0x9E37_79B9_7F4A_7C15));
            if crate::entity::hash01(s) >= e.chance {
                continue;
            }
            let count = if e.min >= e.max {
                e.min
            } else {
                let span = (e.max - e.min + 1) as f32;
                (e.min + (crate::entity::hash01(s ^ 0xD1CE_D1CE) * span) as u8).min(e.max)
            };
            if count == 0 {
                continue;
            }
            out.push(ItemStack::new(e.item, count));
        }
        out
    }
}

/// Every loot table, keyed by mob [`key`](crate::mob::MobDef::key).
#[derive(Clone, Debug, Default)]
pub struct LootTables {
    by_key: HashMap<String, LootTable>,
}

impl LootTables {
    /// The loot table for the mob `key`, or `None` if the species drops nothing.
    pub fn get(&self, key: &str) -> Option<&LootTable> {
        self.by_key.get(key)
    }
}

/// Load loot tables from every `loot_tables.json` layer (base + mod packs; a
/// later layer REPLACES a mob's whole table by its key), falling back to the
/// embedded copy when nothing on disk provides one.
pub fn load_loot() -> LootTables {
    let layers = crate::assets::read_layers("loot_tables.json");
    let texts: Vec<String> = if layers.is_empty() {
        log::info!("loot tables: no on-disk loot_tables.json found, using embedded defaults");
        vec![EMBEDDED.to_string()]
    } else {
        for (_, path) in &layers {
            log::info!("loot tables layer: {}", path.display());
        }
        layers.into_iter().map(|(s, _)| s).collect()
    };
    let mut merged: HashMap<String, Vec<RawDrop>> = HashMap::new();
    for text in &texts {
        match serde_json::from_str::<RawFile>(text) {
            Ok(file) => merged.extend(file.tables),
            Err(e) => log::error!("loot_tables.json layer is not valid JSON (skipped): {e}"),
        }
    }
    convert(merged)
}

/// Test shim: one layer, straight through the merge-free path.
#[cfg(test)]
fn parse(text: &str) -> LootTables {
    match serde_json::from_str::<RawFile>(text) {
        Ok(f) => convert(f.tables),
        Err(e) => {
            log::error!("loot_tables.json is not valid JSON: {e}");
            LootTables::default()
        }
    }
}

fn convert(tables: HashMap<String, Vec<RawDrop>>) -> LootTables {
    let mut by_key = HashMap::new();
    for (mob, drops) in tables {
        let mut entries = Vec::new();
        for (i, d) in drops.into_iter().enumerate() {
            match resolve(d) {
                Ok(e) => entries.push(e),
                Err(msg) => log::error!("loot table '{mob}' drop #{i}: {msg}"),
            }
        }
        by_key.insert(mob, LootTable { entries });
    }
    LootTables { by_key }
}

fn resolve(d: RawDrop) -> Result<LootEntry, String> {
    let item = item_from_key(&d.item).ok_or_else(|| format!("unknown item '{}'", d.item))?;
    let (min, max) = if d.min <= d.max {
        (d.min, d.max)
    } else {
        (d.max, d.min)
    };
    Ok(LootEntry {
        item,
        min,
        max,
        chance: d.chance.clamp(0.0, 1.0),
    })
}

/// Resolve a stable snake_case item key (e.g. `stick`) to its [`ItemType`] — matched
/// against each item's explicit key, like the recipe loader.
fn item_from_key(key: &str) -> Option<ItemType> {
    ItemType::ALL.iter().copied().find(|it| it.key() == key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_loot_parses_and_resolves_the_owl() {
        let tables = parse(EMBEDDED);
        let owl = tables.get("owl").expect("owl loot table exists");
        let items: Vec<ItemType> = owl.entries.iter().map(|e| e.item).collect();
        assert!(items.contains(&ItemType::Stick), "owl drops sticks");
        assert!(items.contains(&ItemType::Coal), "owl drops coal");
        for e in &owl.entries {
            assert!(e.min <= e.max, "count range ordered");
            assert!((0.0..=1.0).contains(&e.chance), "chance in [0,1]");
        }
    }

    #[test]
    fn unknown_item_entry_is_skipped_not_fatal() {
        let text = r#"{ "tables": { "owl": [
            { "item": "stick", "min": 1, "max": 2, "chance": 0.5 },
            { "item": "mystery_meat", "chance": 1.0 }
        ] } }"#;
        let tables = parse(text);
        let owl = tables
            .get("owl")
            .expect("table present despite a bad entry");
        assert_eq!(owl.entries.len(), 1, "only the valid entry survives");
        assert_eq!(owl.entries[0].item, ItemType::Stick);
    }

    #[test]
    fn roll_respects_chance_bounds_and_count_range() {
        // chance 1.0, count 2..=4: always drops, always within range.
        let table = LootTable {
            entries: vec![LootEntry {
                item: ItemType::Stick,
                min: 2,
                max: 4,
                chance: 1.0,
            }],
        };
        for seed in 0..200u64 {
            let stacks = table.roll(seed);
            assert_eq!(stacks.len(), 1, "chance 1.0 always drops");
            assert_eq!(stacks[0].item, ItemType::Stick);
            assert!(
                (2..=4).contains(&stacks[0].count),
                "count {} in 2..=4",
                stacks[0].count
            );
        }
        // chance 0.0 never drops.
        let never = LootTable {
            entries: vec![LootEntry {
                item: ItemType::Coal,
                min: 1,
                max: 1,
                chance: 0.0,
            }],
        };
        assert!(
            (0..200u64).all(|s| never.roll(s).is_empty()),
            "chance 0.0 never drops"
        );
    }

    #[test]
    fn malformed_json_is_empty_not_a_panic() {
        assert!(parse("not json").get("owl").is_none());
        assert!(parse("{}").get("owl").is_none());
    }
}
