use super::*;
use crate::host::fake::rows::LEAVES;
use crate::testing::{Session, ASSET};

/// A wall along x at z = 0 on a floor, with a two-cell object at x = 2.
fn wall_around(lintel: bool) -> (Design, usize) {
    let mut design = Design::new([0; 32], [0, 0, 0], 0);
    let block = |footprint| Plan::Build {
        footprint,
        attachment: false,
        late: false,
        fragile: false,
        glazing: false,
    };
    design.plans = vec![block(vec![[0, 0, 0]]), block(vec![[0, 0, 0], [0, 1, 0]])];
    for x in 0..5 {
        for y in -1..3 {
            let door = x == 2 && (0..2).contains(&y);
            if door || (x == 2 && y == 2 && !lintel) {
                continue;
            }
            design.units.push(Unit {
                pos: [x, y, 0],
                record: 0,
            });
        }
    }
    design.units.push(Unit {
        pos: [2, 0, 0],
        record: 1,
    });
    for unit in design.units.clone() {
        for cell in design.cells(unit) {
            design.filled.insert(cell);
        }
    }
    design.finish();
    let door = design.unit_at([2, 0, 0]).unwrap();
    (design, door)
}

/// A closed 5x4x5 hut on a floor, with a one-block hedge a cell outside
/// its east wall and an eave jutting out over the hedge.
#[test]
fn a_house_keeps_its_rooms_and_the_space_over_its_hedge_empty() {
    let mut design = Design::new([0; 32], [0, 0, 0], 0);
    design.max = [6, 3, 4];
    for x in 0..5 {
        for y in 0..4 {
            for z in 0..5 {
                let shell = x == 0 || x == 4 || y == 0 || y == 3 || z == 0 || z == 4;
                if shell {
                    design.filled.insert([x, y, z]);
                }
            }
        }
    }
    design.filled.insert([5, 0, 2]);
    design.filled.insert([5, 3, 2]);
    design.governed = design.filled.clone();
    let rooms: HashSet<[i32; 3]> = design.rooms().into_iter().collect();
    assert!(rooms.contains(&[2, 1, 2]), "the room inside the walls");
    assert!(rooms.contains(&[5, 1, 2]), "between the hedge and the eave");
    assert!(
        !rooms.contains(&[5, 1, 1]),
        "beside the hedge, under nothing"
    );
    assert!(
        !rooms.contains(&[6, 1, 2]),
        "open ground the design builds nothing on"
    );
    assert!(rooms.iter().all(|c| !design.filled.contains(c)));
}

#[test]
fn a_tall_object_framed_by_wall_is_a_way_in() {
    let (design, door) = wall_around(true);
    assert!(design.passage(door));
    assert_eq!(
        (0..design.units.len())
            .filter(|i| design.passage(*i))
            .count(),
        1
    );
    let (design, door) = wall_around(false);
    assert!(!design.passage(door), "no lintel: an object on a wall top");
}

/// A two-layer strip: a pane, a stone and leaves on the ground, a stone
/// and a torch over the first two, and nothing over the leaves.
const STRIP: [([i32; 3], &str); 5] = [
    ([0, 0, 0], "petramond:glass_pane"),
    ([1, 0, 0], "petramond:stone"),
    ([2, 0, 0], "petramond:oak_leaves"),
    ([0, 1, 0], "petramond:stone"),
    ([1, 1, 0], "petramond:torch"),
];

/// The strip stored `per` cells to a section.
fn strip(per: usize) -> Session {
    let session = Session::flat(8);
    session
        .world
        .schematic(ASSET, "Strip", [3, 2, 1], &STRIP, per);
    session
}

fn compiled(turns: u8) -> Design {
    let _session = strip(64);
    let mut design = Design::new(ASSET, [10, 0, 5], turns);
    assert!(matches!(design.compile(64), Progress::Ready));
    design
}

