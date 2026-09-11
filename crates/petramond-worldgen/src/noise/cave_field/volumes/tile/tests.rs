use super::*;
use crate::data::{excavations, underground};

static ROW: std::sync::OnceLock<&'static Excavation> = std::sync::OnceLock::new();

fn shape() -> &'static FieldShape {
    let biomes = underground::test_table(&[]);
    let table = excavations::test_table(
        &[r#"{"excavations":[{
        "excavation":"test:field", "placement":{"spacing":64,"y":[0,0]},
        "field":{"radius":[24,24],"height":[32,32],"bound_radius":32,
            "y":[-60,80],"grid_step":16,"separation":32,
            "cut":["gt","y",-3],"material":0,"palette":["petramond:air"],
            "boundaries":[{"from":["petramond:air"],"offsets":[[1,0,0],[-1,0,0]],
                "block":"petramond:stone","replace":"fluid"}],
            "courses":[{"from":["petramond:air"],"offset":[0,-1,0],"step":[0,1,0],
                "length":[4,4],"palette":["petramond:dirt"]}]
        }
    }]}"#],
        biomes,
    );
    let _ = ROW.set(table.rows[0]);
    ROW.get().expect("row").field().unwrap()
}

fn plan(shape: &'static FieldShape, center: [i32; 3], biome: u8) -> Plan<'static> {
    let row = ROW.get().expect("shape() ran");
    Plan {
        row,
        site: Site {
            center,
            radius: 24,
            height: 32,
            bounds: super::super::Bounds {
                min: [center[0] - 32, -60, center[2] - 32],
                max: [center[0] + 32, 80, center[2] + 32],
            },
        },
        shape,
        biome,
        salt: 19,
    }
}

/// Seals and course cells are remembered per tile with what the walk needs
/// to lay them, and tiles sharing a halo remember the same thing.
#[test]
fn seals_and_course_cells_replay_across_negative_section_edges() {
    let shape = shape();
    let plans = [plan(shape, [-1, 0, -1], 7)];
    let mut observed = std::collections::BTreeMap::new();
    for origin in [[-16, -16, -16], [-16, 0, -16], [0, -16, -16], [0, 0, -16]] {
        let mut draft = Draft::new(origin, padding(shape));
        for i in 0..draft.cells.len() {
            let [x, y, _] = draft.position(i);
            if x == -1 && y >= -2 {
                draft.cells[i] = DraftCell {
                    cell: Cell::Fill(Fill {
                        block: Block::Air.id(),
                        biome: 7,
                        replace: MaterialFilter::Any,
                    }),
                    owner: 0,
                };
            }
        }
        let surfaces = vec![70; draft.side[0] * draft.side[2]];
        let (mut seals, mut courses) = (Vec::new(), Vec::new());
        boundaries(&mut draft, &plans, &mut seals);
        course_cells(&draft, &plans, &surfaces, 42, &mut courses);
        let tile = finish(draft, origin, seals, courses);
        let tile_y = origin[1].div_euclid(16);
        for seal in tile.seals.iter() {
            let key = seal.key;
            let pos = [
                origin[0] + i32::from(key / 16 % 16),
                origin[1] + i32::from(key % 16),
                origin[2] + i32::from(key / 256),
            ];
            let fact = ("seal", seal.block, i32::from(seal.replace as u8));
            if let Some(old) = observed.insert(pos, fact) {
                assert_eq!(old, fact, "{pos:?}");
            }
        }
        for course in tile.courses.iter() {
            let pos = [
                origin[0] + i32::from(course.key / 16 % 16),
                course.y(tile_y),
                origin[2] + i32::from(course.key / 256),
            ];
            let fact = ("course", course.block, i32::from(course.anchor_dy));
            if let Some(old) = observed.insert(pos, fact) {
                assert_eq!(old, fact, "{pos:?}");
            }
        }
    }
    // Both sides of the air column are sealed against water, up the column.
    for y in [-2, 0, 5] {
        for x in [-2, 0] {
            assert_eq!(
                observed.get(&[x, y, -1]),
                Some(&(
                    "seal",
                    Block::Stone.id(),
                    MaterialFilter::Fluid as u8 as i32
                )),
                "{x} {y}"
            );
        }
    }
    // The course rises four cells from the anchor under the column's foot,
    // each remembering where that anchor is.
    for (y, dy) in [(-3, 0), (-2, -1), (-1, -2), (0, -3)] {
        assert_eq!(
            observed.get(&[-1, y, -1]),
            Some(&("course", Block::Dirt.id(), dy))
        );
    }
    assert_eq!(
        observed.get(&[-1, 1, -1]),
        None,
        "a course must not recursively grow itself"
    );
}

/// The corner-sampled carve fills exactly the heights a linear cut selects
/// and reads no terrain to do it.
#[test]
fn corner_sampled_carve_fills_the_cut_interior() {
    let shape = shape();
    let plans = [plan(shape, [0, 0, 0], 1)];
    let mut draft = Draft::new([0, -16, 0], [1; 3]);
    let surfaces = vec![70; draft.side[0] * draft.side[2]];
    carve(&mut draft, &plans, &surfaces, 0);
    for i in 0..draft.cells.len() {
        let [_, y, _] = draft.position(i);
        let filled = matches!(draft.cells[i].cell, Cell::Fill(_));
        assert_eq!(filled, y > -3, "height {y}");
    }
}

/// A cut recipe's margin is the difference of its comparison's sides, and
/// conjunctions and disjunctions become their minimum and maximum.
#[test]
fn cut_recipes_become_signed_margins() {
    let biomes = underground::test_table(&[]);
    let table = excavations::test_table(
        &[r#"{"excavations":[{
        "excavation":"test:margin", "placement":{"spacing":64,"y":[0,0]},
        "field":{"radius":[24,24],"height":[32,32],"bound_radius":32,
            "y":[-60,80],"grid_step":16,"separation":32,
            "bindings":[["near",["lt",["sub","x","center_x"],10]]],
            "cut":["and","near",["or",["gt","y",0],["le","y",-10]]],
            "material":0,"palette":["petramond:air"]}
    }]}"#],
        biomes,
    );
    let shape = table.rows[0].field().unwrap();
    let at = |x: f64, y: f64| {
        shape
            .margin
            .sample::<1>(Inputs([x, y, 0.0, 4.0, 0.0, 0.0, 24.0, 32.0, 70.0, 63.0]))[0]
    };
    assert_eq!(at(7.0, 5.0), (10.0 - 3.0_f64).min(5.0_f64.max(-14.5)));
    assert_eq!(at(20.0, 5.0), -6.0);
    assert_eq!(at(7.0, -12.0), 2.5);
    assert!(at(7.0, -4.0) < 0.0);
}

