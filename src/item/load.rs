//! Load item definitions from `assets/items.json` (serde).
//!
//! Mirror of `block::load`: every item's data row (stable recipe `key`, display
//! `name`, stack size, held pose, tags, use handler) lives on disk, editable —
//! and moddable — without a rebuild. Rows are keyed by registry name: an ENGINE
//! item name overrides that item's row, a NAMESPACED key (`mod_id:name`)
//! REGISTERS a new dynamic item (see [`crate::registry`]); a new bare name is
//! an error. The item table is load-bearing (recipes resolve by key,
//! inventories index by id), so the loader validates the file covers EVERY
//! registered item exactly once — with unique keys — and fails loudly otherwise.

use serde::{Deserialize, Serialize};

use crate::atlas::Tile;
use crate::block::Block;
use crate::registry::ContentNames;

use super::definition::ItemDef;
use super::{HeldPose, ItemTag, ItemType, ItemUse, Tool, ToolKind};

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
    /// Registry name: an engine item name (override) or a namespaced
    /// `mod_id:name` key (dynamic registration).
    pub item: String,
    pub key: String,
    pub name: String,
    pub max_stack_size: u8,
    pub held_pose: RawPose,
    /// Atlas tile name of the flat billboard sprite, for the items drawn as one
    /// (tools, raw drops, door/torch icons). Absent for items whose icon comes
    /// from their block or bbmodel.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sprite: Option<String>,
    /// Tag names: bare engine tags or namespaced `mod_id:name` pack tags
    /// (interned at load — see [`ItemTag::resolve`]).
    pub tags: Vec<String>,
    /// Registry name of the block a DYNAMIC item places — the data-side link
    /// that replaces the compiled `from_block`/`as_block` match for pack
    /// content. Engine rows omit it (their mapping stays compiled).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub block: Option<String>,
    /// Use-handler key (see [`ItemUse::from_name`]): an engine handler by bare
    /// name (`bucket_fill`, `bucket_pour`, `shear`) or a namespaced pending one.
    #[serde(default, rename = "use", skip_serializing_if = "Option::is_none")]
    pub use_: Option<String>,
    /// Which raycast this item's use click targets with (see
    /// [`UseRay`](super::UseRay)); absent = the normal water-transparent ray.
    #[serde(default, skip_serializing_if = "is_default_use_ray")]
    pub use_ray: super::UseRay,
    /// Game ticks this item burns as furnace fuel; absent = not a fuel.
    #[serde(default, skip_serializing_if = "u16_is_zero")]
    pub fuel_burn_ticks: u16,
    /// The mining tool this item acts as; absent = not a tool.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool: Option<RawTool>,
    /// Edible-item data (hold right mouse to eat); absent = not food.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub food: Option<RawFood>,
    /// Dropped-entity environmental reaction (see
    /// [`DroppedReaction`](super::DroppedReaction)); absent = inert.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dropped_reaction: Option<RawDroppedReaction>,
}

/// A dropped-reaction declaration in `items.json`: the environment predicate,
/// what the stack becomes, and the optional per-entity presentation.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RawDroppedReaction {
    /// Environment name (snake_case — see
    /// [`ReactionEnvironment`](super::ReactionEnvironment)).
    pub environment: super::ReactionEnvironment,
    /// Registry name of the item the whole stack becomes.
    pub result: String,
    /// A one-shot burst bundle key (`particle_emitters.json`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub burst: Option<String>,
    /// A `sounds.json` key played once per transformed entity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sound: Option<String>,
}

/// A food declaration in `items.json`: how long the eat takes and which
/// status effects it grants on being eaten.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RawFood {
    /// Game ticks of held-button eating before the item is consumed.
    #[serde(default = "default_eat_ticks")]
    pub eat_ticks: u32,
    /// Status effects granted when the eat completes.
    #[serde(default)]
    pub effects: Vec<RawFoodEffect>,
}

/// One granted effect: an `effects.json` registry key + duration in ticks.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RawFoodEffect {
    pub effect: String,
    pub ticks: u32,
}

/// 3 seconds at 20 TPS — the standard bite.
fn default_eat_ticks() -> u32 {
    60
}

fn u16_is_zero(v: &u16) -> bool {
    *v == 0
}

fn is_default_use_ray(v: &super::UseRay) -> bool {
    *v == super::UseRay::default()
}

/// A tool declaration in `items.json`: family + material tier (1 = wooden,
/// 2 = stone, 3 = iron, 4 = diamond).
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RawTool {
    pub kind: ToolKind,
    pub tier: u8,
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
    crate::registry::read_catalog("items.json", "item", |texts| {
        parse_layers(texts, crate::registry::names())
    })
}

#[cfg(test)]
pub(super) fn parse(text: &str) -> Result<&'static [ItemDef], String> {
    parse_test_layers(&[text])
}

/// Test harness: parse synthetic layers against a name table built from those
/// same layers (+ the shipped blocks for `block` link resolution), mirroring
/// the real bootstrap without touching the global registries.
#[cfg(test)]
pub(super) fn parse_test_layers(texts: &[&str]) -> Result<&'static [ItemDef], String> {
    let (blocks, _) =
        crate::assets::read_base_text("blocks.json").expect("assets/blocks.json must ship");
    let names = crate::registry::build_names(&[&blocks], texts)?;
    parse_layers(texts, &names)
}

