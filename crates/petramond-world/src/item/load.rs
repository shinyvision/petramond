use serde::{Deserialize, Serialize};

use crate::block::Block;
use crate::registry::ContentNames;
use crate::tile::Tile;

use super::definition::ItemDef;
use super::{HeldPose, ItemTag, ItemType, ItemUse, Tool, ToolKind};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RawItemDef {
    pub item: String,
    pub key: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub info: Option<String>,
    pub max_stack_size: u8,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub held_pose: Option<RawPose>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sprite_axis: Option<f32>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub sprite_face_leads: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sprite: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<crate::block_model::BlockModelKind>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub block: Option<String>,
    #[serde(default, rename = "use", skip_serializing_if = "Option::is_none")]
    pub use_: Option<RawItemUse>,
    #[serde(default, skip_serializing_if = "RawUseRay::is_solid")]
    pub use_ray: RawUseRay,
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub data: serde_json::Map<String, serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub food: Option<RawFood>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dropped_reaction: Option<RawDroppedReaction>,
}

#[derive(Serialize, Deserialize)]
#[serde(untagged)]
pub(super) enum RawItemUse {
    Bare(String),
    Tagged(RawTaggedUse),
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum RawTaggedUse {
    BucketFill(RawBucketFill),
    BucketPour(RawBucketPour),
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RawBucketFill {
    pub becomes: std::collections::BTreeMap<String, String>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RawBucketPour {
    pub becomes: String,
    pub fluid: String,
}

fn resolve_bucket_fluid(names: &ContentNames, name: &str) -> Result<crate::block::Block, String> {
    names
        .blocks
        .id(name)
        .map(crate::block::Block)
        .filter(|b| b.is_fluid())
        .ok_or_else(|| format!("bucket fluid '{name}' is not a fluid block"))
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RawDroppedReaction {
    pub fluid: String,
    pub result: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub burst: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sound: Option<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RawFood {
    #[serde(default = "default_eat_ticks")]
    pub eat_ticks: u32,
    #[serde(default)]
    pub effects: Vec<RawFoodEffect>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RawFoodEffect {
    pub effect: String,
    pub ticks: u32,
}

fn default_eat_ticks() -> u32 {
    60
}

#[derive(Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum RawUseRay {
    #[default]
    Solid,
    Fluids(Vec<Block>),
}

impl RawUseRay {
    fn is_solid(&self) -> bool {
        matches!(self, RawUseRay::Solid)
    }

    fn resolve(self) -> Result<super::UseRay, String> {
        Ok(match self {
            RawUseRay::Solid => super::UseRay::Solid,
            RawUseRay::Fluids(fluids) => {
                if let Some(bad) = fluids.iter().find(|b| !b.is_fluid()) {
                    return Err(format!("use_ray fluid '{bad:?}' is not a fluid block"));
                }
                super::UseRay::Fluids(Box::leak(fluids.into_boxed_slice()))
            }
        })
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RawTool {
    pub kind: ToolKind,
    pub tier: u8,
    #[serde(default)]
    pub speed: Option<f32>,
    #[serde(default)]
    pub damage: Option<[f32; 2]>,
    #[serde(default)]
    pub knockback: Option<f32>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RawProjectile {
    #[serde(default)]
    pub gravity: Option<f32>,
    #[serde(default)]
    pub drag: Option<f32>,
    #[serde(default)]
    pub sticks: Option<bool>,
    #[serde(default)]
    pub sprite_spin: Option<f32>,
    #[serde(default)]
    pub sprite_tilt: Option<f32>,
}

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
    #[serde(default)]
    pub grip: Option<[f32; 3]>,
    #[serde(default)]
    pub third_person: Option<super::SpriteHeldPose>,
}

pub(super) fn table(
    packs: &crate::assets::PackSet,
    names: &ContentNames,
) -> Result<&'static [ItemDef], String> {
    crate::registry::read_catalog(packs, "items.json", "item", |texts| {
        parse_layers(texts, names)
    })
}

#[cfg(test)]
pub(super) fn parse(text: &str) -> Result<&'static [ItemDef], String> {
    parse_test_layers(&[text])
}

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
    let creative = super::creative::catalog(names);
    let mut texts = texts.to_vec();
    texts.push(&creative);
    let mut keys = std::collections::HashSet::new();
    let patches = std::cell::RefCell::new(Vec::new());
    let defs = crate::registry::resolve_catalog(
        &texts,
        |text| {
            crate::registry::parse_rows_with_patches(
                text,
                "items",
                "item",
                &mut patches.borrow_mut(),
            )
        },
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
            RawTaggedUse::BucketFill(b) => {
                if b.becomes.is_empty() {
                    return Err("bucket_fill: `becomes` names no fluid to scoop".into());
                }
                let fills = b
                    .becomes
                    .iter()
                    .map(|(fluid, item)| {
                        Ok((resolve_bucket_fluid(names, fluid)?, becomes_item(item)?))
                    })
                    .collect::<Result<Vec<_>, String>>()?;
                ItemUse::BucketFill {
                    fills: Box::leak(fills.into_boxed_slice()),
                }
            }
            RawTaggedUse::BucketPour(b) => ItemUse::BucketPour {
                becomes: becomes_item(&b.becomes)?,
                fluid: resolve_bucket_fluid(names, &b.fluid)?,
            },
        }),
    };
    let data = crate::registry::compile_data_map(
        names.items.name(item.0).unwrap_or(""),
        &r.data,
        patches,
    )?;
    let (creative_visible, creative_name, placement_variants) =
        super::creative::resolve(data, &r.key, block, names)?;
    let creative_only =
        crate::registry::engine_data::<bool>(data, "petramond:creative_only")?.unwrap_or(false);
    let world_tool = crate::registry::engine_data::<String>(data, "petramond:creative_tool")?
        .map(|name| &*Box::leak(name.into_boxed_str()));
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
                    sprite_spin: {
                        let spin = p.sprite_spin.unwrap_or(0.0);
                        if !spin.is_finite() || spin.abs() > 60.0 {
                            return Err(
                                "projectile sprite_spin must be finite and within ±60 rad/s".into(),
                            );
                        }
                        spin
                    },
                    sprite_tilt: {
                        let tilt = p.sprite_tilt.unwrap_or(0.0);
                        if !tilt.is_finite() || tilt.abs() > 180.0 {
                            return Err(
                                "projectile sprite_tilt must be finite and within ±180 degrees"
                                    .into(),
                            );
                        }
                        tilt.to_radians()
                    },
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
            let fluid = names
                .blocks
                .id(&dr.fluid)
                .map(crate::block::Block)
                .filter(|b| b.is_fluid())
                .ok_or_else(|| {
                    format!("dropped_reaction fluid '{}' is not a fluid block", dr.fluid)
                })?;
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
                fluid,
                result,
                burst,
                sound,
            })
        }
        None => None,
    };
    if let Some(pose) = &r.held_pose {
        if pose
            .grip
            .is_some_and(|grip| grip.iter().any(|v| !v.is_finite()))
        {
            return Err("held_pose grip must be finite".into());
        }
        if let Some(body) = pose.third_person {
            if ![body.pitch, body.yaw, body.roll, body.scale]
                .iter()
                .all(|v| v.is_finite())
                || body.scale <= 0.0
                || body.grip.iter().any(|v| !v.is_finite())
            {
                return Err("third-person sprite hold must be finite with a positive scale".into());
            }
        }
    }
    Ok(ItemDef {
        item,
        key: Box::leak(r.key.into_boxed_str()),
        name: Box::leak(creative_name.unwrap_or(r.name).into_boxed_str()),
        info: r.info.map(|info| &*Box::leak(info.into_boxed_str())),
        max_stack_size: r.max_stack_size,
        held_pose: r.held_pose.map_or(HeldPose::DEFAULT, |p| HeldPose {
            pitch: p.pitch as f32,
            yaw: p.yaw as f32,
            roll: p.roll as f32,
            grip: p.grip,
            third_person: p.third_person,
        }),
        sprite_axis_degrees,
        sprite_face_leads: r.sprite_face_leads,
        sprite,
        model: r.model,
        tags: Box::leak(tags.into_boxed_slice()),
        block,
        creative_visible,
        creative_only,
        world_tool,
        placement_variants,
        item_use,
        use_ray: r.use_ray.resolve()?,
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

    #[test]
    fn shipped_items_json_loads_fully() {
        let (text, path) =
            crate::assets::read_base_text("items.json").expect("assets/items.json must ship");
        let defs = parse(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        assert_eq!(
            defs.iter()
                .filter(|d| !d.key.starts_with(super::super::creative::PREFIX))
                .count(),
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
        assert_eq!(
            defs.iter()
                .filter(|d| !d.key.starts_with(super::super::creative::PREFIX))
                .count(),
            crate::item::ENGINE_ITEM_NAMES.len()
        );
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
        let layer = r#"{"items": [
            {"item": "mymod:filled_gadget", "key": "mymod:filled_gadget", "name": "Filled Gadget", "max_stack_size": 1, "held_pose": {"pitch": 0, "yaw": 1.8, "roll": 0}, "tags": []},
            {"item": "mymod:gadget", "key": "mymod:gadget", "name": "Gadget", "max_stack_size": 64, "held_pose": {"pitch": 0, "yaw": 1.8, "roll": 0}, "tags": [], "block": "petramond:stone", "use": {"bucket_fill": {"becomes": {"petramond:water": "mymod:filled_gadget"}}}}
        ]}"#;
        let defs = parse_test_layers(&[&base, layer]).expect("dynamic rows load");
        let engine = crate::item::ENGINE_ITEM_NAMES.len();
        assert_eq!(
            defs.iter()
                .filter(|d| !d.key.starts_with(super::super::creative::PREFIX))
                .count(),
            engine + 2,
            "fresh ids past the engine set"
        );
        let filled = defs[engine].item;
        let gadget = &defs[engine + 1];
        assert_eq!(gadget.item, ItemType((engine + 1) as u16));
        assert_eq!(gadget.block, Some(crate::block::Block::Stone));
        let Some(ItemUse::BucketFill { fills }) = gadget.item_use else {
            panic!("the gadget carries its fill use: {:?}", gadget.item_use);
        };
        assert_eq!(fills, [(crate::block::Block::Water, filled)]);
        assert_eq!(defs[ItemType::Stone.id() as usize].item, ItemType::Stone);
    }

    #[test]
    fn bare_additions_and_bad_links_are_rejected() {
        let (base, _) =
            crate::assets::read_base_text("items.json").expect("assets/items.json must ship");
        let bare = r#"{"items": [{"item": "gadget", "key": "gadget", "name": "G", "max_stack_size": 64, "held_pose": {"pitch": 0, "yaw": 1.8, "roll": 0}, "tags": []}]}"#;
        let err = parse_test_layers(&[&base, bare]).expect_err("bare additions refused");
        assert!(err.contains("gadget") && err.contains("namespace"), "{err}");
        let bad_use = r#"{"items": [{"item": "mymod:g", "key": "mymod:g", "name": "G", "max_stack_size": 64, "held_pose": {"pitch": 0, "yaw": 1.8, "roll": 0}, "tags": [], "use": "zap"}]}"#;
        let err = parse_test_layers(&[&base, bad_use]).expect_err("unknown use refused");
        assert!(err.contains("unknown use handler"), "{err}");
        let bare_bucket = r#"{"items": [{"item": "mymod:g", "key": "mymod:g", "name": "G", "max_stack_size": 64, "held_pose": {"pitch": 0, "yaw": 1.8, "roll": 0}, "tags": [], "use": "bucket_fill"}]}"#;
        let err = parse_test_layers(&[&base, bare_bucket]).expect_err("bare bucket use refused");
        assert!(err.contains("becomes"), "{err}");
        let bad_becomes = r#"{"items": [{"item": "mymod:g", "key": "mymod:g", "name": "G", "max_stack_size": 64, "held_pose": {"pitch": 0, "yaw": 1.8, "roll": 0}, "tags": [], "use": {"bucket_pour": {"becomes": "mymod:nope", "fluid": "petramond:water"}}}]}"#;
        let err = parse_test_layers(&[&base, bad_becomes]).expect_err("unknown becomes refused");
        assert!(err.contains("becomes"), "{err}");
        let bad_fluid = r#"{"items": [{"item": "mymod:g", "key": "mymod:g", "name": "G", "max_stack_size": 64, "held_pose": {"pitch": 0, "yaw": 1.8, "roll": 0}, "tags": [], "use": {"bucket_fill": {"becomes": {"petramond:stone": "mymod:g"}}}}]}"#;
        let err = parse_test_layers(&[&base, bad_fluid]).expect_err("non-fluid key refused");
        assert!(err.contains("fluid"), "{err}");
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
        let partial = format!("{{\"items\": [{row}]}}");
        assert!(parse(&partial).err().unwrap().contains("missing row"));
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
