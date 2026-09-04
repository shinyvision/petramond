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

use crate::block::Block;
use crate::registry::ContentNames;
use crate::tile::Tile;

use super::definition::ItemDef;
use super::{HeldPose, ItemTag, ItemType, ItemUse, Tool, ToolKind};

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
    /// Optional info line shown under the name in the item's slot tooltip
    /// (usage hints a name alone cannot carry); absent for ordinary items.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub info: Option<String>,
    pub max_stack_size: u8,
    /// First-person hold orientation of the sprite; absent = the upright
    /// default every ordinary item carries ([`HeldPose::DEFAULT`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub held_pose: Option<RawPose>,
    /// Which way the item's SPRITE art points, in degrees anticlockwise
    /// from the tile's +X — the direction a flying or lodged item lays along
    /// its heading (see [`ItemType::sprite_axis_roll`](super::ItemType::sprite_axis_roll)).
    /// Absent = the tool diagonal every tool and weapon sprite is drawn to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sprite_axis: Option<f32>,
    /// Atlas tile name of the flat billboard sprite, for the items drawn as one
    /// (tools, raw drops, door/torch icons). Absent for items whose icon comes
    /// from their block or bbmodel.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sprite: Option<String>,
    /// `models.json` key of the bbmodel an ITEM-ONLY item renders as (held /
    /// dropped / icon — e.g. the bucket). Absent for sprite items and for
    /// block-items (their look follows their block).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<crate::block_model::BlockModelKind>,
    /// Tag names: bare engine tags or namespaced `mod_id:name` pack tags
    /// (interned at load — see [`ItemTag::resolve`]). Optional: a row that
    /// describes its membership through the `data` surface instead (which is
    /// the form a `patch` row can reach) has no tags to state.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    /// Registry name of the block this item places — the ONE source of the
    /// block↔item mapping (`ItemType::from_block`/`as_block`), engine and
    /// pack rows alike. Absent for item-only items.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub block: Option<String>,
    /// Engine use handler (see [`ItemUse`]): a bare name for parameterless
    /// handlers (`"use": "shear"`) or a tagged object whose params ride inside
    /// (`"use": {"bucket_fill": {"becomes": "petramond:water_bucket"}}`).
    #[serde(default, rename = "use", skip_serializing_if = "Option::is_none")]
    pub use_: Option<RawItemUse>,
    /// Which raycast this item's use click targets with (see
    /// [`UseRay`](super::UseRay)); absent = the normal water-transparent ray.
    #[serde(default, skip_serializing_if = "is_default_use_ray")]
    pub use_ray: super::UseRay,
    /// Namespaced consumer-data entries (`"ns:key": <any JSON>`): the item
    /// interop surface. A key names a CONSUMING system's vocabulary — engine
    /// consumers (`petramond:fuel`, `petramond:tool`) and mod consumers
    /// (`furniture:pigment`) alike — and the value is opaque JSON that
    /// consumer parses. Any pack may attach any consumer's key to its own
    /// rows, or to EXISTING rows via `{"patch": ..., "data": ...}` rows.
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub data: serde_json::Map<String, serde_json::Value>,
    /// Edible-item data (hold right mouse to eat); absent = not food.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub food: Option<RawFood>,
    /// Dropped-entity environmental reaction (see
    /// [`DroppedReaction`](super::DroppedReaction)); absent = inert.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dropped_reaction: Option<RawDroppedReaction>,
}

/// A row's `use` field: a bare engine handler name (`"shear"`) or a tagged
/// object carrying the handler's row params (`{"bucket_fill": {"becomes":
/// ...}}` — the `effects.json` behavior shape). Resolved to [`ItemUse`] in
/// [`convert`]; a parameterized handler written bare is a load error, so a
/// bucket can never fall back to some hardcoded engine counterpart.
#[derive(Serialize, Deserialize)]
#[serde(untagged)]
pub(super) enum RawItemUse {
    Bare(String),
    Tagged(RawTaggedUse),
}

/// The parameterized engine use handlers, externally tagged by handler key.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum RawTaggedUse {
    BucketFill(RawBucketUse),
    BucketPour(RawBucketUse),
}

