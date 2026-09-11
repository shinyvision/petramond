use super::*;
use std::collections::BTreeMap;

#[test]
fn water_domain_has_closed_sides_and_bottom_but_an_open_surface() {
    let wet = |[x, y, z]: [i32; 3]| {
        (-3..=3).contains(&x) && (-3..=3).contains(&z) && (-8..=0).contains(&y)
    };
    assert!(!needs_barrier([0, 0, 0], wet));
    for pos in [[-3, -4, 0], [3, -4, 0], [0, -4, -3], [0, -4, 3], [0, -8, 0]] {
        assert!(needs_barrier(pos, wet));
    }
}

#[test]
fn flooded_carving_matches_across_sections_and_keeps_water_contained() {
    let table = underground::test_table(&[r#"{
  "underground_biomes": [
    {
      "underground_biome": "fixture:lake",
      "y": [
        -32,
        24
      ],
      "aquifer": {
        "level": 8,
        "barrier": "petramond:stone"
      },
      "climate": {
        "humidity": [
          -1.5,
          1.5
        ],
        "depth": [
          -2,
          2
        ]
      }
    }
  ]
}"#]);
    let field = CaveField::with_table(42, table);
    let lat = field.build_lattice(-16, -48, -16, 15, 31, 15);
    let batch = BatchCarve::new(&field, &lat);
    let mut cells = BTreeMap::new();
    let mut wet = 0;
    let mut queries = Vec::new();
    let mut expected = Vec::new();
    for cx in -1..=0 {
        for cz in -1..=0 {
            for cy in -3..=1 {
                let mut section = Section::new(cx, cy, cz);
                section.edit_ids_bulk(|ids| ids.fill(Block::Stone.id()));
                field.carve_section(&mut section, &[80; 256]);
                for z in 0..16 {
                    for x in 0..16 {
                        let wx = cx * 16 + x;
                        let wz = cz * 16 + z;
                        let mut column = vec![Block::Stone.id(); 80];
                        batch.column::<false, _>(
                            &mut column,
                            |y| (y + 48) as usize,
                            wx,
                            wz,
                            -48,
                            31,
                            80,
                            &mut super::super::carve::Scratch::default(),
                        );
                        for y in 0..16 {
                            let wy = cy * 16 + y;
                            let block = section.block(x as usize, y as usize, z as usize);
                            assert_eq!(
                                block.id(),
                                column[(wy + 48) as usize],
                                "different batch at {wx},{wy},{wz}"
                            );
                            cells.insert([wx, wy, wz], block);
                            wet += usize::from(block == Block::Water);
                            if x == 0 && z == 0 {
                                queries.push(([wx, wy, wz], 80));
                                expected.push(block == Block::Air || block == Block::Water);
                            }
                        }
                    }
                }
            }
        }
    }
    assert!(wet > 0);
    for (&[x, y, z], &block) in &cells {
        if block == Block::Water {
            assert!(
                !needs_barrier([x, y, z], |p| cells
                    .get(&p)
                    .is_none_or(|b| *b != Block::Air)),
                "water escapes at {x},{y},{z}"
            );
        }
    }
    let mut actual = Vec::new();
    field.cave_carved_batch(&queries, &mut actual);
    assert_eq!(actual, expected);
}
