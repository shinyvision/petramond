use super::*;
use crate::data::underground;

/// Batching is just scheduling, so batched queries must answer the same as the one-voxel
/// lattice, position for position. A drift here is invisible: a mod's structures shift by a cell
/// somewhere deep. So we pin batches directly against the reference, across habitat lining,
/// regional selection, and singleton batches.
#[test]
fn batched_positional_queries_match_the_one_voxel_reference() {
    for pack in [None, Some(LINING_PACK), Some(REGION_PACK)] {
        let table = match pack {
            None => underground::table(),
            Some(p) => underground::test_table(&[p]),
        };
        let layers: Vec<_> = pack.into_iter().collect();
        let field = CaveField::with_tables(
            0x5EED_BEEF,
            table,
            crate::data::excavations::test_table(&layers, table),
        );
        let mut positions = Vec::new();
        let mut st = 0x9E37_79B9_7F4A_7C15u64;
        for _ in 0..3000 {
            let mut next = |lo: i32, hi: i32| {
                st ^= st << 13;
                st ^= st >> 7;
                st ^= st << 17;
                lo + (st % ((hi - lo + 1) as u64)) as i32
            };
            positions.push([next(-300, 300), next(CAVE_MIN_Y, 90), next(-300, 300)]);
        }
        for y in CAVE_MIN_Y..CAVE_MIN_Y + 60 {
            positions.push([7, y, -13]);
        }
        for x in 0..24 {
            for z in 0..24 {
                positions.push([x, -20, z]);
            }
        }

        let mut biomes = Vec::new();
        field.underground_biome_at_batch(&positions, &mut biomes);
        let queries: Vec<([i32; 3], i32)> = positions.iter().map(|&p| (p, 70)).collect();
        let mut carved = Vec::new();
        field.cave_carved_batch(&queries, &mut carved);

        for (i, &[x, y, z]) in positions.iter().enumerate() {
            assert_eq!(
                biomes[i],
                field.underground_biome_at(x, y, z),
                "biome batch diverged at {x},{y},{z}"
            );
            assert_eq!(
                carved[i],
                field.cave_carved(x, y, z, 70),
                "carve batch diverged at {x},{y},{z}"
            );
        }
    }
}

const REGION_PACK: &str = r#"{
  "underground_biomes": [
    {
      "underground_biome": "test:region",
      "y": [
        -48,
        32
      ],
      "lining": {
        "block": "petramond:moss_block",
        "shell": 1.4,
        "blend": [
          0.02,
          8
        ]
      },
      "climate": {
        "humidity": [
          -1,
          1
        ],
        "depth": [
          -2,
          2
        ],
        "temperature": [
          0,
          0.4
        ]
      }
    }
  ],
  "excavations": [
    {
      "excavation": "test:region_rooms",
      "placement": {
        "spacing": 96,
        "one_in": 1,
        "y": [
          -48,
          32
        ],
        "underground_biome": "test:region"
      },
      "chamber": {
        "radius": [
          10,
          14
        ],
        "feather": 9,
        "strength": 1,
        "tunnel": 2
      }
    }
  ]
}"#;

const LINING_PACK: &str = r#"{
  "underground_biomes": [
    {
      "underground_biome": "mymod:veined",
      "lining": {
        "block": "petramond:marble",
        "shell": 2.0,
        "blend": [
          0.02,
          4
        ]
      },
      "climate": {
        "humidity": [
          0.17,
          1.0
        ],
        "depth": [
          -2,
          2
        ]
      }
    },
    {
      "underground_biome": "mymod:roomy",
      "lining": {
        "block": "petramond:moss_block",
        "shell": 2.0,
        "faces": {
          "ceiling": {
            "weight": 0.0
          }
        },
        "blend": [
          0.02,
          4
        ]
      },
      "climate": {
        "humidity": [
          -1.5,
          0.17
        ],
        "depth": [
          -2,
          2
        ]
      }
    }
  ]
}"#;

const BANDED_LINING_PACK: &str = r#"{
  "underground_biomes": [
    {
      "underground_biome": "deep:roomy",
      "y": [
        -64,
        -33
      ],
      "lining": {
        "block": "petramond:moss_block",
        "shell": 2.0,
        "blend": [
          0.02,
          4
        ]
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
    },
    {
      "underground_biome": "rare:roomy",
      "lining": {
        "block": "petramond:marble",
        "shell": 2.0,
        "blend": [
          0.02,
          4
        ]
      },
      "climate": {
        "humidity": [
          0.24,
          1.5
        ],
        "depth": [
          -2,
          2
        ]
      }
    }
  ]
}"#;