/// A bucket handler's row params: which item the held one becomes on success
/// (the row-owned empty↔filled pair — fill declares the filled item, pour the
/// empty one).
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RawBucketUse {
    /// Registry name of the resulting item.
    pub becomes: String,
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

fn is_default_use_ray(v: &super::UseRay) -> bool {
    *v == super::UseRay::default()
}

/// The `petramond:tool` data entry: family, harvest gate, and — optionally —
/// the two properties a gate cannot express on its own.
///
/// The shipped mining ladder is stone `2`, iron `3`, diamond `4`; rung `1`
/// carries only the shears since the wooden tools were retired. A row that
/// states only `kind` and `tier` gets exactly that ladder's speed and damage,
/// so this surface costs the engine's own rows nothing.
///
/// `speed` and `damage` exist because a MOD adding a material owns what that
/// material is like, and materials are not points on one line: a soft metal
/// can reach everything and dig like a fist.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RawTool {
    pub kind: ToolKind,
    pub tier: u8,
    /// Mining speed over the bare hand. Defaults to the tier's rung.
    #[serde(default)]
    pub speed: Option<f32>,
    /// Melee damage `[min, max]`. Defaults to the `(kind, tier)` rung.
    #[serde(default)]
    pub damage: Option<[f32; 2]>,
    /// Knockback multiplier over the victim's own. Defaults to `1.0`.
    #[serde(default)]
    pub knockback: Option<f32>,
}

/// The `petramond:projectile` data entry — see [`super::Projectile`]; every
/// field optional over that type's defaults.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RawProjectile {
    #[serde(default)]
    pub gravity: Option<f32>,
    #[serde(default)]
    pub drag: Option<f32>,
    #[serde(default)]
    pub sticks: Option<bool>,
}

/// The `petramond:fuel` data entry: game ticks one of this item burns as
/// furnace fuel.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RawFuel {
    pub burn_ticks: u16,
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
    // Data patches split out of every layer during the parse (layer order
    // preserved); the convert applies each row's matching patches. RefCell:
    // the parse and convert closures both borrow the collection.
    let patches = std::cell::RefCell::new(Vec::new());
    let defs = crate::registry::resolve_catalog(
        texts,
        |text| crate::registry::parse_rows_with_patches(text, "items", &mut patches.borrow_mut()),
        |r: &RawItemDef| &r.item,
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
            convert(r, ItemType(id), names, &patches.borrow())
                .map_err(|e| format!("item '{name}': {e}"))
        },
    )?;
    for p in patches.borrow().iter() {
        if names.items.id(&p.patch).is_none() {
            return Err(format!("data patch targets unknown item '{}'", p.patch));
        }
    }
    Ok(Box::leak(defs.into_boxed_slice()))
}

