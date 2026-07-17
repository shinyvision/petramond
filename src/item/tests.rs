use crate::atlas::Tile;
use crate::block::{Block, RenderShape};

use super::*;

#[test]
fn attack_damage_ranges_are_ordered_and_positive() {
    // Mechanic, not the tuned numbers (which are free to change): an empty hand and
    // a non-weapon item both punch for exactly 1, and every item's range is a valid,
    // positive `lo <= hi`.
    assert_eq!(attack_damage(None), (1.0, 1.0), "fist is a deterministic 1");
    assert_eq!(
        attack_damage(Some(ItemType::Dirt)),
        (1.0, 1.0),
        "a non-weapon punches like a fist"
    );
    for &it in ItemType::all() {
        let (lo, hi) = attack_damage(Some(it));
        assert!(lo > 0.0 && lo <= hi, "{it:?}: invalid range {lo}..{hi}");
    }
    // Every diamond tool one-shots a 4-health mob (its minimum damage alone is lethal).
    for it in [
        ItemType::DiamondPickaxe,
        ItemType::DiamondAxe,
        ItemType::DiamondShovel,
    ] {
        assert!(
            attack_damage(Some(it)).0 >= 4.0,
            "a diamond tool one-shots: {it:?}"
        );
    }
}

#[test]
fn item_only_items_render_as_sprites_and_carry_tools() {
    for item in [
        ItemType::Stick,
        ItemType::WoodenPickaxe,
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
    // Tools carry a kind + tier (gating mining); non-tools carry none. The
    // three families share the 1..=4 tier ladder (wooden, stone, iron, diamond).
    use ToolKind::{Axe, Pickaxe, Shovel};
    assert_eq!(
        ItemType::WoodenPickaxe.tool(),
        Some(Tool {
            kind: Pickaxe,
            tier: 1
        })
    );
    assert_eq!(
        ItemType::StonePickaxe.tool(),
        Some(Tool {
            kind: Pickaxe,
            tier: 2
        })
    );
    assert_eq!(
        ItemType::IronPickaxe.tool(),
        Some(Tool {
            kind: Pickaxe,
            tier: 3
        })
    );
    assert_eq!(
        ItemType::DiamondPickaxe.tool(),
        Some(Tool {
            kind: Pickaxe,
            tier: 4
        })
    );
    assert_eq!(
        ItemType::WoodenAxe.tool(),
        Some(Tool { kind: Axe, tier: 1 })
    );
    assert_eq!(
        ItemType::DiamondAxe.tool(),
        Some(Tool { kind: Axe, tier: 4 })
    );
    assert_eq!(
        ItemType::WoodenShovel.tool(),
        Some(Tool {
            kind: Shovel,
            tier: 1
        })
    );
    assert_eq!(
        ItemType::StoneShovel.tool(),
        Some(Tool {
            kind: Shovel,
            tier: 2
        })
    );
    assert_eq!(
        ItemType::IronShovel.tool(),
        Some(Tool {
            kind: Shovel,
            tier: 3
        })
    );
    assert_eq!(
        ItemType::DiamondShovel.tool(),
        Some(Tool {
            kind: Shovel,
            tier: 4
        })
    );
    assert_eq!(ItemType::Stick.tool(), None);
    assert_eq!(ItemType::Cobblestone.tool(), None);
}

#[test]
fn durable_items_do_not_stack() {
    // The stack limit of 1 follows from durability, not from being a "tool".
    // Every mining tool — pickaxes, axes, shovels and shears — is durable.
    for durable in [
        ItemType::WoodenPickaxe,
        ItemType::StonePickaxe,
        ItemType::IronPickaxe,
        ItemType::DiamondPickaxe,
        ItemType::WoodenAxe,
        ItemType::StoneAxe,
        ItemType::IronAxe,
        ItemType::DiamondAxe,
        ItemType::WoodenShovel,
        ItemType::StoneShovel,
        ItemType::IronShovel,
        ItemType::DiamondShovel,
        ItemType::Shears,
    ] {
        assert!(durable.is_durable(), "{durable:?}");
        assert_eq!(durable.max_stack_size(), 1, "{durable:?}");
        // ItemStack clamps to the durable limit.
        assert_eq!(ItemStack::new(durable, 5).count, 1);
    }
    // Non-durable items keep their table stack size (sticks, raw drops, gems,
    // ingots, blocks).
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
    // Sticks are neither logs nor planks.
    assert!(!ItemType::OakLog.has_tag(PLANKS));
    assert!(!ItemType::Stick.has_tag(LOGS));
    assert!(!ItemType::Stick.has_tag(PLANKS));
    // Tag names resolve from the recipe key.
    assert_eq!(ItemTag::from_key("petramond:planks"), Some(PLANKS));
    assert_eq!(ItemTag::from_key("petramond:logs"), Some(LOGS));
    assert_eq!(ItemTag::from_key("bogus"), None);

    // Furnace routing tags: coal is fuel; raw ores are smeltable; the products
    // are neither (so a finished ingot doesn't shift back into the furnace).
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
fn render_kind_matches_render_shape() {
    for &block in Block::all() {
        let item = ItemType::from_block(block);
        // A dynamic block with no linked item (e.g. a machine's lit
        // variant) maps to Air — there is no item whose render kind could
        // mirror the block's shape.
        if item == ItemType::Air && block != Block::Air {
            continue;
        }
        match block.render_shape() {
            RenderShape::Cube => {
                assert_eq!(
                    item.render_kind(),
                    ItemRenderKind::BlockCube(block),
                    "{block:?}"
                );
            }
            RenderShape::LoweredCube(_) => {
                assert_eq!(
                    item.render_kind(),
                    ItemRenderKind::BlockCube(block),
                    "{block:?}"
                );
            }
            RenderShape::Stair => {
                assert_eq!(
                    item.render_kind(),
                    ItemRenderKind::BlockCube(block),
                    "{block:?}"
                );
            }
            RenderShape::Slab => {
                assert_eq!(
                    item.render_kind(),
                    ItemRenderKind::BlockCube(block),
                    "{block:?}"
                );
            }
            RenderShape::Cross => {
                assert_eq!(
                    item.render_kind(),
                    ItemRenderKind::Sprite(block.tiles()[0]),
                    "{block:?}"
                );
            }
            // Crop planters normally carry their own sprite (which wins);
            // either way the ITEM is always a flat sprite, never a cube.
            RenderShape::Crop => {
                assert!(
                    matches!(item.render_kind(), ItemRenderKind::Sprite(_)),
                    "{block:?} crop items render as flat sprites"
                );
            }
            RenderShape::Torch => {
                assert_eq!(
                    item.render_kind(),
                    ItemRenderKind::Sprite(Tile::named("torch")),
                    "{block:?}"
                );
            }
            RenderShape::Model(kind) => {
                assert_eq!(item.render_kind(), ItemRenderKind::Model(kind), "{block:?}");
            }
            RenderShape::Door => {
                assert!(
                    matches!(item.render_kind(), ItemRenderKind::Sprite(_)),
                    "{block:?} door renders as a flat sprite"
                );
            }
            RenderShape::Pane => {
                assert!(
                    matches!(item.render_kind(), ItemRenderKind::Sprite(_)),
                    "{block:?} pane renders as a flat sprite"
                );
            }
            RenderShape::Ladder => {
                assert!(
                    matches!(item.render_kind(), ItemRenderKind::Sprite(_)),
                    "{block:?} ladder renders as a flat sprite"
                );
            }
        }
    }
}

#[test]
fn item_only_model_item_renders_as_its_model() {
    // The bucket has no block, but must NOT fall back to a flat sprite: the
    // held / dropped / icon paths all key off the Model render kind.
    assert_eq!(ItemType::WoodenBucket.as_block(), None);
    assert!(matches!(
        ItemType::WoodenBucket.render_kind(),
        ItemRenderKind::Model(_)
    ));
}

#[test]
fn stack_basics() {
    // new clamps to max stack size.
    let s = ItemStack::new(ItemType::Stone, 200);
    assert_eq!(s.count, 64);
    assert_eq!(s.space_left(), 0);

    let s = ItemStack::new(ItemType::Dirt, 10);
    assert!(!s.is_empty());
    assert_eq!(s.space_left(), 54);
    assert!(s.can_stack_with(&ItemStack::new(ItemType::Dirt, 1)));
    assert!(!s.can_stack_with(&ItemStack::new(ItemType::Stone, 1)));

    // Empty cases.
    assert!(ItemStack::new(ItemType::Air, 5).is_empty());
    assert!(ItemStack::new(ItemType::Dirt, 0).is_empty());
}

#[test]
fn drop_spec_none_is_empty() {
    assert!(DropSpec::NONE.drops.is_empty());
}

/// Every placeable item's block maps back to it: a row accidentally
/// linking a block some other item already links (a copy-paste in
/// `items.json`) would silently make the later item's placed block
/// hand back the wrong item when broken.
#[test]
fn block_item_links_round_trip() {
    for &it in ItemType::all() {
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
