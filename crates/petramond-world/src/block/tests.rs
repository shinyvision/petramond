use super::{Block, ShapeFamily};
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
    let mut checked = 0;
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
        }
    }
    assert!(checked > 0, "expected the engine's own box-set rows");
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
    assert_eq!(
        Block::Stone.shape_kind(),
        Block::Dirt.shape_kind(),
        "plain cubes share one shape kind"
    );
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
    }
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