const CHAMBER_PACK: &str = r#"{
  "underground_biomes": [
    {
      "underground_biome": "wide:rock",
      "climate": {
        "humidity": [
          -1.5,
          0.24
        ],
        "depth": [
          -2,
          2
        ]
      }
    },
    {
      "underground_biome": "mymod:cathedral",
      "y": [
        -64,
        -16
      ],
      "lining": {
        "block": "petramond:moss_block",
        "shell": 1.4,
        "faces": {
          "floor_depth": 2,
          "floor": {
            "block": "petramond:moss_block"
          },
          "wall": {
            "weight": 0.7
          },
          "ceiling": {
            "weight": 0.2
          }
        },
        "blend": [
          0.02,
          4
        ]
      },
      "climate": {
        "humidity": [
          0.24,
          1.0
        ],
        "depth": [
          -2,
          2
        ]
      }
    }
  ],
  "excavations": [
    {
      "excavation": "mymod:cathedral_rooms",
      "placement": {
        "spacing": 64,
        "one_in": 1,
        "y": [
          -64,
          -16
        ],
        "underground_biome": "mymod:cathedral"
      },
      "chamber": {
        "radius": [
          10,
          14
        ],
        "flatten": 0.45,
        "sill": 0.75,
        "feather": 9,
        "strength": 1.0,
        "lobes": 3,
        "lobe_spread": 0.7,
        "lobe_scale": [
          0.55,
          0.8
        ],
        "tunnel": 2.0,
        "rim_noise": 1.5
      }
    }
  ]
}"#;

const BANDED_CHAMBER_PACK: &str = r#"{
  "underground_biomes": [
    {
      "underground_biome": "wide:rock",
      "climate": {
        "humidity": [
          -1.5,
          0.22
        ],
        "depth": [
          -2,
          2
        ]
      }
    },
    {
      "underground_biome": "deep:cathedral",
      "y": [
        -64,
        -24
      ],
      "lining": {
        "block": "petramond:moss_block"
      },
      "climate": {
        "humidity": [
          0.22,
          1.0
        ],
        "depth": [
          -2,
          2
        ]
      }
    }
  ],
  "excavations": [
    {
      "excavation": "deep:cathedral_rooms",
      "placement": {
        "spacing": 32,
        "one_in": 1,
        "y": [
          -64,
          -24
        ],
        "underground_biome": "deep:cathedral"
      },
      "chamber": {
        "radius": [
          5,
          6
        ],
        "flatten": 0.9,
        "sill": 0.4,
        "feather": 8,
        "strength": 1.6
      }
    }
  ]
}"#;

const LINING_ROOMS: &str = r#"{"excavations":[
        {"excavation":"test:lining_rooms","placement":{"spacing":32,"y":[-48,32]},
         "chamber":{"radius":[5,7],"feather":8,"tunnel":1}}
    ]}"#;

fn has_banded_rows(table: &underground::UndergroundBiomes) -> bool {
    table.name(1).is_some()
}

fn tables() -> [(
    &'static UndergroundBiomes,
    &'static crate::data::excavations::Excavations,
); 6] {
    [
        None,
        Some(LINING_PACK),
        Some(BANDED_LINING_PACK),
        Some(CHAMBER_PACK),
        Some(BANDED_CHAMBER_PACK),
        Some(REGION_PACK),
    ]
    .map(|pack| {
        let layers: Vec<_> = pack.into_iter().collect();
        let table = underground::test_table(&layers);
        (table, crate::data::excavations::test_table(&layers, table))
    })
}

fn sweep_boxes(field: &CaveField, span: i32) -> Vec<[i32; 3]> {
    let mut boxes = vec![
        [-8, -40, 24],
        [-20, -48, 12],
        [96, -60, -144],
        [-232, -24, 88],
        [312, 8, 296],
    ];
    if field.chamber_y_span.is_some() {
        let rooms = field.chamber_field([-768, -64, -768], [768, -8, 768]);
        for c in rooms.centers().into_iter().take(3) {
            boxes.push([c[0] - span / 2, c[1] - span / 2, c[2] - span / 2]);
            for (dx, dy, dz) in [(18, 0, 0), (-24, 0, 6), (0, 9, -20), (6, -11, 16)] {
                boxes.push([c[0] + dx - span / 2, c[1] + dy, c[2] + dz - span / 2]);
            }
        }
        assert!(
            boxes.len() > 5,
            "no chamber rolled anywhere near the sweep; the coverage is vacuous"
        );
    }
    boxes
}

