use crate::atlas::Tile;
use crate::block::{Block, ShapeFamily};

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
    // Tools carry a kind + tier (gating mining); non-tools carry none. The
    // three families share one tier ladder; rung 1 is vacant since the wooden
    // tools were retired (shears still sit on it), so stone is the entry tool.
    use ToolKind::{Axe, Pickaxe, Shovel};
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
    assert_eq!(ItemType::StoneAxe.tool(), Some(Tool { kind: Axe, tier: 2 }));
    assert_eq!(
        ItemType::DiamondAxe.tool(),
        Some(Tool { kind: Axe, tier: 4 })
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

/// EVERY item must own an art source, because the fallback is SILENT.
///
/// `item_sprite` falls back to the stick tile when a row declares no sprite,
/// and a row with no `block` link and no `model` reaches it — so an item with
/// nothing to draw does not fail, it quietly becomes a stick in the icon, the
/// hand, and on the ground. That shipped once: taking the `block` link off
/// `petramond:hemp` to stop it being replantable left the row with no art at
/// all, and harvested hemp drew as sticks (2026-07-31).
///
/// The item is fine either way — this is purely how it LOOKS, which is exactly
/// why nothing else catches it.
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

/// A plant the WORLD hands out for free must not be re-placeable.
///
/// Wild hemp drops its fibre, and the farming pack rolls a seed bonus on the
/// break. While the fibre could be planted back, that bonus was a faucet:
/// place, break, repeat, infinite seeds — the 2026-07-31 report. Net-zero drops
/// are not enough on their own; ANY per-break bonus turns a replantable wild
/// plant into a mint. Every other wild crop already answers this the same way
/// (no item links `farming:wild_wheat`/`wild_carrots`/`wild_potatoes`), so this
/// pins the one the engine owns.
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
fn render_kind_matches_shape_family() {
    for &block in Block::all() {
        let item = ItemType::from_block(block);
        // A dynamic block with no linked item (e.g. a machine's lit
        // variant) maps to Air — there is no item whose render kind could
        // mirror the block's shape.
        if item == ItemType::Air && block != Block::Air {
            continue;
        }
        match block.shape_family() {
            // Cube-drawn families (the plain cube, the true-geometry stair/slab/
            // fence, and the lowered cube) render as a block cube in the slot.
            ShapeFamily::Cube
            | ShapeFamily::BoxSet
            | ShapeFamily::Stair
            | ShapeFamily::Slab
            | ShapeFamily::Fence => {
                assert_eq!(
                    item.render_kind(),
                    ItemRenderKind::BlockCube(block),
                    "{block:?}"
                );
            }
            // Both plant families: the ITEM is always a flat sprite, never a
            // cube. It shows the block's own art UNLESS the item row declares a
            // sprite, which wins everywhere the item appears — that is how hemp
            // seeds show seeds rather than the plant they grow into.
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
            // A torch draws its OWN row art. Pinning the engine's `torch` tile
            // here asserted that only one torch can exist, which the family
            // never promised — packs ship their own (coloured flames). What
            // must hold is that the row DECLARES a sprite: an `ItemSprite`
            // shape with none falls back to the stick, silently.
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
                assert_eq!(item.render_kind(), ItemRenderKind::Model(kind), "{block:?}");
            }
            // The thin / flat-art shapes render as a flat sprite (their row art).
            ShapeFamily::Door | ShapeFamily::Pane | ShapeFamily::Ladder => {
                assert!(
                    matches!(item.render_kind(), ItemRenderKind::Sprite(_)),
                    "{block:?} renders as a flat sprite"
                );
            }
            // A custom shape's item defaults to a cube icon (a mod can
            // bake its own item form; the default `ShapeRender` is a cube).
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
