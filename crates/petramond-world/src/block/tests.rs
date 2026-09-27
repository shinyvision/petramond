use super::{Block, BlockInteraction, BlockMaterial, ShapeFamily};
use crate::item::ItemType;

/// Apertures come from shape occupancy, not per-family data. A quadrant is open where nothing
/// seals behind it. Uses a bottom slab (open above, sealed below) and a bottom stair (sealed
/// toward its riser, open toward its tread) since their open half is obvious by inspection.
#[test]
fn light_apertures_are_derived_from_the_shape_occupancy() {
    use crate::block::{light_aperture_face, CellCodec, ShapeNeighborhood, ShapeState};
    use crate::block_state::{SlabSplit, SlabState, StairHalf, StairState};
    use crate::facing::Facing;
    use crate::mathh::IVec3;

    struct OneCell(Block, ShapeState);
    impl ShapeNeighborhood for OneCell {
        fn block(&self, _pos: IVec3) -> Block {
            self.0
        }
        fn shape_state(&self, _pos: IVec3) -> ShapeState {
            self.1
        }
    }
    let masks = |block: Block, state: ShapeState| {
        let k = block.shape_kind().def();
        k.sim
            .light_apertures(&k.params, &OneCell(block, state), IVec3::ZERO, block)
    };
    let face = |block, state, dir| light_aperture_face(masks(block, state), dir);

    let slab = SlabState::single(SlabSplit::Y, 0, Block::DirtSlab).to_cell();
    assert_eq!(face(Block::DirtSlab, slab, (0, -1, 0)), 0, "sealed below");
    assert_eq!(face(Block::DirtSlab, slab, (0, 1, 0)), 0b1111, "open above");
    assert_eq!(
        face(Block::DirtSlab, slab, (1, 0, 0)),
        0b1100,
        "only the upper side quadrants are open"
    );
    let full = SlabState {
        split: SlabSplit::Y,
        layers: [Block::DirtSlab, Block::StoneSlab],
    }
    .to_cell();
    assert_eq!(
        face(Block::DirtSlab, full, (0, 1, 0)),
        0,
        "a full stack seals"
    );

    let east = StairState::new(Facing::East, StairHalf::Bottom).to_cell();
    assert_eq!(
        face(Block::OakStairs, east, (0, -1, 0)),
        0,
        "solid underside"
    );
    assert_eq!(face(Block::OakStairs, east, (-1, 0, 0)), 0, "riser side");
    assert_ne!(face(Block::OakStairs, east, (1, 0, 0)), 0, "tread side");
    assert_ne!(
        face(Block::OakStairs, east, (0, 1, 0)),
        0,
        "open over tread"
    );
}

#[test]
fn a_box_set_derives_every_box_from_its_authored_shape() {
    let (mut checked, mut sealing) = (0, 0);
    for &b in Block::all() {
        let Some(set) = b.shape_kind().def().params.box_set() else {
            continue;
        };
        checked += 1;
        assert_eq!(
            b.visual_aabb(),
            Some((set.bounds(0, 0).min, set.bounds(0, 0).max)),
            "{b:?} outlines the drawn union"
        );
        let collision: Vec<_> = b.collision_boxes().to_vec();
        let expect: Vec<_> = set
            .boxes(0, 0)
            .iter()
            .filter(|d| d.collides)
            .map(|d| d.aabb)
            .collect();
        assert_eq!(collision, expect, "{b:?} collides as its colliding boxes");
        if floor_fully_covered(set.boxes(0, 0)) {
            let sealed = crate::block::light_aperture_face(b.default_light_apertures(), (0, -1, 0));
            assert_eq!(sealed, 0, "{b:?} full-floor box must block light downward");
            sealing += 1;
        }
    }
    assert!(checked >= 2, "expected the engine's own box-set rows");
    assert!(
        sealing >= 1,
        "expected a shipped box set that covers its floor"
    );
}