fn convert(
    r: RawItemDef,
    item: ItemType,
    names: &ContentNames,
    patches: &[crate::registry::RawDataPatch],
) -> Result<ItemDef, String> {
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
    let becomes_item = |name: &str| {
        names
            .items
            .id(name)
            .map(ItemType)
            .ok_or_else(|| format!("unknown 'becomes' item '{name}' in the row's use handler"))
    };
    let item_use = match &r.use_ {
        None => None,
        Some(RawItemUse::Bare(name)) => Some(match name.as_str() {
            "shear" => ItemUse::Shear,
            // Parameterized handlers written bare would need a hardcoded
            // engine counterpart — the row must declare its own.
            "bucket_fill" | "bucket_pour" => {
                return Err(format!(
                    "use '{name}' needs its result item: {{\"{name}\": {{\"becomes\": \
                     \"<item>\"}}}}"
                ))
            }
            other => {
                return Err(format!(
                    "unknown use handler '{other}' (engine handlers only; mods react via \
                     the item_use_pre event)"
                ))
            }
        }),
        Some(RawItemUse::Tagged(tagged)) => Some(match tagged {
            RawTaggedUse::BucketFill(b) => ItemUse::BucketFill {
                becomes: becomes_item(&b.becomes)?,
            },
            RawTaggedUse::BucketPour(b) => ItemUse::BucketPour {
                becomes: becomes_item(&b.becomes)?,
            },
        }),
    };
    let data = crate::registry::compile_data_map(
        names.items.name(item.0).unwrap_or(""),
        &r.data,
        patches,
    )?;
    // Fuel and tool are ordinary data-surface consumers whose system is the
    // engine (furnace / mining) — same vocabulary a mod consumer uses.
    let fuel_burn_ticks = crate::registry::engine_data::<RawFuel>(data, "petramond:fuel")?
        .map_or(0, |f| f.burn_ticks);
    let tool = match crate::registry::engine_data::<RawTool>(data, "petramond:tool")? {
        Some(t) => {
            if !(1..=4).contains(&t.tier) {
                return Err(format!(
                    "tool tier {} out of range (1 = wooden … 4 = diamond)",
                    t.tier
                ));
            }
            let speed = match t.speed {
                None => crate::item::default_speed(t.tier),
                Some(s) if s.is_finite() && s > 0.0 => s,
                Some(s) => return Err(format!("tool speed {s} must be finite and positive")),
            };
            let damage = match t.damage {
                None => crate::item::default_damage(t.kind, t.tier),
                Some([lo, hi]) if lo.is_finite() && hi.is_finite() && lo >= 0.0 && hi >= lo => {
                    (lo, hi)
                }
                Some(d) => {
                    return Err(format!(
                        "tool damage {d:?} must be finite, non-negative and ordered [min, max]"
                    ))
                }
            };
            let knockback = match t.knockback {
                None => crate::item::DEFAULT_KNOCKBACK,
                Some(k) if k.is_finite() && k >= 0.0 => k,
                Some(k) => {
                    return Err(format!(
                        "tool knockback {k} must be finite and non-negative"
                    ))
                }
            };
            Some(Tool {
                kind: t.kind,
                tier: t.tier,
                speed,
                damage,
                knockback,
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
    let projectile =
        match crate::registry::engine_data::<RawProjectile>(data, super::PROJECTILE_DATA_KEY)? {
            Some(p) => {
                let base = super::Projectile::default();
                let gravity = match p.gravity {
                    None => base.gravity,
                    Some(g) if g.is_finite() && g >= 0.0 => g,
                    Some(g) => {
                        return Err(format!(
                            "projectile gravity {g} must be finite and non-negative"
                        ))
                    }
                };
                let drag = match p.drag {
                    None => base.drag,
                    Some(d) if (0.0..=1.0).contains(&d) => d,
                    Some(d) => return Err(format!("projectile drag {d} must lie in [0, 1]")),
                };
                Some(super::Projectile {
                    gravity,
                    drag,
                    sticks: p.sticks.unwrap_or(base.sticks),
                })
            }
            None => None,
        };
    let sprite_axis_degrees = match r.sprite_axis {
        None => super::DEFAULT_SPRITE_AXIS_DEGREES,
        Some(a) if a.is_finite() => a,
        Some(a) => return Err(format!("sprite_axis {a} must be finite")),
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
                    crate::sound_registry::by_name(key)
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
        info: r.info.map(|info| &*Box::leak(info.into_boxed_str())),
        max_stack_size: r.max_stack_size,
        held_pose: r.held_pose.map_or(HeldPose::DEFAULT, |p| HeldPose {
            pitch: p.pitch as f32,
            yaw: p.yaw as f32,
            roll: p.roll as f32,
        }),
        sprite_axis_degrees,
        sprite,
        model: r.model,
        tags: Box::leak(tags.into_boxed_slice()),
        block,
        item_use,
        use_ray: r.use_ray,
        fuel_burn_ticks,
        tool,
        food,
        dropped_reaction,
        projectile,
        data,
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
    fn a_rows_info_line_loads_and_defaults_to_none() {
        let (base, _) =
            crate::assets::read_base_text("items.json").expect("assets/items.json must ship");
        let layer = r#"{"items": [{"item": "petramond:stone", "key": "petramond:stone", "name": "Stone", "info": "A hint", "max_stack_size": 64, "held_pose": {"pitch": 0, "yaw": 1.8, "roll": 0}, "tags": []}]}"#;
        let defs = parse_test_layers(&[&base, layer]).expect("info row loads");
        assert_eq!(defs[ItemType::Stone.id() as usize].info, Some("A hint"));
        assert_eq!(defs[ItemType::Dirt.id() as usize].info, None);
    }

    #[test]
    fn namespaced_pack_row_registers_a_new_item_with_links() {
        let (base, _) =
            crate::assets::read_base_text("items.json").expect("assets/items.json must ship");
        // A dynamic item linking to an engine block (any registered block name
        // resolves the same way) and carrying an engine use handler whose
        // result item is the ROW'S OWN declaration — a pack bucket fills into
        // the pack's counterpart, never a hardcoded engine item.
        let layer = r#"{"items": [
            {"item": "mymod:filled_gadget", "key": "mymod:filled_gadget", "name": "Filled Gadget", "max_stack_size": 1, "held_pose": {"pitch": 0, "yaw": 1.8, "roll": 0}, "tags": []},
            {"item": "mymod:gadget", "key": "mymod:gadget", "name": "Gadget", "max_stack_size": 64, "held_pose": {"pitch": 0, "yaw": 1.8, "roll": 0}, "tags": [], "block": "petramond:stone", "use": {"bucket_fill": {"becomes": "mymod:filled_gadget"}}}
        ]}"#;
        let defs = parse_test_layers(&[&base, layer]).expect("dynamic rows load");
        let engine = crate::item::ENGINE_ITEM_NAMES.len();
        assert_eq!(defs.len(), engine + 2, "fresh ids past the engine set");
        let filled = defs[engine].item;
        let gadget = &defs[engine + 1];
        assert_eq!(gadget.item, ItemType((engine + 1) as u16));
        assert_eq!(gadget.block, Some(crate::block::Block::Stone));
        assert_eq!(
            gadget.item_use,
            Some(ItemUse::BucketFill { becomes: filled })
        );
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
        // A bucket handler written BARE has no declared result item — refused,
        // never defaulted to an engine bucket.
        let bare_bucket = r#"{"items": [{"item": "mymod:g", "key": "mymod:g", "name": "G", "max_stack_size": 64, "held_pose": {"pitch": 0, "yaw": 1.8, "roll": 0}, "tags": [], "use": "bucket_fill"}]}"#;
        let err = parse_test_layers(&[&base, bare_bucket]).expect_err("bare bucket use refused");
        assert!(err.contains("becomes"), "{err}");
        // A declared `becomes` naming an unknown item is a load error.
        let bad_becomes = r#"{"items": [{"item": "mymod:g", "key": "mymod:g", "name": "G", "max_stack_size": 64, "held_pose": {"pitch": 0, "yaw": 1.8, "roll": 0}, "tags": [], "use": {"bucket_pour": {"becomes": "mymod:nope"}}}]}"#;
        let err = parse_test_layers(&[&base, bad_becomes]).expect_err("unknown becomes refused");
        assert!(err.contains("becomes"), "{err}");
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

#[cfg(test)]
mod data_tests {
    use super::*;

    /// The item-data interop surface end to end: a row's own `data` entries
    /// compile; a later layer's `{"patch", "data"}` row attaches entries to an
    /// EXISTING (engine) row and overrides earlier keys (later layer wins);
    /// the engine's own fuel/tool consumers read the same surface; a patch
    /// naming an unknown row is a load error.
    #[test]
    fn data_entries_load_and_patch_rows_merge_by_layer_order() {
        let (base, _) =
            crate::assets::read_base_text("items.json").expect("assets/items.json must ship");
        let layer_a = r#"{"items": [
            {"item": "mymod:berry", "key": "mymod:berry", "name": "Berry", "max_stack_size": 64,
             "held_pose": {"pitch": 0, "yaw": 0, "roll": 0}, "tags": [],
             "data": {"furniture:pigment": {"color": [1, 2, 3]}, "petramond:fuel": {"burn_ticks": 100}}},
            {"patch": "petramond:poppy", "data": {"furniture:pigment": {"color": [222, 38, 28]}}}
        ]}"#;
        let layer_b = r#"{"items": [
            {"patch": "mymod:berry", "data": {"furniture:pigment": {"color": [9, 9, 9]}}}
        ]}"#;
        let defs = parse_test_layers(&[&base, layer_a, layer_b]).expect("layers load");
        let berry = &defs[crate::item::ENGINE_ITEM_NAMES.len()];
        assert_eq!(
            berry.data.iter().find(|(k, _)| *k == "furniture:pigment"),
            Some(&("furniture:pigment", r#"{"color":[9,9,9]}"#)),
            "the later layer's patch wins per key"
        );
        assert_eq!(
            berry.fuel_burn_ticks, 100,
            "engine fuel reads the data surface"
        );
        let poppy = &defs[ItemType::Poppy.id() as usize];
        assert!(
            poppy
                .data
                .iter()
                .any(|(k, v)| *k == "furniture:pigment" && v.contains("222")),
            "a patch attaches data to an engine row"
        );
        // The shipped tool rows still compile through `petramond:tool`.
        let pick = &defs[ItemType::StonePickaxe.id() as usize];
        assert_eq!(pick.tool.map(|t| t.tier), Some(2));

        let bad = r#"{"items": [{"patch": "mymod:missing", "data": {"a:b": 1}}]}"#;
        assert!(
            parse_test_layers(&[&base, bad]).is_err(),
            "unknown patch target"
        );
        let bare = r#"{"items": [{"patch": "petramond:poppy", "data": {"nonamespace": 1}}]}"#;
        assert!(parse_test_layers(&[&base, bare]).is_err(), "bare data key");
    }
}
