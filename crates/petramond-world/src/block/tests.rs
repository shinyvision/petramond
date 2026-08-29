use super::{Block, BlockInteraction, BlockMaterial, ShapeFamily};
use crate::item::ItemType;

/// Light apertures are DERIVED from the shape's own occupancy, not written
/// per family: a face quadrant is open exactly where nothing seals the
/// boundary behind it. Pins the quadrant bit layout and the derivation
/// against shapes whose open half is obvious by inspection — a bottom slab
/// (open above, sealed below, upper side quadrants only) and a bottom stair
/// (sealed toward its riser, open toward its tread).
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

/// A static box set has ONE geometry source: its authored boxes. The row
/// cannot author collision (the loader refuses it), so the drawn boxes, the
/// collided boxes and the selection outline are all the same list and cannot
/// drift — and a box that declares it does not collide still draws, still
/// outlines, and still blocks light.
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
        // Drawn matter blocks light whether or not it collides: a shape whose
        // boxes cover the whole floor seals the face below it even when every
        // one of them is `collides: false` (the snow layer). A shape that only
        // DOTS its floor — a pebble field — must not, or the cell it draws
        // inside floods black.
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

/// Whether boxes resting on the cell floor cover its whole 16×16 footprint.
/// Sampled per texel, which is exact: box extents are authored in texels.
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

/// The shape-kind registry resolves every block to a valid, self-consistent
/// kind: the dense family LUT (`shape_family`) agrees with the registry row
/// (`shape_kind().family`), the payload accessors agree with the row's params,
/// and known engine blocks land in the expected family. Pins the loader's
/// shape-kind interning (`shape_kind::ShapeKindInterner`) end-to-end.
#[test]
fn shape_kinds_resolve_consistently_for_every_block() {
    for &b in Block::all() {
        let def = b.shape_kind().def();
        assert_eq!(b.shape_family(), def.family, "{b:?} LUT vs registry family");
        // The payload accessors are exactly the row's params.
        assert_eq!(b.model_kind(), def.params.model_kind(), "{b:?} model");
        // Payloads exist iff the family carries them.
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
    // Spot-check known engine blocks land in their family.
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
    // A dedup sanity check: all plain cubes share ONE shape-kind id.
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
fn door_shaped_blocks_advertise_toggle_interaction() {
    let mut checked_any = false;
    for &block in Block::all() {
        if block.shape_family() != ShapeFamily::Door {
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
        // requires_tool() is the harvest gate's condition.
        assert_eq!(
            block.requires_tool(),
            block.harvest_tier() >= 1,
            "{block:?}"
        );
        // A gated row must have a tool kind that can open it, or nothing in
        // the game can ever harvest it: `tool_power` is 0 for every kind but
        // the material's own preferred one. Material does NOT imply the gate
        // in the other direction — loose surface stone (pebbles) is stone to
        // mine and to listen to, and still comes up in a bare hand.
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
    // Leaves pair with shears too, and theirs is the pairing that PARTS the
    // block rather than speeding it up (`mining::break_time` returns 0).
    for b in [Block::OakLeaves, Block::SpruceLeaves] {
        assert_eq!(
            b.material(),
            BlockMaterial::Foliage,
            "{b:?} should be foliage"
        );
        assert_eq!(b.preferred_tool(), Some(ToolKind::Shears), "{b:?}");
        assert!(b.cut_by_preferred_tool(), "{b:?}");
    }
    // Everything a hand mines just as well has no preferred tool (glass, air).
    for b in [Block::Glass, Block::Air] {
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

/// The per-part cell-KV addressing is a stable, ROUND-TRIPPING encoding with
/// exactly one spelling per address.
///
/// It has to be: the key is what a placed slab layer's dye is stored under and
/// what the break courier reads it back from, so a base key that survives the
/// split as something else, or a second spelling of part 0, silently loses a
/// player's colour rather than failing loudly.
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
        // Part 0 IS the bare key — the whole reason existing dyed cubes and
        // saves need no migration.
        assert_eq!(part_kv_key(key, 0), key);
    }

    // A literal `#0` is not the canonical spelling of part 0, so it stays an
    // opaque key of its own rather than aliasing the bare one.
    assert_eq!(
        split_part_kv_key("mod:key#0"),
        ("mod:key#0", 0),
        "a non-canonical `#0` must not alias the bare key"
    );
    // Neither does a non-numeric or out-of-range suffix.
    for odd in ["mod:key#", "mod:key#x", "mod:key#256", "mod:key#-1"] {
        assert_eq!(split_part_kv_key(odd), (odd, 0), "{odd:?}");
    }

    // Any PART's tint feeds the mesh, so a re-mesh is queued for all of them.
    assert!(kv_key_affects_mesh(TINT_KV_KEY));
    assert!(kv_key_affects_mesh(&part_kv_key(TINT_KV_KEY, 1)));
    assert!(!kv_key_affects_mesh("mod:other"));
    assert!(!kv_key_affects_mesh(&part_kv_key("mod:other", 1)));
}