fn floor_fully_covered(boxes: &[crate::block::shape_kind::BoxDef]) -> bool {
    (0..16).all(|tz| {
        (0..16).all(|tx| {
            let (x, z) = ((tx as f32 + 0.5) / 16.0, (tz as f32 + 0.5) / 16.0);
            boxes.iter().any(|d| {
                d.aabb.min[1] <= 0.0
                    && d.aabb.min[0] <= x
                    && x <= d.aabb.max[0]
                    && d.aabb.min[2] <= z
                    && z <= d.aabb.max[2]
            })
        })
    })
}

#[test]
fn shape_kinds_resolve_consistently_for_every_block() {
    for &b in Block::all() {
        let def = b.shape_kind().def();
        assert_eq!(b.model_kind(), def.params.model_kind(), "{b:?} model");
        assert_eq!(
            def.params.box_set().is_some(),
            b.shape_family() == ShapeFamily::BoxSet,
            "{b:?} box payload matches family"
        );
        assert_eq!(
            b.model_kind().is_some(),
            b.shape_family() == ShapeFamily::Model,
            "{b:?} model payload matches family"
        );
    }
    for (b, family) in [
        (Block::Stone, ShapeFamily::Cube),
        (Block::ShortGrass, ShapeFamily::Cross),
        (Block::Torch, ShapeFamily::Torch),
        (Block::OakStairs, ShapeFamily::Stair),
        (Block::OakSlab, ShapeFamily::Slab),
        (Block::GlassPane, ShapeFamily::Pane),
        (Block::OakFence, ShapeFamily::Fence),
        (Block::Ladder, ShapeFamily::Ladder),
        (Block::OakDoor, ShapeFamily::Door),
        (Block::SnowLayer, ShapeFamily::BoxSet),
        (Block::Cactus, ShapeFamily::BoxSet),
        (Block::Bed, ShapeFamily::Model),
    ] {
        assert_eq!(b.shape_family(), family, "{b:?}");
    }
    assert_eq!(
        Block::Stone.shape_kind(),
        Block::Dirt.shape_kind(),
        "plain cubes share one shape kind"
    );
}

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
fn hinged_panel_shapes_advertise_their_toggle_interaction() {
    for (family, expected) in [
        (ShapeFamily::Door, BlockInteraction::ToggleDoor),
        (ShapeFamily::Trapdoor, BlockInteraction::ToggleTrapdoor),
    ] {
        let mut checked_any = false;
        for &block in Block::all() {
            if block.shape_family() != family {
                continue;
            }
            checked_any = true;
            assert_eq!(block.interaction(), expected, "{block:?}");
        }
        assert!(checked_any, "expected at least one {family:?} block");
    }
}

#[test]
fn every_block_has_consistent_metadata() {
    for &block in Block::all() {
        let spec = block.drop_spec();
        for d in spec.drops {
            assert_ne!(d.item, ItemType::Air, "{block:?} drops Air");
            assert!(
                d.min >= 1 && d.min <= d.max,
                "{block:?} bad drop count {}..{}",
                d.min,
                d.max
            );
        }
        assert_eq!(
            block.requires_tool(),
            block.harvest_tier() >= 1,
            "{block:?}"
        );
        if block.requires_tool() {
            assert!(
                block.preferred_tool().is_some(),
                "{block:?} is tool-gated with no tool that fits it"
            );
        }
    }
}