#[test]
fn point_and_batch_carve_decisions_agree() {
    const SPAN: i32 = 19;
    for (table, excavations) in tables() {
        for seed in [0x51EEDu32, 0x1D001, 0x26AAC] {
            let field = CaveField::with_tables(seed, table, excavations);
            let (mut carved, mut chamber_boxes) = (0usize, 0usize);
            for [x0, y0, z0] in sweep_boxes(&field, SPAN) {
                let (x1, y1, z1) = (x0 + SPAN, y0 + SPAN, z0 + SPAN);
                let lat = field.build_lattice(x0, y0, z0, x1, y1, z1);
                chamber_boxes += lat.chamber_is_live() as usize;
                for surf_y in [y1 - 2, y1 + 80] {
                    for y in (y0..=y1).step_by(3) {
                        for z in (z0..=z1).step_by(3) {
                            for x in (x0..=x1).step_by(3) {
                                let batch = field.carved_lat(&lat, x, y, z, surf_y);
                                let point = field.cave_carved(x, y, z, surf_y);
                                assert_eq!(
                                    batch, point,
                                    "divergence at ({x},{y},{z}) surf {surf_y} seed {seed:#x}"
                                );
                                carved += batch as usize;
                            }
                        }
                    }
                }
            }
            // Not shape pins — proof the sweep exercised what it claims to.
            assert!(carved > 0, "test volume should contain some cave air");
            assert!(
                excavations.y_span.is_none() || chamber_boxes > 0,
                "no swept box carried a live chamber lane (seed {seed:#x})"
            );
        }
    }
}

/// The skip mask may only drop cells it can PROVE are all-solid. Nothing
/// else asserts this, and a mask that forgot to bound a pack's lining shell —
/// or by the chamber term now riding the same cavern threshold — deletes
/// caves while still producing plausible-looking output.
///
/// Adversarial on purpose: several seeds, and boxes aimed at the rims of
/// the rooms each seed actually rolls, since a chamber's bound is exact in
/// its core and only interesting where the term is partial.
///
/// A skipped cell must also not be a cell the LINING would have painted. An
/// orientation lining paints the rock UNDER carved air even when that rock
/// is outside the shell band, and that rock can sit in the lattice cell
/// below the air's — so the mask has to be dilated. Skipping it produces
/// moss that appears or not depending on where the 4-block boundary falls,
/// which is plausible output with nothing asserting against it.
#[test]
fn may_cut_mask_never_skips_a_carved_cell() {
    const SPAN: i32 = 23;
    for (table, excavations) in tables() {
        for seed in [0xA5C0u32, 0x312, 0x1D001, 0x2BEEF] {
            let field = CaveField::with_tables(seed, table, excavations);
            let (mut skipped, mut chamber_boxes) = (0usize, 0usize);
            for [x0, y0, z0] in sweep_boxes(&field, SPAN) {
                let (x1, y1, z1) = (x0 + SPAN, y0 + SPAN, z0 + SPAN);
                let lat = field.build_lattice(x0, y0, z0, x1, y1 + MAX_FLOOR_DEPTH as i32, z1);
                chamber_boxes += lat.chamber_is_live() as usize;
                let mask = lat.may_cut_mask(field.underground, field.lining_faces);
                let (mx, mz) = (lat.nx - 1, lat.nz - 1);
                for surf_y in [y1 - 2, y1 + 80] {
                    for y in y0..=y1 {
                        for z in z0..=z1 {
                            for x in x0..=x1 {
                                let cx = (x.div_euclid(LATTICE_STEP) - lat.lx0) as usize;
                                let cy = (y.div_euclid(LATTICE_STEP) - lat.ly0) as usize;
                                let cz = (z.div_euclid(LATTICE_STEP) - lat.lz0) as usize;
                                if mask[(cy * mz + cz) * mx + cx] {
                                    continue;
                                }
                                skipped += 1;
                                assert_eq!(
                                    field.cut_lat(&lat, x, y, z, surf_y),
                                    CaveCut::Solid,
                                    "mask skipped ({x},{y},{z}) surf {surf_y} seed {seed:#x} \
                                         but the carver acts"
                                );
                                if field.lining_faces {
                                    for d in 1..=table.lining_floor_depth_max {
                                        assert!(
                                            !field.cut_lat(&lat, x, y + d, z, surf_y).is_open(),
                                            "mask skipped ({x},{y},{z}) surf {surf_y} seed \
                                                 {seed:#x} but a cave FLOOR course {d} above \
                                                 reaches it"
                                        );
                                    }
                                }
                            }
                        }
                    }
                }
            }
            assert!(skipped > 0, "the mask must actually skip something here");
            assert!(
                excavations.y_span.is_none() || chamber_boxes > 0,
                "no swept box carried a live chamber lane (seed {seed:#x})"
            );
        }
    }
}