/// A projection anchors on the field's floor wherever the column has one,
/// paints the target column over the rule's admitted range whatever tile
/// asks, and reads the terrain it replaces as rock under the surface and
/// water above it.
#[test]
fn projected_columns_find_anchors_outside_the_section_and_preserve_materials() {
    let biomes = underground::test_table(&[]);
    let table = excavations::test_table(
        &[r#"{"excavations":[{
        "excavation":"test:projection", "placement":{"spacing":64,"y":[0,0]},
        "field":{"radius":[24,24],"height":[32,32],"bound_radius":32,
            "y":[-60,80],"grid_step":16,"separation":32,
            "cut":["and",["lt",["abs",["sub","x","center_x"]],6],["and",["le",-20,"y"],["le","y",-10]]],
            "material":0,"palette":["petramond:air"],
            "projections":[{"from":["petramond:air"],"offsets":[[1,0,0]],
                "when":["eq","y",-20],"range":[-25,3],"material":0,
                "palette":["petramond:dirt"],
                "preserve":[{"from":["petramond:water"],"when":["le","y",-2]}],
                "remap":[["petramond:water","petramond:stone"]]},
                {"from":["petramond:air"],"offsets":[[-1,0,0]],
                "when":["eq","y",-20],"range":[-40,-30],"material":0,
                "palette":["petramond:gravel"],
                "probe":{"y":-30,"avoid":["petramond:water","petramond:air"]}},
                {"from":["petramond:air"],"offsets":[[0,0,1]],
                "when":["eq","y",-20],"range":[-40,-30],"material":0,
                "palette":["petramond:sand"],
                "probe":{"y":-15,"avoid":["petramond:air"]}}]
        }
    }]}"#],
        biomes,
    );
    let field = CaveField::with_tables(123, biomes, table);
    let row = &table.rows[0];
    let shape = row.field().unwrap();
    // A site the placement grid really puts down: chunk anchors are shared
    // by every tile over a chunk, so they come from placement, not a tile.
    let site = (-2..2)
        .flat_map(|z| (-2..2).map(move |x| [x, z]))
        .find_map(|cell| site(&field, row, shape, cell))
        .expect("a placed site");
    let [cx, _, cz] = site.center;
    let plans = [Plan {
        site,
        row,
        shape,
        biome: 7,
        salt: 0,
    }];
    let guess = |y: i32| {
        if y <= -3 {
            Block::Stone.id()
        } else {
            Block::Water.id()
        }
    };
    let (target, beyond) = (cx + 1, cx + 7);
    let (ox, oz) = (target.div_euclid(16) * 16, cz.div_euclid(16) * 16);
    let mut observed = std::collections::BTreeMap::new();
    for oy in [0, -48, -32, -16] {
        for ox in [ox, ox - 16] {
            let mut draft = Draft::new([ox, oy, oz], padding(shape));
            let surfaces = vec![-3; draft.side[0] * draft.side[2]];
            carve(&mut draft, &plans, &surfaces, 123);
            projection::apply(&mut draft, &plans, &surfaces, &field);
            // Only a tile's own interior is published; its halo may miss
            // an anchor one column further out.
            let interior = |pos: [i32; 3]| {
                pos[0] >= ox && pos[0] < ox + 16 && pos[1] >= oy && pos[1] < oy + 16
            };
            for y in -41..=4 {
                for x in [target, beyond] {
                    let pos = [x, y, cz];
                    if draft.index(pos).is_none() || !interior(pos) {
                        continue;
                    }
                    let value = draft.read(pos, &mut |p| guess(p[1]));
                    if let Some(old) = observed.insert(pos, value) {
                        assert_eq!(old, value, "{pos:?}");
                    }
                }
                let side = [cx, y, cz + 1];
                if let Some(i) = draft.index(side).filter(|_| interior(side)) {
                    assert!(
                        !matches!(draft.cells[i].cell, Cell::Fill(Fill { block, .. }) if block == Block::Sand.id()),
                        "a probe into the field's own air refuses the column"
                    );
                }
            }
        }
    }
    for y in -25..=3 {
        let expected = match y {
            -2 => Block::Water,
            -1..=3 => Block::Stone,
            _ => Block::Dirt,
        };
        assert_eq!(observed[&[target, y, cz]], expected.id(), "projected y={y}");
        assert_eq!(
            observed[&[beyond, y, cz]],
            guess(y),
            "projection has finite horizontal reach"
        );
    }
    assert_eq!(observed[&[target, 4, cz]], guess(4));
    // A probe into rock under the field admits the column.
    for y in -40..=-30 {
        assert_eq!(
            observed[&[target, y, cz]],
            Block::Gravel.id(),
            "probed y={y}"
        );
    }
}
