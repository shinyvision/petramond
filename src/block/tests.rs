use super::{Block, BlockInteraction, BlockMaterial, RenderShape};
use crate::item::ItemType;

#[test]
fn directional_view_is_block_data_for_blocks_with_a_front() {
    for block in [Block::Furnace, Block::Chest, Block::FurnitureWorkbench] {
        assert!(
            block.directional_view(),
            "{block:?} should face the player on placement"
        );
    }
    for block in [Block::CraftingTable, Block::Torch, Block::Stone] {
        assert!(
            !block.directional_view(),
            "{block:?} has no authored front view"
        );
    }
}

#[test]
fn door_shaped_blocks_advertise_toggle_interaction() {
    let mut checked_any = false;
    for &block in Block::all() {
        if block.render_shape() != RenderShape::Door {
            continue;
        }
        checked_any = true;
        assert_eq!(
            block.interaction(),
            BlockInteraction::ToggleDoor,
            "{block:?}"
        );
    }
    assert!(checked_any, "expected at least one door block");
}

#[test]
fn every_block_has_consistent_metadata() {
    for &block in Block::all() {
        let spec = block.drop_spec();
        // Every dropped item is a real (non-Air) item with a sane count range.
        for d in spec.drops {
            assert_ne!(d.item, ItemType::Air, "{block:?} drops Air");
            assert!(
                d.min >= 1 && d.min <= d.max,
                "{block:?} bad drop count {}..{}",
                d.min,
                d.max
            );
        }
        // requires_tool() is the harvest gate's condition. Every Stone/Ore
        // material block is tool-gated (needs at least a wooden pickaxe);
        // soft blocks may opt in too (the snow layer's shovel-gated drop).
        assert_eq!(
            block.requires_tool(),
            block.harvest_tier() >= 1,
            "{block:?}"
        );
        if matches!(block.material(), BlockMaterial::Stone | BlockMaterial::Ore) {
            assert!(block.requires_tool(), "{block:?}");
        }
    }
}

#[test]
fn preferred_tool_pairs_pickaxe_axe_shovel_with_their_materials() {
    use crate::item::ToolKind;
    // Stone & ore want a pickaxe.
    for b in [
        Block::Stone,
        Block::Cobblestone,
        Block::CoalOre,
        Block::DiamondOre,
    ] {
        assert_eq!(b.preferred_tool(), Some(ToolKind::Pickaxe), "{b:?}");
    }
    // Wood wants an axe — logs and planks, AND (sanity check) the crafting
    // table and chest, which are Wood-material blocks.
    for b in [
        Block::OakLog,
        Block::OakPlanks,
        Block::CraftingTable,
        Block::Chest,
    ] {
        assert_eq!(b.material(), BlockMaterial::Wood, "{b:?} should be wood");
        assert_eq!(b.preferred_tool(), Some(ToolKind::Axe), "{b:?}");
    }
    // Dirt & sand want a shovel — the soft cover blocks (grass, podzol, gravel,
    // clay, snow). All but the snow layer are hand-harvestable, so the shovel
    // is a pure speed bonus there; the snow layer's snowball drop is
    // shovel-gated (harvest tier 1).
    for b in [
        Block::Dirt,
        Block::Grass,
        Block::Podzol,
        Block::Sand,
        Block::Gravel,
        Block::Clay,
        Block::SnowLayer,
    ] {
        assert!(
            matches!(b.material(), BlockMaterial::Dirt | BlockMaterial::Sand),
            "{b:?} should be dirt/sand"
        );
        assert_eq!(b.preferred_tool(), Some(ToolKind::Shovel), "{b:?}");
    }
    // Wool and plants want shears — the wool block family, and the cut
    // plants. For tier-0 plants the pairing is inert (hand-harvested
    // instantly); short grass raises its harvest tier, making it the
    // CUT-ONLY yield that feeds pasture-building (a bare hand destroys it
    // dropless, like the snow layer without a shovel).
    for b in [Block::WoolBlock, Block::WoolStairs, Block::WoolSlab] {
        assert_eq!(b.material(), BlockMaterial::Wool, "{b:?} should be wool");
        assert_eq!(b.preferred_tool(), Some(ToolKind::Shears), "{b:?}");
    }
    for b in [Block::Poppy, Block::ShortGrass] {
        assert_eq!(b.material(), BlockMaterial::Plant, "{b:?} should be plant");
        assert_eq!(b.preferred_tool(), Some(ToolKind::Shears), "{b:?}");
    }
    // Everything a hand mines just as well has no preferred tool (leaves, air).
    for b in [Block::OakLeaves, Block::Air] {
        assert_eq!(b.preferred_tool(), None, "{b:?}");
    }
    // The shears harvest gate itself: bare-handed short grass is destroyed
    // dropless, any shears cut it whole; every other plant stays
    // hand-harvestable.
    let shears = crate::item::ItemType::Shears.tool();
    assert!(!crate::mining::harvests(Block::ShortGrass, None));
    assert!(crate::mining::harvests(Block::ShortGrass, shears));
    assert!(crate::mining::harvests(Block::Poppy, None));
}