#[test]
fn the_chamber_lane_is_only_skipped_where_no_room_can_reach() {
    for (table, excavations) in tables() {
        let Some((lo, hi)) = excavations.y_span else {
            continue;
        };
        let field = CaveField::with_tables(0x1D001, table, excavations);
        for y in [lo - 1, lo - 64, hi + 1, hi + 64] {
            let rooms = field.chamber_field([-512, y, -512], [512, y, 512]);
            for c in rooms.centers() {
                let far = field.chamber_field([c[0], y, c[2]], [c[0], y, c[2]]);
                assert_eq!(
                    far.at(c[0], y, c[2], field.knead_sample(c[0], y, c[2])),
                    (0.0, 0.0),
                    "a room contributes at y={y}, outside the declared span {lo}..={hi}"
                );
            }
        }
        let live = sweep_boxes(&field, 19).into_iter().any(|[x, y, z]| {
            field
                .build_lattice(x, y, z, x + 19, y + 19, z + 19)
                .chamber_is_live()
        });
        assert!(live, "the declared span never yields a live lane");
    }
}

#[test]
fn abi_query_agrees_with_the_carver() {
    for (table, excavations) in tables() {
        let field = CaveField::with_tables(0xB10, table, excavations);
        let mut seen = std::collections::BTreeSet::new();
        for (x0, y0, z0) in [
            (-6, -50, 30),
            (-280, -20, 150),
            (190, 8, -240),
            (-140, 40, 320),
            (260, -44, -70),
        ]
        .into_iter()
        .chain(territory_boxes(&field))
        {
            let (x1, y1, z1) = (x0 + 15, y0 + 15, z0 + 15);
            let lat = field.build_lattice(x0, y0, z0, x1, y1, z1);
            for y in (y0..=y1).step_by(3) {
                for z in (z0..=z1).step_by(3) {
                    for x in (x0..=x1).step_by(3) {
                        let via_lattice = field.biome_id_lat(&lat, x, y, z);
                        assert_eq!(
                            field.underground_biome_at(x, y, z),
                            via_lattice,
                            "ABI/carver divergence at ({x},{y},{z})"
                        );
                        seen.insert(via_lattice);
                    }
                }
            }
        }
        assert!(
            seen.len() > 1 || !has_banded_rows(table),
            "sample should span more than one biome"
        );
    }
}

#[test]
fn the_box_query_never_omits_a_biome_the_point_query_answers() {
    for (table, excavations) in tables() {
        let field = CaveField::with_tables(0xB10, table, excavations);
        let mut spanned = std::collections::BTreeSet::new();
        for (x0, y0, z0) in [
            (-6, -50, 30),
            (-281, -19, 151),
            (191, 7, -239),
            (-141, 41, 321),
            (263, -45, -71),
        ]
        .into_iter()
        .chain(territory_boxes(&field))
        {
            let (x1, y1, z1) = (x0 + 15, y0 + 15, z0 + 15);
            let ids = field.underground_biome_ids_in_box([x0, y0, z0], [x1, y1, z1]);
            for y in y0..=y1 {
                for z in (z0..=z1).step_by(3) {
                    for x in (x0..=x1).step_by(3) {
                        let id = field.underground_biome_at(x, y, z);
                        spanned.insert(id);
                        assert!(
                            ids.contains(id),
                            "box query omitted biome {id} that owns ({x},{y},{z})"
                        );
                    }
                }
            }
        }
        assert!(
            spanned.len() > 1 || !has_banded_rows(table),
            "sample should span more than one biome, or it proves nothing"
        );
    }
}

