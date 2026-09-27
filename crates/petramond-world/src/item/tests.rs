use crate::block::{Block, ShapeFamily};
use crate::tile::Tile;

use super::*;

#[test]
fn attack_damage_ranges_are_ordered_and_positive() {
    assert_eq!(attack_damage(None), (1.0, 1.0), "fist is a deterministic 1");
    assert_eq!(
        attack_damage(Some(&ItemStack::new(ItemType::Dirt, 1))),
        (1.0, 1.0),
        "a non-weapon punches like a fist"
    );
    for &it in ItemType::all() {
        let (lo, hi) = attack_damage(Some(&ItemStack::new(it, 1)));
        assert!(lo > 0.0 && lo <= hi, "{it:?}: invalid range {lo}..{hi}");
    }
    for it in [
        ItemType::DiamondPickaxe,
        ItemType::DiamondAxe,
        ItemType::DiamondShovel,
    ] {
        assert!(
            attack_damage(Some(&ItemStack::new(it, 1))).0 >= 4.0,
            "a diamond tool one-shots: {it:?}"
        );
    }
}

#[test]
fn a_stack_tool_override_changes_tier_speed_and_damage_but_never_kind() {
    let stone_pick = ItemStack::new(ItemType::StonePickaxe, 1);
    let row = stone_pick.tool().expect("stone pickaxe is a tool");

    let mut map = variant::VariantMap::new();
    map.insert(
        TOOL_DATA_KEY.to_string(),
        br#"{"tier":4,"speed":8.0,"damage":[5.0,7.0]}"#.to_vec(),
    );
    let augmented = ItemStack::with_variant(
        ItemType::StonePickaxe,
        1,
        variant::intern(&map).expect("valid variant"),
    );
    let t = augmented.tool().expect("still a tool");
    assert_eq!(t.kind, row.kind, "kind is never overridable");
    assert_eq!(t.tier, 4);
    assert_eq!(t.speed, 8.0);
    assert_eq!(t.damage, (5.0, 7.0));
    assert!(
        crate::mining::harvests(Block::DiamondOre, augmented.tool()),
        "a tier-4 override unlocks what the row's tier 2 cannot"
    );
    assert!(!crate::mining::harvests(
        Block::DiamondOre,
        stone_pick.tool()
    ));
    assert_eq!(attack_damage(Some(&augmented)), (5.0, 7.0));
}

