use std::collections::BTreeMap;

use petramond_math::math::IVec3;
use petramond_world::block::Block;
use petramond_world::chunk::{Chunk, ChunkPos};
use petramond_world::construction::{plan, Plan, Record};
use petramond_world::item::{ItemStack, ItemType};
use petramond_world::world::placement::authored::{Inputs, Turn};

use super::CellStatus;
use crate::world::World;

fn world() -> World {
    let mut w = World::new(1, 1);
    w.insert_chunk_for_test(ChunkPos::new(0, 0), Chunk::new(0, 0));
    for x in 0..16 {
        for z in 0..16 {
            w.set_block_world(x, 63, z, Block::Stone);
        }
    }
    w
}

/// Build `block` at `at` with authored properties, the way a structure does.
fn author(w: &mut World, block: Block, at: IVec3, props: &[(&str, &str)]) {
    let props: BTreeMap<String, String> = props
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    let mut inputs = Inputs::new(at, Turn::default(), &props);
    let plan = block
        .shape_kind_def()
        .placement
        .authored_plan(block, &mut inputs)
        .expect("authored plan");
    assert!(w.commit_placement(&plan, true));
}

#[test]
fn a_neighbour_derived_shape_still_counts_as_built() {
    let mut w = world();
    let at = IVec3::new(4, 64, 4);
    author(&mut w, Block::OakStairs, at, &[("facing", "north")]);
    let design = Record::at(&w, at);
    // A second stair beside it turns the first into a corner: its stored
    // shape changes, its authored intent does not.
    let neighbour = [IVec3::X, IVec3::NEG_X, IVec3::Z, IVec3::NEG_Z]
        .into_iter()
        .flat_map(|d| ["north", "south", "east", "west"].map(|f| (d, f)))
        .find(|&(d, f)| {
            let mut probe = world();
            author(&mut probe, Block::OakStairs, at, &[("facing", "north")]);
            author(&mut probe, Block::OakStairs, at + d, &[("facing", f)]);
            Record::at(&probe, at).state != design.state
        })
        .expect("some neighbour turns a stair into a corner");
    author(
        &mut w,
        Block::OakStairs,
        at + neighbour.0,
        &[("facing", neighbour.1)],
    );
    assert_ne!(
        Record::at(&w, at).state,
        design.state,
        "the neighbour refined it"
    );
    assert_eq!(w.construction_status(at, &design), CellStatus::Satisfied);

    let mut other = world();
    author(&mut other, Block::OakStairs, at, &[("facing", "south")]);
    assert!(matches!(
        other.construction_status(at, &design),
        CellStatus::Clear {
            block: Block::OakStairs,
            ..
        }
    ));
}

#[test]
fn a_door_is_one_paid_object_and_standing_open_is_still_built() {
    let mut w = world();
    let at = IVec3::new(6, 64, 6);
    author(
        &mut w,
        Block::OakDoor,
        at,
        &[("facing", "east"), ("open", "true")],
    );
    let lower = Record::at(&w, at);
    let upper = Record::at(&w, at + IVec3::Y);
    let Plan::Unit { cost, writes } = plan(&lower, at) else {
        panic!("the lower half anchors the door");
    };
    assert_eq!(cost.len(), 1);
    assert_eq!(cost[0].count, 1);
    assert_eq!(writes.writes.len(), 2, "the whole door is one write");
    assert_eq!(plan(&upper, at + IVec3::Y), Plan::Member(at));
    assert_eq!(w.construction_status(at, &lower), CellStatus::Satisfied);

    let empty = world();
    assert!(matches!(
        empty.construction_status(at, &lower),
        CellStatus::Place { .. }
    ));
    assert_eq!(
        empty.construction_status(at + IVec3::Y, &upper),
        CellStatus::Pending(at)
    );
}

#[test]
fn a_half_built_cell_pays_only_for_its_missing_part() {
    let mut w = world();
    let at = IVec3::new(8, 64, 8);
    author(&mut w, Block::OakSlab, at, &[("half", "bottom")]);
    let state = petramond_world::block_state::SlabState::single(
        petramond_world::block_state::SlabSplit::Y,
        0,
        Block::OakSlab,
    )
    .with_slot(1, Block::SpruceSlab)
    .expect("an empty upper slot");
    let design = Record {
        block: petramond_world::slab::representative_block(state),
        state: petramond_world::block::CellCodec::to_cell(&state),
        data: BTreeMap::new(),
    };
    let Plan::Unit { cost, .. } = plan(&design, at) else {
        panic!("a slab cell is a unit");
    };
    assert_eq!(
        cost.iter().map(|s| s.count).sum::<u8>(),
        2,
        "two parts, two items"
    );
    match w.construction_status(at, &design) {
        CellStatus::Place { missing, .. } => assert_eq!(
            missing,
            vec![ItemStack::new(ItemType::from_block(Block::SpruceSlab), 1)]
        ),
        other => panic!("expected the missing part, got {other:?}"),
    }
}

#[test]
fn explicit_air_clears_even_replaceable_plants_and_unknown_terrain_waits() {
    let mut w = world();
    let at = IVec3::new(2, 64, 2);
    w.set_block_world(at.x, at.y, at.z, Block::ShortGrass);
    let air = Record {
        block: Block::Air,
        state: Default::default(),
        data: BTreeMap::new(),
    };
    assert!(matches!(
        w.construction_status(at, &air),
        CellStatus::Clear {
            block: Block::ShortGrass,
            ..
        }
    ));
    let stone = Record {
        block: Block::Stone,
        state: Default::default(),
        data: BTreeMap::new(),
    };
    assert!(
        matches!(w.construction_status(at, &stone), CellStatus::Place { .. }),
        "a placement replaces a plant in place"
    );
    assert_eq!(
        w.construction_status(IVec3::new(40, 64, 40), &stone),
        CellStatus::Unloaded,
        "an unloaded cell is neither empty nor built"
    );
}

#[test]
fn a_portable_record_keeps_only_carried_data() {
    let kv = BTreeMap::from([
        ("petramond:tint".to_owned(), vec![1, 2, 3]),
        ("some_pack:machine_state".to_owned(), vec![9]),
    ]);
    let record = Record::portable(Block::Stone, Default::default(), &kv);
    let carried: Vec<_> = Block::Stone.carry().iter().map(|k| k.to_string()).collect();
    assert!(record.data.keys().all(|k| carried.contains(k)));
    assert!(!record.data.contains_key("some_pack:machine_state"));
}