#[test]
fn preferred_tool_pairs_pickaxe_axe_shovel_with_their_materials() {
    use crate::item::ToolKind;
    for b in [
        Block::Stone,
        Block::Cobblestone,
        Block::CoalOre,
        Block::DiamondOre,
    ] {
        assert_eq!(b.preferred_tool(), Some(ToolKind::Pickaxe), "{b:?}");
    }
    for b in [
        Block::OakLog,
        Block::OakPlanks,
        Block::CraftingTable,
        Block::Chest,
    ] {
        assert_eq!(b.material(), BlockMaterial::Wood, "{b:?} should be wood");
        assert_eq!(b.preferred_tool(), Some(ToolKind::Axe), "{b:?}");
    }
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
            matches!(
                b.material(),
                BlockMaterial::Dirt | BlockMaterial::Sand | BlockMaterial::Snow
            ),
            "{b:?} should be dirt/sand/snow"
        );
        assert_eq!(b.preferred_tool(), Some(ToolKind::Shovel), "{b:?}");
    }
    for b in [Block::WoolBlock, Block::WoolStairs, Block::WoolSlab] {
        assert_eq!(b.material(), BlockMaterial::Wool, "{b:?} should be wool");
        assert_eq!(b.preferred_tool(), Some(ToolKind::Shears), "{b:?}");
    }
    for b in [Block::Poppy, Block::ShortGrass] {
        assert_eq!(b.material(), BlockMaterial::Plant, "{b:?} should be plant");
        assert_eq!(b.preferred_tool(), Some(ToolKind::Shears), "{b:?}");
    }
    for b in [Block::OakLeaves, Block::SpruceLeaves] {
        assert_eq!(
            b.material(),
            BlockMaterial::Foliage,
            "{b:?} should be foliage"
        );
        assert_eq!(b.preferred_tool(), Some(ToolKind::Shears), "{b:?}");
        assert!(b.cut_by_preferred_tool(), "{b:?}");
    }
    for b in [Block::Glass, Block::Air] {
        assert_eq!(b.preferred_tool(), None, "{b:?}");
    }
    let shears = crate::item::ItemType::Shears.tool();
    assert!(!crate::mining::harvests(Block::ShortGrass, None));
    assert!(crate::mining::harvests(Block::ShortGrass, shears));
    assert!(crate::mining::harvests(Block::Poppy, None));
}

#[test]
fn a_melting_block_leaves_its_fluid_only_over_support() {
    let (melting, fluid) = Block::all()
        .iter()
        .find_map(|&b| Some((b, b.melts_to()?)))
        .expect("a shipped row melts");
    let solid = Block::all()
        .iter()
        .copied()
        .find(|b| b.is_solid() && b.melts_to().is_none())
        .unwrap();
    assert_eq!(melting.break_residue(fluid), fluid);
    assert_eq!(melting.break_residue(solid), fluid);
    assert_eq!(
        melting.break_residue(Block::Air),
        Block::Air,
        "no floating source over a void"
    );
    assert_eq!(solid.break_residue(fluid), Block::Air);
}

#[test]
fn is_terrain_solid_is_the_bare_ground_set() {
    let terrain = [Block::Stone, Block::Dirt, Block::Grass, Block::Sand];
    for &b in &terrain {
        assert!(b.is_terrain_solid(), "{b:?} should be terrain-solid");
    }
    for &b in Block::all() {
        let expected = terrain.contains(&b);
        assert_eq!(b.is_terrain_solid(), expected, "{b:?}");
    }
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

#[test]
fn part_kv_keys_round_trip_with_one_spelling_per_address() {
    use super::{kv_key_affects_mesh, part_kv_key, split_part_kv_key, TINT_KV_KEY};

    for key in [TINT_KV_KEY, "mod:some_key", "mod:key#with#hashes"] {
        for part in [0u8, 1, 2, 255] {
            let stored = part_kv_key(key, part);
            assert_eq!(
                split_part_kv_key(&stored),
                (key, part),
                "{key:?} part {part} must survive the round trip"
            );
        }
        assert_eq!(part_kv_key(key, 0), key);
    }

    assert_eq!(
        split_part_kv_key("mod:key#0"),
        ("mod:key#0", 0),
        "a non-canonical `#0` must not alias the bare key"
    );
    for odd in ["mod:key#", "mod:key#x", "mod:key#256", "mod:key#-1"] {
        assert_eq!(split_part_kv_key(odd), (odd, 0), "{odd:?}");
    }

    assert!(kv_key_affects_mesh(TINT_KV_KEY));
    assert!(kv_key_affects_mesh(&part_kv_key(TINT_KV_KEY, 1)));
    assert!(!kv_key_affects_mesh("mod:other"));
    assert!(!kv_key_affects_mesh(&part_kv_key("mod:other", 1)));
}