#[test]
fn a_schematic_compiles_the_sections_each_call_is_given() {
    let _session = strip(2);
    let mut design = Design::new(ASSET, [10, 0, 5], 0);
    assert!(matches!(design.compile(1), Progress::Compiling));
    assert!(matches!(design.compile(1), Progress::Compiling));
    assert_eq!((design.compiled(), design.complete), (2, false));
    assert!(matches!(design.compile(1), Progress::Ready));
    assert!(design.complete);
    assert!(matches!(design.compile(1), Progress::Ready), "and stays so");
    assert_eq!(design.title, "Strip");
    assert_eq!((design.min, design.max), ([10, 0, 5], [12, 1, 5]));
}

/// Layer by layer; within a layer clearance, then whole blocks, then what
/// hangs on them, then what only lives beside them.
#[test]
fn units_go_bottom_up_whole_blocks_before_what_hangs_on_them() {
    let design = compiled(0);
    let order: Vec<[i32; 3]> = design.units.iter().map(|u| u.pos).collect();
    assert_eq!(
        order,
        vec![
            [11, 0, 5], // stone
            [10, 0, 5], // pane
            [12, 0, 5], // leaves
            [12, 1, 5], // the empty cell over the leaves
            [10, 1, 5], // stone
            [11, 1, 5], // torch
        ]
    );
    let at = |pos| design.units[design.unit_at(pos).expect("a unit there")];
    assert!(matches!(design.plan(at([12, 1, 5])), Plan::Air));
    assert!(design.glazing(design.unit_at([10, 0, 5]).unwrap()));
    assert!(!design.glazing(design.unit_at([11, 0, 5]).unwrap()));
    assert!(design.late(at([12, 0, 5])));
    assert!(design.fragile(at([11, 1, 5])));
    let cost = design.cost(at([11, 0, 5]));
    assert_eq!((cost.len(), cost[0].item.as_str()), (1, "petramond:stone"));
    assert!(design.cost(at([12, 1, 5])).is_empty());
    assert!(
        !design.governed.contains(&[12, 1, 5]),
        "a room is somewhere to stand"
    );
    assert!((0..design.units.len()).all(|i| !design.passage(i)));
}

#[test]
fn a_turned_design_stays_in_its_turned_box() {
    let design = compiled(1);
    assert_eq!((design.min, design.max), ([10, 0, 5], [10, 1, 7]));
    let inside = |c: &&[i32; 3]| (0..3).all(|i| (design.min[i]..=design.max[i]).contains(&c[i]));
    assert_eq!(design.governed.iter().filter(inside).count(), 5);
    assert_eq!(design.governed.len(), 5);
}

#[test]
fn a_design_that_cannot_be_read_fails_with_the_reason() {
    let session = strip(64);
    let mut missing = Design::new([1; 32], [0; 3], 0);
    assert!(matches!(
        missing.compile(8),
        Progress::Failed(reason) if reason == "The schematic is missing"
    ));
    session.world.schematic_lookup(
        ASSET,
        SchematicLookup::Failed {
            reason: "Corrupt".into(),
        },
    );
    let mut failed = Design::new(ASSET, [0; 3], 0);
    assert!(matches!(failed.compile(8), Progress::Failed(reason) if reason == "Corrupt"));
    session
        .world
        .schematic_lookup(ASSET, SchematicLookup::Loading);
    let mut loading = Design::new(ASSET, [0; 3], 0);
    assert!(matches!(loading.compile(8), Progress::Compiling));
    assert_eq!(loading.compiled(), 0);
}

#[test]
fn a_block_items_cannot_build_stays_in_the_design_as_unsupported() {
    let session = Session::flat(4);
    session.world.schematic(
        ASSET,
        "Pond",
        [2, 1, 1],
        &[
            ([0, 0, 0], "petramond:stone"),
            ([1, 0, 0], "petramond:water"),
        ],
        64,
    );
    let mut design = Design::new(ASSET, [0, 0, 0], 0);
    assert!(matches!(design.compile(8), Progress::Ready));
    let water = design.unit_at([1, 0, 0]).expect("the water is a unit");
    assert!(matches!(
        design.plan(design.units[water]),
        Plan::Unsupported(reason) if reason == "Fluids cannot be built"
    ));
    assert_eq!(design.overgrowth(), vec![LEAVES]);
}
