use super::{at_work, working};
use crate::fx::{HashMap, HashSet};
use crate::host::fake::rows::{BEDROCK, DIRT, DOOR, STONE};
use crate::host::fake::stack;
use crate::host::prelude::*;
use crate::testing::{CHEST_AT, HOME, TABLE_AT};
use crate::worker::ground::{fall_cost, ground, search, Cell, Move, Way};
use crate::worker::tuning::price::{DESIGN_MOVES, DOOR_MOVES, HURT_MOVES, LEVEL_MOVES, SAFE_FALL};

const OPEN: Cell = Cell {
    open: true,
    dig: None,
    floor: false,
    door: false,
};
/// What no tool breaks.
const WALL: Cell = Cell {
    open: false,
    dig: None,
    floor: true,
    door: false,
};
const DOOR_HALF: Cell = Cell {
    open: false,
    dig: None,
    floor: true,
    door: true,
};

fn earth(moves: u32) -> Cell {
    Cell {
        open: false,
        dig: Some(moves),
        floor: true,
        door: false,
    }
}

/// A box of walls from `lo` to `hi` with the listed cells set.
fn grid(lo: [i32; 3], hi: [i32; 3], cells: &[([i32; 3], Cell)]) -> HashMap<[i32; 3], Cell> {
    let mut grid = HashMap::default();
    for x in lo[0]..=hi[0] {
        for y in lo[1]..=hi[1] {
            for z in lo[2]..=hi[2] {
                grid.insert([x, y, z], WALL);
            }
        }
    }
    for (cell, kind) in cells {
        grid.insert(*cell, *kind);
    }
    grid
}

/// A corridor along x from 0 to 4 at z = 0, two cells high, floored.
fn corridor(extra: &[([i32; 3], Cell)]) -> HashMap<[i32; 3], Cell> {
    let mut cells: Vec<([i32; 3], Cell)> = Vec::new();
    for x in 0..=4 {
        cells.push(([x, 0, 0], OPEN));
        cells.push(([x, 1, 0], OPEN));
    }
    cells.extend_from_slice(extra);
    grid([-1, -1, -1], [5, 2, 1], &cells)
}

fn from(cell: [i32; 3]) -> HashMap<[i32; 3], u32> {
    [(cell, 0)].into_iter().collect()
}

fn to(cell: [i32; 3]) -> HashSet<[i32; 3]> {
    [cell].into_iter().collect()
}

fn way(way: Option<Way>) -> ([i32; 3], String, Vec<[i32; 3]>, Option<[i32; 3]>, u32) {
    let way = way.expect("a way out");
    (way.from, format!("{:?}", way.step), way.digs, way.door, way.cost)
}

#[test]
fn the_way_along_open_ground_is_walked() {
    let grid = corridor(&[]);
    let found = search(&grid, &from([0, 0, 0]), &to([4, 0, 0]), 20.0, &[], false, true);
    assert_eq!(
        way(found),
        ([0, 0, 0], format!("{:?}", Move::Walk([1, 0, 0])), Vec::new(), None, 4)
    );
    let found = search(&grid, &from([4, 0, 0]), &to([4, 0, 0]), 20.0, &[], false, true);
    assert_eq!(way(found).0, [4, 0, 0], "already there: walk where it stands");
}

#[test]
fn a_door_in_the_way_is_opened_and_passed() {
    let grid = corridor(&[([2, 0, 0], DOOR_HALF), ([2, 1, 0], DOOR_HALF)]);
    let found = search(&grid, &from([1, 0, 0]), &to([4, 0, 0]), 20.0, &[], false, true);
    // Into the doorway (a door's worth), out of it (the panel may be in the
    // way: another), and on.
    let cost = 1 + DOOR_MOVES + 1 + DOOR_MOVES + 1;
    assert_eq!(
        way(found),
        (
            [1, 0, 0],
            format!("{:?}", Move::Walk([2, 0, 0])),
            Vec::new(),
            Some([2, 0, 0]),
            cost
        )
    );
}

#[test]
fn earth_in_the_way_is_dug_through_by_what_it_costs() {
    let grid = corridor(&[([2, 0, 0], earth(2)), ([2, 1, 0], earth(2))]);
    let found = search(&grid, &from([1, 0, 0]), &to([4, 0, 0]), 20.0, &[], false, true);
    assert_eq!(
        way(found),
        (
            [1, 0, 0],
            format!("{:?}", Move::Walk([2, 0, 0])),
            vec![[2, 1, 0], [2, 0, 0]],
            None,
            1 + 4 + 1 + 1
        )
    );
    let sealed = corridor(&[([2, 0, 0], WALL), ([2, 1, 0], WALL)]);
    assert!(search(&sealed, &from([1, 0, 0]), &to([4, 0, 0]), 20.0, &[], true, true).is_none());
    let failed = [([0, 0, 0], [1, 0, 0])];
    assert!(
        search(&corridor(&[]), &from([0, 0, 0]), &to([4, 0, 0]), 20.0, &failed, false, true)
            .is_none(),
        "a way walked and failed is not taken again"
    );
}

/// A ledge five blocks over the only ground that walks home.
fn ledge() -> HashMap<[i32; 3], Cell> {
    let mut cells = vec![([0, 5, 0], OPEN), ([0, 6, 0], OPEN)];
    cells.extend((0..=6).map(|y| ([1, y, 0], OPEN)));
    grid([-1, -1, -1], [2, 7, 1], &cells)
}