#[test]
fn a_malformed_tool_override_degrades_to_the_rows_values() {
    let row = ItemType::StonePickaxe.tool().unwrap();
    let stack_with = |bytes: &[u8]| {
        let mut map = variant::VariantMap::new();
        map.insert(TOOL_DATA_KEY.to_string(), bytes.to_vec());
        ItemStack::with_variant(
            ItemType::StonePickaxe,
            1,
            variant::intern(&map).expect("valid variant"),
        )
    };
    assert_eq!(stack_with(b"not json").tool(), Some(row));
    let t = stack_with(br#"{"tier":3,"speed":-1.0}"#).tool().unwrap();
    assert_eq!(t.tier, 3);
    assert_eq!(t.speed, row.speed, "a non-positive speed is refused");
    let t = stack_with(br#"{"damage":[7.0,5.0],"future":1}"#)
        .tool()
        .unwrap();
    assert_eq!(t.damage, row.damage);
}

#[test]
fn item_only_items_render_as_sprites_and_carry_tools() {
    for item in [
        ItemType::Stick,
        ItemType::Pebble,
        ItemType::Rope,
        ItemType::DiamondPickaxe,
        ItemType::IronAxe,
        ItemType::DiamondShovel,
        ItemType::RawIron,
        ItemType::RawGold,
        ItemType::Diamond,
        ItemType::GoldIngot,
        ItemType::Coal,
    ] {
        assert_eq!(item.as_block(), None, "{item:?}");
        assert!(
            matches!(item.render_kind(), ItemRenderKind::Sprite(_)),
            "{item:?} should render as a sprite"
        );
    }
    use ToolKind::{Axe, Pickaxe, Shovel};
    assert_eq!(ItemType::StonePickaxe.tool(), Some(Tool::new(Pickaxe, 2)));
    assert_eq!(ItemType::IronPickaxe.tool(), Some(Tool::new(Pickaxe, 3)));
    assert_eq!(ItemType::DiamondPickaxe.tool(), Some(Tool::new(Pickaxe, 4)));
    assert_eq!(ItemType::StoneAxe.tool(), Some(Tool::new(Axe, 2)));
    assert_eq!(ItemType::DiamondAxe.tool(), Some(Tool::new(Axe, 4)));
    assert_eq!(ItemType::StoneShovel.tool(), Some(Tool::new(Shovel, 2)));
    assert_eq!(ItemType::IronShovel.tool(), Some(Tool::new(Shovel, 3)));
    assert_eq!(ItemType::DiamondShovel.tool(), Some(Tool::new(Shovel, 4)));
    assert_eq!(ItemType::Stick.tool(), None);
    assert_eq!(ItemType::Cobblestone.tool(), None);
}

#[test]
fn durable_items_do_not_stack() {
    for durable in [
        ItemType::StonePickaxe,
        ItemType::IronPickaxe,
        ItemType::DiamondPickaxe,
        ItemType::StoneAxe,
        ItemType::IronAxe,
        ItemType::DiamondAxe,
        ItemType::StoneShovel,
        ItemType::IronShovel,
        ItemType::DiamondShovel,
        ItemType::Shears,
    ] {
        assert!(durable.is_durable(), "{durable:?}");
        assert_eq!(durable.max_stack_size(), 1, "{durable:?}");
        assert_eq!(ItemStack::new(durable, 5).count, 1);
    }
    for stackable in [
        ItemType::Stick,
        ItemType::RawIron,
        ItemType::RawGold,
        ItemType::Diamond,
        ItemType::GoldIngot,
        ItemType::Cobblestone,
    ] {
        assert!(!stackable.is_durable(), "{stackable:?}");
        assert_eq!(stackable.max_stack_size(), 64, "{stackable:?}");
    }
}

#[test]
fn every_item_draws_as_itself_and_never_falls_back_to_the_stick() {
    let stick = ItemType::Stick.render_kind();
    for &it in ItemType::all() {
        if it == ItemType::Air || it == ItemType::Stick {
            continue;
        }
        assert_ne!(
            it.render_kind(),
            stick,
            "{it:?} has no sprite, block link or model, so it silently draws as a stick"
        );
    }
}

#[test]
fn the_wild_hemp_fibre_is_a_material_not_a_placeable() {
    assert_eq!(ItemType::Hemp.as_block(), None);
}

#[test]
fn item_tags_are_item_data() {
    const PLANKS: ItemTag = ItemTag::PLANKS;
    const LOGS: ItemTag = ItemTag::LOGS;
    for p in [ItemType::OakPlanks, ItemType::SprucePlanks] {
        assert!(p.has_tag(PLANKS), "{p:?}");
    }
    for log in [
        ItemType::OakLog,
        ItemType::SpruceLog,
        ItemType::BirchLog,
        ItemType::JungleLog,
        ItemType::AcaciaLog,
    ] {
        assert!(log.has_tag(LOGS), "{log:?}");
        assert!(!log.has_tag(PLANKS), "{log:?}");
    }
    assert!(!ItemType::OakLog.has_tag(PLANKS));
    assert!(!ItemType::Stick.has_tag(LOGS));
    assert!(!ItemType::Stick.has_tag(PLANKS));
    assert_eq!(ItemTag::from_key("petramond:planks"), Some(PLANKS));
    assert_eq!(ItemTag::from_key("petramond:logs"), Some(LOGS));
    assert_eq!(ItemTag::from_key("bogus"), None);

    assert!(ItemType::Coal.has_tag(ItemTag::FUEL));
    assert!(!ItemType::Coal.has_tag(ItemTag::SMELTABLE));
    assert!(ItemType::RawIron.has_tag(ItemTag::SMELTABLE));
    assert!(ItemType::RawCopper.has_tag(ItemTag::SMELTABLE));
    assert!(ItemType::Cobblestone.has_tag(ItemTag::SMELTABLE));
    assert!(!ItemType::RawIron.has_tag(ItemTag::FUEL));
    assert!(!ItemType::IronIngot.has_tag(ItemTag::SMELTABLE));
    assert!(!ItemType::IronIngot.has_tag(ItemTag::FUEL));
    assert_eq!(ItemTag::from_key("petramond:fuel"), Some(ItemTag::FUEL));
    assert_eq!(
        ItemTag::from_key("petramond:smeltable"),
        Some(ItemTag::SMELTABLE)
    );
}

#[test]
fn render_kind_matches_shape_family() {
    for &block in Block::all() {
        let item = ItemType::from_block(block);
        if item == ItemType::Air && block != Block::Air {
            continue;
        }
        match block.shape_family() {
            ShapeFamily::Cube
            | ShapeFamily::BoxSet
            | ShapeFamily::Stair
            | ShapeFamily::Slab
            | ShapeFamily::Fence => {
                let expected = match item.declared_sprite() {
                    Some(sprite) => ItemRenderKind::Sprite(sprite),
                    None => ItemRenderKind::BlockCube(block),
                };
                assert_eq!(item.render_kind(), expected, "{block:?}");
            }
            ShapeFamily::Cross | ShapeFamily::Crop => {
                let kind = item.render_kind();
                assert!(
                    matches!(kind, ItemRenderKind::Sprite(_)),
                    "{block:?} plant items render as flat sprites"
                );
                if item.declared_sprite().is_none() {
                    assert_eq!(kind, ItemRenderKind::Sprite(block.tiles()[0]), "{block:?}");
                }
            }
            ShapeFamily::Torch => {
                assert!(
                    matches!(item.render_kind(), ItemRenderKind::Sprite(_)),
                    "{block:?} renders as a flat sprite"
                );
                assert_ne!(
                    item.render_kind(),
                    ItemRenderKind::Sprite(Tile::named("stick")),
                    "{block:?} must declare its own item sprite"
                );
            }
            ShapeFamily::Model => {
                let kind = block.model_kind().expect("model family");
                match item.declared_sprite() {
                    Some(sprite) => {
                        assert_eq!(
                            item.render_kind(),
                            ItemRenderKind::Sprite(sprite),
                            "{block:?}"
                        )
                    }
                    None => {
                        assert_eq!(item.render_kind(), ItemRenderKind::Model(kind), "{block:?}")
                    }
                }
            }
            ShapeFamily::Door | ShapeFamily::Trapdoor | ShapeFamily::Pane | ShapeFamily::Ladder => {
                assert!(
                    matches!(item.render_kind(), ItemRenderKind::Sprite(_)),
                    "{block:?} renders as a flat sprite"
                );
            }
            ShapeFamily::Custom => {
                assert_eq!(
                    item.render_kind(),
                    ItemRenderKind::BlockCube(block),
                    "{block:?}"
                );
            }
        }
    }
}

#[test]
fn item_only_model_item_renders_as_its_model() {
    assert_eq!(ItemType::WoodenBucket.as_block(), None);
    assert!(matches!(
        ItemType::WoodenBucket.render_kind(),
        ItemRenderKind::Model(_)
    ));
}

#[test]
fn stack_basics() {
    let s = ItemStack::new(ItemType::Stone, 200);
    assert_eq!(s.count, 64);
    assert_eq!(s.space_left(), 0);

    let s = ItemStack::new(ItemType::Dirt, 10);
    assert!(!s.is_empty());
    assert_eq!(s.space_left(), 54);
    assert!(s.can_stack_with(&ItemStack::new(ItemType::Dirt, 1)));
    assert!(!s.can_stack_with(&ItemStack::new(ItemType::Stone, 1)));

    assert!(ItemStack::new(ItemType::Air, 5).is_empty());
    assert!(ItemStack::new(ItemType::Dirt, 0).is_empty());
}

#[test]
fn drop_spec_none_is_empty() {
    assert!(DropSpec::NONE.drops.is_empty());
}

#[test]
fn block_item_links_round_trip() {
    for &it in ItemType::all() {
        if it.creative_only() {
            if let Some(block) = it.as_block() {
                assert_eq!(ItemType::from_block(block), ItemType::Air);
            }
            continue;
        }
        if let Some(b) = it.as_block() {
            assert_eq!(
                ItemType::from_block(b),
                it,
                "{it:?} links {b:?}, but that block's item is {:?}",
                ItemType::from_block(b)
            );
        }
    }
}