#[test]
fn overlapping_lattices_agree_at_shared_voxels() {
    let field = CaveField::new(0xC0FFEE);
    let a = field.build_lattice(0, 0, 0, 15, 15, 15);
    let b = field.build_lattice(-16, 4, 8, 15, 35, 23);
    for &(x, y, z) in &[(0, 4, 8), (7, 15, 15), (15, 12, 9), (3, 8, 15)] {
        let (mut ca, mut cb) = (Col::new(&a, x, z), Col::new(&b, x, z));
        for k in [
            lane::ENTRANCE,
            lane::NOODLE_A,
            lane::NOODLE_TOGGLE,
            lane::NOODLE_WIDTH,
        ] {
            assert_eq!(ca.get(k, y).to_bits(), cb.get(k, y).to_bits());
        }
        assert_eq!(
            a.climate_at(x, y, z).map(f64::to_bits),
            b.climate_at(x, y, z).map(f64::to_bits)
        );
    }
}

#[test]
fn lining_shell_is_disjoint_from_carved_air() {
    for (table, excavations) in tables() {
        let field = CaveField::with_tables(0xBEEF, table, excavations);
        let (x0, y0, z0) = (32, -32, -16);
        let (x1, y1, z1) = (x0 + 31, y0 + 31, z0 + 31);
        let lat = field.build_lattice(x0, y0, z0, x1, y1, z1);
        let surf_y = 90;
        let (mut open, mut shell, mut solid) = (0usize, 0usize, 0usize);
        for y in y0..=y1 {
            for z in z0..=z1 {
                for x in x0..=x1 {
                    match field.cut_lat(&lat, x, y, z, surf_y) {
                        CaveCut::Air => open += 1,
                        CaveCut::Shell => shell += 1,
                        CaveCut::Solid | CaveCut::Barrier(_) => solid += 1,
                        CaveCut::Fill(b) => {
                            if Block::from_id(b).is_solid() {
                                solid += 1;
                            } else {
                                open += 1;
                            }
                        }
                    }
                }
            }
        }
        let total = (open + shell + solid) as f64;
        assert!(open > 0, "test volume should contain cave air");
        assert!(shell > 0, "test volume should contain wall shell");
        assert!(
            (shell as f64) < total * 0.5,
            "shell must be a lining, not a region fill ({shell}/{total})"
        );
    }
}

