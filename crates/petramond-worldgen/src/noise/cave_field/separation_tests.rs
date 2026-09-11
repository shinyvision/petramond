use super::*;
use crate::data::excavations;

const HABITAT: &str = r#"{
  "underground_biomes": [
    {
      "underground_biome": "test:habitat",
      "y": [
        -48,
        32
      ],
      "lining": {
        "block": "petramond:moss_block",
        "shell": 4,
        "faces": {
          "floor_depth": 3
        }
      },
      "climate": {
        "humidity": [
          -1,
          1
        ],
        "depth": [
          -2,
          2
        ]
      }
    }
  ]
}"#;
const ROOMS: &str = r#"{"excavations":[
 {"excavation":"test:rooms","placement":{"spacing":64,"y":[-48,32]},
  "chamber":{"radius":[10,12],"feather":8,"tunnel":1.5}}
]}"#;

#[test]
fn habitat_assignment_and_lining_do_not_change_cave_geometry() {
    let bare = underground::test_table(&[]);
    let painted = underground::test_table(&[HABITAT]);
    let plain = CaveField::with_table(17, bare);
    let habitat = CaveField::with_table(17, painted);
    let queries: Vec<_> = (-64..64)
        .step_by(3)
        .flat_map(|x| {
            (-64..64)
                .step_by(3)
                .flat_map(move |z| (-48..48).step_by(3).map(move |y| ([x, y, z], 75)))
        })
        .collect();
    let mut a = Vec::new();
    let mut b = Vec::new();
    plain.cave_carved_batch(&queries, &mut a);
    habitat.cave_carved_batch(&queries, &mut b);
    assert!(a.iter().any(|&v| v) && a.iter().any(|&v| !v));
    assert_eq!(a, b);
    let p = [0, -20, 0];
    assert_ne!(
        plain.underground_biome_at(p[0], p[1], p[2]),
        habitat.underground_biome_at(p[0], p[1], p[2])
    );
}

#[test]
fn unconditioned_excavation_changes_only_its_bounded_neighborhood() {
    let bare = underground::test_table(&[]);
    let painted = underground::test_table(&[HABITAT]);
    let rooms = excavations::test_table(&[ROOMS], bare);
    let plain = CaveField::with_table(17, bare);
    let carved = CaveField::with_tables(17, bare, rooms);
    let relined = CaveField::with_tables(17, painted, rooms);
    let positions = carved
        .chamber_field([-64, -48, -64], [64, 32, 64])
        .centers();
    assert!(!positions.is_empty());
    let [cx, cy, cz] = positions[0];
    let mut added = 0;
    for x in (cx - 16..=cx + 16).step_by(2) {
        for z in (cz - 16..=cz + 16).step_by(2) {
            for y in cy - 8..=cy + 8 {
                let a = plain.cave_carved(x, y, z, 75);
                let b = carved.cave_carved(x, y, z, 75);
                assert!(!a || b, "excavation must not close an existing passage");
                assert_eq!(b, relined.cave_carved(x, y, z, 75));
                added += usize::from(!a && b);
            }
        }
    }
    assert!(added > 0);
    for y in [33, 40, 50] {
        for x in -16..16 {
            assert_eq!(
                plain.cave_carved(x, y, 0, 75),
                carved.cave_carved(x, y, 0, 75)
            );
        }
    }
}

#[test]
fn natural_caves_close_before_the_protected_bottom_section() {
    let field = CaveField::with_table(42, underground::test_table(&[]));
    let positions: Vec<_> = (-128..128)
        .step_by(3)
        .flat_map(|x| {
            (-128..128)
                .step_by(3)
                .map(move |z| ([x, CAVE_MIN_Y, z], 80))
        })
        .collect();
    let mut open = Vec::new();
    field.cave_carved_batch(&positions, &mut open);
    assert!(
        open.iter().all(|&v| !v),
        "a hard stop exposes the protected section as a flat floor"
    );
    let above: Vec<_> = positions
        .iter()
        .map(|(p, y)| ([p[0], CAVE_MIN_Y + CAVE_FLOOR_FADE as i32, p[2]], *y))
        .collect();
    field.cave_carved_batch(&above, &mut open);
    assert!(
        open.iter().any(|&v| v),
        "the floor taper must leave the cave network above it"
    );
}