/// Asset↔shader contract, both directions. OPAQUE rows: every referenced
/// tile must be genuinely opaque (min alpha ≥ 128, comfortably above the
/// cutout passes' 0.25 discard) or the block renders as an invisible
/// x-ray hole — the mesher culled the faces behind it, then the shader
/// discarded every texel of its own (the 2026-07-16 ice bug: `ice.png` at
/// uniform alpha 126). TRANSLUCENT rows: tiles must sit in the 0.25..0.5
/// band — at or above the cutout discard so item cubes/icons/particles
/// still draw them solid, and below 0.5 so `fs_transparent`'s water/ice
/// split hands them their own authored alpha instead of water's constant.
#[test]
fn block_tiles_match_their_render_pass_alpha_contract() {
    for &b in Block::all() {
        for tile in b.tiles() {
            if b.is_opaque() {
                assert!(
                    tile.min_alpha() >= 128,
                    "opaque {b:?} tile '{}' has sub-opaque texels (min alpha {})",
                    tile.name(),
                    tile.min_alpha(),
                );
            } else if b.is_translucent() {
                assert!(
                    (64..128).contains(&tile.min_alpha()),
                    "translucent {b:?} tile '{}' must author alpha in 0.25..0.5 \
                     (min alpha {})",
                    tile.name(),
                    tile.min_alpha(),
                );
            }
        }
    }
}

/// The melt rule: broken ice leaves water wherever something below can
/// hold it, air over a void; nothing else ever leaves residue. Mining the
/// frozen sea must refill (water cannot flow back upward into the hole).
#[test]
fn broken_ice_melts_to_water_only_over_support() {
    assert_eq!(Block::Ice.break_residue(Block::Water), Block::Water);
    assert_eq!(Block::Ice.break_residue(Block::Stone), Block::Water);
    assert_eq!(
        Block::Ice.break_residue(Block::Air),
        Block::Air,
        "no floating water over a void"
    );
    // Packed ice is a crafted building block: it breaks clean.
    assert_eq!(Block::PackedIce.break_residue(Block::Water), Block::Air);
    assert_eq!(Block::Stone.break_residue(Block::Water), Block::Air);
}

#[test]
fn is_terrain_solid_is_the_bare_ground_set() {
    // Exactly the natural ground blocks — the set the genmap audits treat as
    // terrain (excludes logs/leaves and built blocks).
    // The snow layer is deliberately NOT in the set: it is decorative cover
    // above the surface, not load-bearing ground, so the debris audits
    // ignore it.
    let terrain = [Block::Stone, Block::Dirt, Block::Grass, Block::Sand];
    for &b in &terrain {
        assert!(b.is_terrain_solid(), "{b:?} should be terrain-solid");
    }
    for &b in Block::all() {
        let expected = terrain.contains(&b);
        assert_eq!(b.is_terrain_solid(), expected, "{b:?}");
    }
    // Notably NOT terrain even though solid: tree parts and built blocks.
    for b in [
        Block::OakLog,
        Block::OakLeaves,
        Block::Cobblestone,
        Block::Sandstone,
        Block::Water,
        Block::Air,
    ] {
        assert!(!b.is_terrain_solid(), "{b:?} should NOT be terrain-solid");
    }
}