#[test]
fn a_drop_is_taken_only_getting_out_and_never_a_deadly_one() {
    let grid = ledge();
    let found = search(&grid, &from([0, 5, 0]), &to([1, 0, 0]), 20.0, &[], false, true);
    // Five blocks down, two past the safe fall: two points of damage.
    let cost = 1 + 5 + 2 * HURT_MOVES;
    assert_eq!(
        way(found),
        ([0, 5, 0], format!("{:?}", Move::Drop([1, 0, 0])), Vec::new(), None, cost)
    );
    assert!(
        search(&grid, &from([0, 5, 0]), &to([1, 0, 0]), 20.0, &[], false, false).is_none(),
        "digging a way in never drops where it cannot climb back"
    );
    assert!(
        search(&grid, &from([0, 5, 0]), &to([1, 0, 0]), 2.0, &[], false, true).is_none(),
        "a fall that kills is no way"
    );
}

#[test]
fn a_scaffold_to_rise_on_is_the_last_way_out() {
    // A shaft with ground beside its top.
    let mut cells: Vec<([i32; 3], Cell)> = (0..=4).map(|y| ([0, y, 0], OPEN)).collect();
    cells.extend((2..=4).map(|y| ([1, y, 0], OPEN)));
    let grid = grid([-1, -1, -1], [2, 5, 1], &cells);
    assert!(search(&grid, &from([0, 0, 0]), &to([1, 2, 0]), 20.0, &[], false, true).is_none());
    let found = search(&grid, &from([0, 0, 0]), &to([1, 2, 0]), 20.0, &[], true, true);
    assert_eq!(
        way(found),
        (
            [0, 0, 0],
            format!("{:?}", Move::Rise),
            Vec::new(),
            None,
            LEVEL_MOVES as u32 + 1
        )
    );
}

#[test]
fn the_ground_is_read_as_what_it_costs_to_get_through() {
    let (mut session, id, golem) = working(HOME);
    session.world.set([3, 0, -5], DIRT);
    session.world.set([3, 0, -6], DOOR);
    session.world.set([2, 0, -6], BEDROCK);
    let lo = [-2, -1, -7];
    let size = [6, 3, 9];
    let body = session.body(golem);
    let project = session.builder.projects.get(id).unwrap().clone();
    let read = |session: &mut crate::testing::Session, body: &crate::worker::Body, spare: bool| {
        at_work(session, id, |ctx, _, job| {
            ground(ctx, job, &project, body, lo, size, spare).expect("all of it loaded")
        })
    };
    let grid = read(&mut session, &body, true);
    assert_eq!(grid.len(), 6 * 3 * 9);
    // Stone by hand: fruitless without a pickaxe, four times slower.
    assert_eq!(grid[&[0, -1, 0]].dig, Some(60));
    assert!(grid[&[0, -1, 0]].floor && !grid[&[0, -1, 0]].open);
    assert_eq!(grid[&[3, 0, -5]].dig, Some(5), "earth by hand");
    assert!(grid[&[3, 0, -6]].door && grid[&[3, 0, -6]].dig.is_none());
    assert!(grid[&[2, 0, -6]].floor && grid[&[2, 0, -6]].dig.is_none(), "unbreakable");
    assert!(grid[&[0, 0, -2]].open && !grid[&[0, 0, -2]].floor);
    assert_eq!(grid[&CHEST_AT].dig, None, "a supply chest is never dug");
    assert_eq!(grid[&TABLE_AT].dig, None, "nor the table");

    session
        .world
        .give(ContainerAddress::Mob(golem), "petramond:stone_pickaxe", 1);
    let body = session.body(golem);
    let grid = read(&mut session, &body, true);
    assert_eq!(grid[&[0, -1, 0]].dig, Some(4), "a stone pickaxe, four times faster");
    assert!(body.slots.contains(&Some(stack("petramond:stone_pickaxe", 1))));

    session.world.unload([0, 1, 0], [0, 1, 0]);
    assert!(at_work(&mut session, id, |ctx, _, job| {
        ground(ctx, job, &project, &body, lo, size, true).is_none()
    }));
}

#[test]
fn the_designs_own_blocks_are_spared_digging_in_and_dear_getting_out() {
    let (mut session, id) = crate::testing::Session::row();
    session.world.set([0, 0, 0], STONE);
    session.world.set([2, 0, 0], DIRT);
    session.job(id);
    let golem = session.golem(id, HOME, HOME);
    let body = session.body(golem);
    let project = session.builder.projects.get(id).unwrap().clone();
    let (spared, dug) = at_work(&mut session, id, |ctx, _, job| {
        let lo = [-1, -1, -1];
        let size = [5, 3, 3];
        (
            ground(ctx, job, &project, &body, lo, size, true).unwrap(),
            ground(ctx, job, &project, &body, lo, size, false).unwrap(),
        )
    });
    assert_eq!(spared[&[0, 0, 0]].dig, None, "built: left alone");
    assert_eq!(dug[&[0, 0, 0]].dig, Some(60 + DESIGN_MOVES));
    assert_eq!(spared[&[2, 0, 0]].dig, Some(5), "earth in a design cell is work to clear");
    assert!(spared[&[1, 0, 0]].open);
}

#[test]
fn a_fall_costs_its_levels_and_what_it_hurts() {
    let safe = SAFE_FALL as u32;
    assert_eq!(fall_cost(1, 20.0), Some(1));
    assert_eq!(fall_cost(SAFE_FALL, 20.0), Some(safe), "a safe fall does not hurt");
    assert_eq!(fall_cost(SAFE_FALL + 2, 20.0), Some(safe + 2 + 2 * HURT_MOVES));
    assert_eq!(fall_cost(SAFE_FALL + 2, 2.0), None, "a fall that kills is no way");
}