#[test]
fn a_declared_floor_lining_paints_every_cave_floor_in_its_biome() {
    const FLOORED: &str = r#"{
  "underground_biomes": [
    {
      "underground_biome": "moss:everywhere",
      "y": [
        -64,
        -16
      ],
      "lining": {
        "block": "petramond:moss_block",
        "shell": 1.4,
        "faces": {
          "floor_depth": 2,
          "ceiling": {
            "weight": 0.0
          }
        },
        "blend": [
          0.02,
          4
        ]
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
}"#;
    let table = underground::test_table(&[FLOORED]);
    let row = table.id("moss:everywhere").expect("the floored row");
    let moss = petramond_world::block::Block::MossBlock.id();
    let (air, stone) = (Block::Air.id(), Block::Stone.id());
    let surf = vec![40i32; SECTION_SIZE * SECTION_SIZE];
    for seed in [0x312u32, 0x1D001, 0x2BEEF] {
        let field = CaveField::with_tables(
            seed,
            table,
            crate::data::excavations::test_table(&[LINING_ROOMS], table),
        );
        let mut floors = 0usize;
        for (cx, cz) in [(0, 0), (-3, 2), (7, -5), (11, 9), (-14, -8), (4, 17)] {
            let carved: Vec<Section> = (-4..=-2)
                .map(|cy| {
                    let mut s = Section::new(cx, cy, cz);
                    s.blocks_mut().fill(stone);
                    field.carve_section(&mut s, &surf);
                    s
                })
                .collect();
            let at = |wy: i32, x: usize, z: usize| {
                let cy = wy.div_euclid(SECTION_SIZE as i32);
                carved.get((cy + 4) as usize).map(|s| {
                    s.blocks_iter().collect::<Vec<_>>()
                        [section_idx(x, wy.rem_euclid(SECTION_SIZE as i32) as usize, z)]
                })
            };
            for z in 0..SECTION_SIZE {
                for x in 0..SECTION_SIZE {
                    for wy in -64..-16 {
                        let (Some(here), Some(above)) = (at(wy, x, z), at(wy + 1, x, z)) else {
                            continue;
                        };
                        let wx = cx * SECTION_SIZE as i32 + x as i32;
                        let wz = cz * SECTION_SIZE as i32 + z as i32;
                        if above != air
                            || here == air
                            || Block::from_id(here).is_fluid()
                            || field.underground_biome_at(wx, wy, wz) != row
                        {
                            continue;
                        }
                        floors += 1;
                        if here != moss {
                            panic!(
                                    "cave floor at ({wx},{wy},{wz}) is block {here}, not the                                      declared floor lining (seed {seed:#x})"
                                );
                        }
                    }
                }
            }
        }
        assert!(
            floors > 200,
            "only {floors} cave floors swept (seed {seed:#x})"
        );
    }
}

const ORIENTED: &str = r#"{
  "underground_biomes": [
    {
      "underground_biome": "faces:everywhere",
      "y": [
        -64,
        -16
      ],
      "lining": {
        "block": "petramond:moss_block",
        "shell": 1.4,
        "faces": {
          "floor_depth": 3,
          "floor": {
            "block": "petramond:moss_block"
          },
          "wall": {
            "block": "petramond:marble"
          },
          "ceiling": {
            "block": "petramond:gravel"
          }
        },
        "blend": [
          0.02,
          4
        ]
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
}"#;

const LAYERED: &str = r#"{
  "underground_biomes": [
    {
      "underground_biome": "faces:layered",
      "y": [
        -64,
        -16
      ],
      "lining": {
        "block": "petramond:moss_block",
        "shell": 1.4,
        "faces": {
          "floor_depth": 3,
          "floor": {
            "block": "petramond:moss_block"
          },
          "floor_under": {
            "block": "petramond:marble"
          }
        },
        "blend": [
          0.02,
          4
        ]
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
}"#;

/// Exactly ONE surface cell per floor course, with the subsurface under
/// it — including where the course top sits in the box ABOVE. That case is
/// the whole hazard: a run beginning mid-course cannot see its own top, so
/// a painter that assumes depth 0 lays a second surface on every section
/// boundary, which reads as stripes of moss buried in the rock.
#[test]
fn a_layered_floor_course_has_one_surface_wherever_the_batch_splits_it() {
    let mut pack: serde_json::Value = serde_json::from_str(LAYERED).unwrap();
    pack["underground_biomes"][0]["lining"]["faces"]["floor_depth"] = serde_json::json!({
        "max": 13,
        "extent": ["select", ["lt", "x", 0], 11, 6]
    });
    let table = underground::test_table(&[&pack.to_string()]);
    let row = table.id("faces:layered").expect("the layered row");
    let (moss, under) = (Block::MossBlock.id(), Block::Marble.id());
    let (air, stone) = (Block::Air.id(), Block::Stone.id());
    let surf = vec![40i32; SECTION_SIZE * SECTION_SIZE];
    let (mut courses, mut split) = (0usize, 0usize);
    for seed in [0x312u32, 0x1D001, 0x2BEEF] {
        let field = CaveField::with_tables(
            seed,
            table,
            crate::data::excavations::test_table(&[LINING_ROOMS], table),
        );
        for (cx, cz) in [(0, 0), (-3, 2), (7, -5), (11, 9)] {
            let carved: Vec<Section> = (-4..=-2)
                .map(|cy| {
                    let mut s = Section::new(cx, cy, cz);
                    s.blocks_mut().fill(stone);
                    field.carve_section(&mut s, &surf);
                    s
                })
                .collect();
            let at = |wy: i32, x: usize, z: usize| {
                let cy = wy.div_euclid(SECTION_SIZE as i32);
                carved.get((cy + 4) as usize).map(|s| {
                    s.blocks_iter().collect::<Vec<_>>()
                        [section_idx(x, wy.rem_euclid(SECTION_SIZE as i32) as usize, z)]
                })
            };
            for z in 0..SECTION_SIZE {
                for x in 0..SECTION_SIZE {
                    let wx = cx * SECTION_SIZE as i32 + x as i32;
                    let wz = cz * SECTION_SIZE as i32 + z as i32;
                    for wy in -63..-17 {
                        let (Some(here), Some(above)) = (at(wy, x, z), at(wy + 1, x, z)) else {
                            continue;
                        };
                        if here == air
                            || above != air
                            || Block::from_id(here).is_fluid()
                            || field.underground_biome_at(wx, wy, wz) != row
                        {
                            continue;
                        }
                        courses += 1;
                        split += (wy.rem_euclid(SECTION_SIZE as i32) == 0) as usize;
                        assert_eq!(
                            here, moss,
                            "({wx},{wy},{wz}) tops a course but is not the surface \
                                 (seed {seed:#x})"
                        );
                        for d in 1..if wx < 0 { 11 } else { 6 } {
                            let Some(deep) = at(wy - d, x, z) else { break };
                            if deep == air || Block::from_id(deep).is_fluid() {
                                break;
                            }
                            assert_eq!(
                                deep,
                                under,
                                "({wx},{},{wz}) is {d} under the course top and should be \
                                     subsurface, not a second surface (seed {seed:#x})",
                                wy - d
                            );
                        }
                    }
                }
            }
        }
    }
    assert!(courses > 0, "only {courses} course tops swept");
    assert!(split > 0, "no course top swept on a section-floor plane");
}

const POOLS_EVERYWHERE: &str = r#"{"fluid_pools":[{"fluid_pool":"test:lava",
    "fluid":"petramond:lava","anchor_y":-40,"chance":1.0,"height_scale":1000.0,"max_y":0,
    "reach":16,"max_depth":10,"max_drop":24,"max_sink":16,"budget":2000,
    "surface_clearance":32}]}"#;

fn floor_courses_under(fluid: Block, submerged: bool) {
    let mut pack: serde_json::Value = serde_json::from_str(LAYERED).unwrap();
    let row = pack["underground_biomes"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|row| row["underground_biome"] == "faces:layered")
        .unwrap();
    row["lining"]["faces"]["floor_submerged"] = serde_json::json!({
        "block":"petramond:dirt", "pattern":{
            "material":["lt","x",0], "palette":["petramond:sand","petramond:sandstone"]
        }
    });
    let mut layers = Vec::new();
    if fluid == Block::Water {
        row["aquifer"] = serde_json::json!({"level":-20,"barrier":"petramond:stone"});
    } else {
        row["lining"]["faces"]["submerged_in"] = serde_json::json!(["petramond:water"]);
        layers.push(POOLS_EVERYWHERE);
    }
    let pack = pack.to_string();
    layers.push(&pack);
    let table = underground::synthetic_table(&layers);
    let row = table.id("faces:layered").unwrap();
    let is_fluid = |b: u16| Block::from_id(b).is_fluid();
    let mut floors = 0;
    let (mut tops_on_floor_plane, mut tops_on_top_plane) = (0, 0);
    for seed in [0x312, 0x1d001, 0x2beef] {
        if floors > 0 && tops_on_floor_plane > 0 && tops_on_top_plane > 0 {
            break;
        }
        let field = CaveField::with_tables(
            seed,
            table,
            crate::data::excavations::test_table(&[LINING_ROOMS], table),
        );
        for (cx, cz) in (-4..4).flat_map(|cz| (-4..4).map(move |cx| (cx, cz))) {
            let sections: Vec<Vec<_>> = (-4..=-2)
                .map(|cy| {
                    let mut section = Section::new(cx, cy, cz);
                    section.blocks_mut().fill(Block::Stone.id());
                    field.carve_section(&mut section, &[40; 256]);
                    section.blocks_iter().collect()
                })
                .collect();
            let at = |y: i32, x: usize, z: usize| {
                sections[(y.div_euclid(16) + 4) as usize]
                    [section_idx(x, y.rem_euclid(16) as usize, z)]
            };
            for x in 0..16 {
                for z in 0..16 {
                    for y in -61..-21 {
                        let wx = cx * 16 + x as i32;
                        let wz = cz * 16 + z as i32;
                        let here = at(y, x, z);
                        if here == Block::Air.id()
                            || is_fluid(here)
                            || at(y + 1, x, z) != fluid.id()
                            || field.underground_biome_at(wx, y, wz) != row
                        {
                            continue;
                        }
                        floors += 1;
                        tops_on_floor_plane += usize::from(y.rem_euclid(16) == 0);
                        tops_on_top_plane += usize::from(y.rem_euclid(16) == 15);
                        let surface = match (submerged, wx < 0) {
                            (true, true) => Block::Sandstone,
                            (true, false) => Block::Sand,
                            (false, _) => Block::MossBlock,
                        };
                        assert_eq!(
                            here,
                            surface.id(),
                            "floor under {fluid:?} at {wx},{y},{wz} (seed {seed:#x})"
                        );
                        for depth in 1..=2 {
                            let below = at(y - depth, x, z);
                            if below == Block::Air.id() || is_fluid(below) {
                                break;
                            }
                            assert_eq!(
                                below,
                                Block::Marble.id(),
                                "course under {fluid:?} has a second surface at {wx},{},{wz}",
                                y - depth
                            );
                        }
                    }
                }
            }
        }
    }
    assert!(
        floors > 0 && tops_on_floor_plane > 0 && tops_on_top_plane > 0,
        "the fixture must exercise courses split both ways ({floors} floors, \
         {tops_on_floor_plane} on a section floor, {tops_on_top_plane} on a section top)"
    );
}

#[test]
fn submerged_floor_patterns_keep_one_surface_across_section_boundaries() {
    floor_courses_under(Block::Water, true);
}

#[test]
fn a_submerged_floor_lining_applies_only_under_the_fluids_it_lists() {
    floor_courses_under(Block::Lava, false);
}

/// Which orientation a cell has, and how far a floor course reaches into
/// it, are properties of the CAVE — not of the box the carve happened to
/// be batched into. Both are loop-carried state in the column walk, so
/// both are only as good as what the walk does at a box boundary, and a
/// 16-block section has a boundary every sixteen voxels.
///
/// Categorical, not statistical: every rock cell over carved air must be
/// off the WALL rule, and every stone cell within the declared course of a
/// cave floor must carry the floor block.
#[test]
fn face_orientation_and_course_depth_do_not_depend_on_the_batch() {
    const DEPTH: i32 = 3;
    let table = underground::test_table(&[ORIENTED]);
    let row = table.id("faces:everywhere").expect("the oriented row");
    let moss = Block::MossBlock.id();
    let wall = Block::Marble.id();
    let (air, stone) = (Block::Air.id(), Block::Stone.id());
    let surf = vec![40i32; SECTION_SIZE * SECTION_SIZE];
    let mut boundary = 0usize;
    for seed in [0x312u32, 0x1D001, 0x2BEEF] {
        let field = CaveField::with_tables(
            seed,
            table,
            crate::data::excavations::test_table(&[LINING_ROOMS], table),
        );
        let (mut ceilings, mut course) = (0usize, 0usize);
        for (cx, cz) in [(0, 0), (-3, 2), (7, -5), (11, 9), (-14, -8), (4, 17)] {
            let carved: Vec<Section> = (-4..=-2)
                .map(|cy| {
                    let mut s = Section::new(cx, cy, cz);
                    s.blocks_mut().fill(stone);
                    field.carve_section(&mut s, &surf);
                    s
                })
                .collect();
            let at = |wy: i32, x: usize, z: usize| {
                let cy = wy.div_euclid(SECTION_SIZE as i32);
                carved.get((cy + 4) as usize).map(|s| {
                    s.blocks_iter().collect::<Vec<_>>()
                        [section_idx(x, wy.rem_euclid(SECTION_SIZE as i32) as usize, z)]
                })
            };
            for z in 0..SECTION_SIZE {
                for x in 0..SECTION_SIZE {
                    let wx = cx * SECTION_SIZE as i32 + x as i32;
                    let wz = cz * SECTION_SIZE as i32 + z as i32;
                    let owned = |wy: i32| field.underground_biome_at(wx, wy, wz) == row;
                    for wy in -64..-16 {
                        let (Some(here), Some(below)) = (at(wy, x, z), at(wy - 1, x, z)) else {
                            continue;
                        };
                        if here != air && below == air && at(wy + 1, x, z) != Some(air) && owned(wy)
                        {
                            ceilings += 1;
                            boundary += (wy.rem_euclid(SECTION_SIZE as i32) == 0) as usize;
                            assert_ne!(
                                here, wall,
                                "({wx},{wy},{wz}) is over carved air but took the WALL \
                                     rule (seed {seed:#x})"
                            );
                        }
                        if here != air || !owned(wy - 1) {
                            continue;
                        }
                        for d in 1..=DEPTH {
                            let Some(deep) = at(wy - d, x, z) else { break };
                            if deep == air || Block::from_id(deep).is_fluid() {
                                break;
                            }
                            course += 1;
                            assert_eq!(
                                deep,
                                moss,
                                "({wx},{},{wz}) is {d} under a cave floor but is not the \
                                     declared course (seed {seed:#x})",
                                wy - d
                            );
                        }
                    }
                }
            }
        }
        assert!(course > 0, "only {course} course cells (seed {seed:#x})");
        assert!(ceilings > 0, "only {ceilings} ceilings (seed {seed:#x})");
    }
    assert!(boundary > 0, "no ceiling swept on a section-floor plane");
}

fn territory_boxes(field: &CaveField) -> Vec<(i32, i32, i32)> {
    let mut representatives = std::collections::BTreeMap::new();
    for x in (-2048..2048).step_by(384) {
        for z in (-2048..2048).step_by(384) {
            for y in [-40, -16, 8, 40] {
                representatives
                    .entry(field.underground_biome_at(x, y, z))
                    .or_insert((x, y, z));
            }
        }
    }
    representatives.into_values().collect()
}
