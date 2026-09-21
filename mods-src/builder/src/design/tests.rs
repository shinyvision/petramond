use super::*;

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