pub(super) fn parse_layers(
    texts: &[&str],
    names: &ContentNames,
) -> Result<&'static [ItemDef], String> {
    let mut keys = std::collections::HashSet::new();
    let defs = crate::registry::resolve_catalog(
        texts,
        |text| serde_json::from_str::<RawFile>(text).map(|f| f.items),
        |r| &r.item,
        &names.items,
        "item",
        |r, id, _| {
            if !keys.insert(r.key.clone()) {
                return Err(format!(
                    "item '{}': duplicate key '{}' — recipes resolve by key, so keys must be unique",
                    r.item, r.key
                ));
            }
            let name = r.item.clone();
            convert(r, ItemType(id), names).map_err(|e| format!("item '{name}': {e}"))
        },
    )?;
    Ok(Box::leak(defs.into_boxed_slice()))
}

fn convert(r: RawItemDef, item: ItemType, names: &ContentNames) -> Result<ItemDef, String> {
    if r.max_stack_size == 0 {
        return Err("max_stack_size must be positive".to_owned());
    }
    let sprite = match &r.sprite {
        Some(name) => {
            Some(Tile::from_name(name).ok_or_else(|| format!("unknown sprite tile '{name}'"))?)
        }
        None => None,
    };
    let block = match &r.block {
        Some(name) => Some(
            names
                .blocks
                .id(name)
                .map(Block)
                .ok_or_else(|| format!("unknown block '{name}' in the row's block link"))?,
        ),
        None => None,
    };
    let item_use = match r.use_.as_deref() {
        Some(name) => Some(ItemUse::from_name(name).ok_or_else(|| {
            format!("unknown use handler '{name}' (engine handlers or 'mod_id:key' only)")
        })?),
        None => None,
    };
    let tool = match &r.tool {
        Some(t) => {
            if !(1..=4).contains(&t.tier) {
                return Err(format!(
                    "tool tier {} out of range (1 = wooden … 4 = diamond)",
                    t.tier
                ));
            }
            Some(Tool {
                kind: t.kind,
                tier: t.tier,
            })
        }
        None => None,
    };
    let tags: Vec<ItemTag> = r
        .tags
        .iter()
        .map(|t| ItemTag::resolve(t))
        .collect::<Result<_, String>>()?;
    let food = match &r.food {
        Some(f) => {
            if f.eat_ticks == 0 {
                return Err("food eat_ticks must be positive".to_owned());
            }
            let effects: Vec<(crate::effect::Effect, u32)> = f
                .effects
                .iter()
                .map(|e| {
                    crate::effect::by_name(&e.effect)
                        .map(|fx| (fx, e.ticks))
                        .ok_or_else(|| format!("unknown food effect '{}'", e.effect))
                })
                .collect::<Result<_, String>>()?;
            Some(super::FoodDef {
                eat_ticks: f.eat_ticks,
                effects: Box::leak(effects.into_boxed_slice()),
            })
        }
        None => None,
    };
    let dropped_reaction = match &r.dropped_reaction {
        Some(dr) => {
            let result =
                names.items.id(&dr.result).map(ItemType).ok_or_else(|| {
                    format!("unknown dropped_reaction result item '{}'", dr.result)
                })?;
            let burst = match &dr.burst {
                Some(key) => {
                    let bundle = crate::particle_emitters::by_key(key)
                        .ok_or_else(|| format!("unknown dropped_reaction burst bundle '{key}'"))?;
                    if bundle.burst.is_none() {
                        return Err(format!(
                            "dropped_reaction burst '{key}' is a looping bundle (one-shot \
                             'burst' bundles only)"
                        ));
                    }
                    Some(bundle.id)
                }
                None => None,
            };
            let sound = match &dr.sound {
                Some(key) => Some(
                    crate::audio::sound_by_name(key)
                        .ok_or_else(|| format!("unknown dropped_reaction sound '{key}'"))?,
                ),
                None => None,
            };
            Some(super::DroppedReaction {
                environment: dr.environment,
                result,
                burst,
                sound,
            })
        }
        None => None,
    };
    Ok(ItemDef {
        item,
        key: Box::leak(r.key.into_boxed_str()),
        name: Box::leak(r.name.into_boxed_str()),
        max_stack_size: r.max_stack_size,
        held_pose: HeldPose {
            pitch: r.held_pose.pitch as f32,
            yaw: r.held_pose.yaw as f32,
            roll: r.held_pose.roll as f32,
        },
        sprite,
        tags: Box::leak(tags.into_boxed_slice()),
        block,
        item_use,
        use_ray: r.use_ray,
        fuel_burn_ticks: r.fuel_burn_ticks,
        tool,
        food,
        dropped_reaction,
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
            crate::assets::read_base_text("items.json").expect("assets/items.json must ship");
        let defs = parse(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        assert_eq!(
            defs.len(),
            crate::item::ENGINE_ITEM_NAMES.len(),
            "the base table is exactly the engine set"
        );
    }

    #[test]
    fn pack_layer_overrides_rows_by_item() {
        let (base, _) =
            crate::assets::read_base_text("items.json").expect("assets/items.json must ship");
        let layer = r#"{"items": [{"item": "petramond:stone", "key": "petramond:stone", "name": "Modded Stone", "max_stack_size": 16, "held_pose": {"pitch": 0, "yaw": 1.8, "roll": 0}, "tags": []}]}"#;
        let defs = parse_test_layers(&[&base, layer]).expect("layered table loads");
        let stone = &defs[ItemType::Stone.id() as usize];
        assert_eq!(stone.name, "Modded Stone");
        assert_eq!(stone.max_stack_size, 16);
        assert_eq!(defs.len(), crate::item::ENGINE_ITEM_NAMES.len());
    }

    #[test]
    fn namespaced_pack_row_registers_a_new_item_with_links() {
        let (base, _) =
            crate::assets::read_base_text("items.json").expect("assets/items.json must ship");
        // A dynamic item linking to an engine block (any registered block name
        // resolves the same way) and carrying an engine use handler.
        let layer = r#"{"items": [
            {"item": "mymod:gadget", "key": "mymod:gadget", "name": "Gadget", "max_stack_size": 64, "held_pose": {"pitch": 0, "yaw": 1.8, "roll": 0}, "tags": [], "block": "petramond:stone", "use": "bucket_fill"}
        ]}"#;
        let defs = parse_test_layers(&[&base, layer]).expect("dynamic rows load");
        let engine = crate::item::ENGINE_ITEM_NAMES.len();
        assert_eq!(defs.len(), engine + 1, "fresh id past the engine set");
        let gadget = &defs[engine];
        assert_eq!(gadget.item, ItemType(engine as u8));
        assert_eq!(gadget.block, Some(crate::block::Block::Stone));
        assert_eq!(gadget.item_use, Some(ItemUse::BucketFill));
        // Engine rows are untouched.
        assert_eq!(defs[ItemType::Stone.id() as usize].item, ItemType::Stone);
    }

    #[test]
    fn bare_additions_and_bad_links_are_rejected() {
        let (base, _) =
            crate::assets::read_base_text("items.json").expect("assets/items.json must ship");
        // A NEW bare item name is refused at name-table build.
        let bare = r#"{"items": [{"item": "gadget", "key": "gadget", "name": "G", "max_stack_size": 64, "held_pose": {"pitch": 0, "yaw": 1.8, "roll": 0}, "tags": []}]}"#;
        let err = parse_test_layers(&[&base, bare]).expect_err("bare additions refused");
        assert!(err.contains("gadget") && err.contains("namespace"), "{err}");
        // An unknown use handler is a load error (there are only engine handlers;
        // mods react to item use via the `item_use_pre` event).
        let bad_use = r#"{"items": [{"item": "mymod:g", "key": "mymod:g", "name": "G", "max_stack_size": 64, "held_pose": {"pitch": 0, "yaw": 1.8, "roll": 0}, "tags": [], "use": "zap"}]}"#;
        let err = parse_test_layers(&[&base, bad_use]).expect_err("unknown use refused");
        assert!(err.contains("unknown use handler"), "{err}");
        // An unknown block link is a load error.
        let bad_block = r#"{"items": [{"item": "mymod:g", "key": "mymod:g", "name": "G", "max_stack_size": 64, "held_pose": {"pitch": 0, "yaw": 1.8, "roll": 0}, "tags": [], "block": "bogus_block"}]}"#;
        let err = parse_test_layers(&[&base, bad_block]).expect_err("unknown block refused");
        assert!(err.contains("bogus_block"), "{err}");
        let zero_stack = r#"{"items": [{"item": "mymod:g", "key": "mymod:g", "name": "G", "max_stack_size": 0, "held_pose": {"pitch": 0, "yaw": 1.8, "roll": 0}, "tags": []}]}"#;
        let err = parse_test_layers(&[&base, zero_stack]).expect_err("zero stack size refused");
        assert!(err.contains("max_stack_size must be positive"), "{err}");
    }

    #[test]
    fn loader_rejects_incomplete_tables_and_duplicate_keys() {
        let row = r#"{"item": "petramond:air", "key": "petramond:air", "name": "Air", "max_stack_size": 64, "held_pose": {"pitch": 0, "yaw": 1.8, "roll": 0}, "tags": []}"#;
        // One valid row is not a full table.
        let partial = format!("{{\"items\": [{row}]}}");
        assert!(parse(&partial).err().unwrap().contains("missing row"));
        // Two DIFFERENT items sharing one key: rejected (recipes resolve by key).
        let (base, _) =
            crate::assets::read_base_text("items.json").expect("assets/items.json must ship");
        let clash = r#"{"items": [{"item": "petramond:grass", "key": "petramond:stone", "name": "Grass", "max_stack_size": 64, "held_pose": {"pitch": 0, "yaw": 1.8, "roll": 0}, "tags": []}]}"#;
        assert!(parse_test_layers(&[&base, clash])
            .err()
            .unwrap()
            .contains("duplicate key"));
    }
}
