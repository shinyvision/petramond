use super::*;
use crate::data::underground;

/// The batched positional queries share ONE lattice across a whole box and
/// walk a column through a single cursor; both are pure scheduling, so they
/// must answer exactly what the one-voxel lattice answers, position for
/// position, whatever the batch happens to contain. A drift here is
/// invisible — a mod's structures shift by a cell somewhere deep — so it is
/// pinned against the reference path directly, including habitat lining and regional selection and singleton batches.
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
        // Plus a dense column and a dense slab, the two shapes the
        // subdivision is supposed to collapse into one lattice.
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

/// Regional habitat selection with an independent bounded excavation.
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

/// Narrow lining bands exercise conservative per-cell shell bounds.
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

/// A row declaring a CHAMBER: a term summed straight into the cavern carve
/// threshold, so it arms a lattice lane the shipped table never allocates
/// and widens the skip mask's cheese bound. Every carve invariant re-runs
/// against it for exactly that reason — it is the newest way for the point
/// and batch paths, and for the mask and the carver, to drift apart.
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

/// A chamber row banded to a rare field value AND a narrow depth, so most
/// of any test box lies outside the lane's declared span. That is where a
/// gate on the lane can be wrong in the dangerous direction — skipping it
/// somewhere a room does reach.
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

/// Synthetic rooms provide floor and ceiling surfaces independently of habitats.
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

/// Boxes worth sweeping for a table: a fixed scatter, plus — when the table
/// declares chambers — boxes straddling the RIM of every room the seed
/// actually rolls nearby. The rim is where a room's term is neither zero
/// nor saturated, i.e. the only place a bound or a lane gate can be wrong
/// in a way that still produces plausible output.
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
        // A handful is enough; every extra room multiplies a cubic sweep.
        for c in rooms.centers().into_iter().take(3) {
            // One box on the room's core, and four crossing its rim from
            // different sides, so both the saturated and the dissolving
            // part of the term are swept.
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

/// The invariant everything hangs on: the point path (surface walks feeding
/// column heightmaps) and the batch path (the actual block carve) must agree at
/// every voxel, or heightmaps drift from carved blocks and skylight breaks.
///
/// With independent excavation this is also the hazard the sparse path is most
/// prone to: the point query builds a PARTIAL lattice, so a producer that
/// skips a field the consumer reads shows up here as a wrong answer (and in
/// debug as the named lattice assertion) long before it can become a bounds
/// panic in production. A CHAMBER lane raises the stakes: it is skipped for
/// whole boxes, so producer and consumer must agree not only about how to
/// read it but about when it is provably zero.
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
                                // Checking only y+1 would miss courses that
                                // outgrow the mask's vertical dilation.
                                if field.lining_faces {
                                    for d in 1..=table.lining_floor_depth_max {
                                        assert_ne!(
                                            field.cut_lat(&lat, x, y + d, z, surf_y),
                                            CaveCut::Open,
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

/// A chamber lane is SKIPPED for boxes no declared room can reach, which is
/// only sound if the term there is provably zero. Probe the depth span from
/// both sides: just inside it the lane must be live somewhere, and just
/// outside it every gathered room must contribute exactly nothing — a lane
/// gate that is one block too tight deletes the top or bottom slice of
/// every room in the world and nothing downstream can tell.
#[test]
fn the_chamber_lane_is_only_skipped_where_no_room_can_reach() {
    for (table, excavations) in tables() {
        let Some((lo, hi)) = excavations.y_span else {
            continue;
        };
        let field = CaveField::with_tables(0x1D001, table, excavations);
        // Outside the span the gather itself must come back empty over a
        // wide box, whatever the field says.
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
        // Inside it, the lane must be live for at least one real box, or
        // the span is so loose that skipping never happens.
        let live = sweep_boxes(&field, 19).into_iter().any(|[x, y, z]| {
            field
                .build_lattice(x, y, z, x + 19, y + 19, z + 19)
                .chamber_is_live()
        });
        assert!(live, "the declared span never yields a live lane");
    }
}

/// The mod ABI's underground-biome query must answer exactly what the
/// habitat lining reads at that cell — the contract a pack's
/// content placement depends on ("place inside my biome, get my caves").
#[test]
fn abi_query_agrees_with_the_carver() {
    for (table, excavations) in tables() {
        let field = CaveField::with_tables(0xB10, table, excavations);
        let mut seen = std::collections::BTreeSet::new();
        // Underground-biome regions span a few hundred blocks, so the
        // sample is scattered boxes rather than one — a single small box
        // would sit inside one biome and prove nothing about boundaries.
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

/// The box query is a REJECTION gate, so its only real contract is the one
/// direction: whatever `underground_biome_at` answers anywhere in the box
/// must appear in the box's set. A bound that tightens — a lattice cell
/// missed at the box edge, a clamped depth window, a memo keyed too coarsely
/// — silently deletes every mod feature that gates on it, and nothing else
/// would notice. Boxes deliberately straddle the memo grid so the snap-out
/// is exercised rather than the aligned happy path.
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

/// Batch lattices are world-anchored, so two different boxes covering the same
/// voxel interpolate identical values: section seams cannot show.
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

/// Wall lining is a shell AROUND carved air, never a replacement for it: a
/// voxel the carvers open can never simultaneously be lining, and lining only
/// appears in a bounded band next to carve decisions (guards against a shell
/// threshold inversion silently turning whole regions into lining). Re-run
/// with a `lining.shell` multiplier so the knob a pack can turn is covered
/// by the same ceiling, not just the engine's own shell widths.
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
                        CaveCut::Open => open += 1,
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

/// A declared FLOOR lining is a GUARANTEE, not a coverage percentage: the
/// spawn rules a pack hangs off it are only as good as its worst cell, and
/// a 95% floor is a hole a player finds and the author never does.
///
/// Swept over real carved sections, including the two places the rule has
/// no memory to fall back on — the top voxel of a section (its air
/// neighbour lives in the next section up) and the world-floor plane the
/// carvers refuse to cut, which is the flattest, most walkable floor in the
/// biome and sits in a section the carve otherwise skips outright.
#[test]
fn a_declared_floor_lining_paints_every_cave_floor_in_its_biome() {
    // A wide band, so the sweep finds plenty of the row's own cells — but
    // NARROWER rows still win inside it (marble does), which is why the
    // assertion below is gated on who actually owns the cell rather than
    // on the box.
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
            // Stack of sections over one column, so the voxel above a
            // section's top voxel is a real neighbour and not a guess.
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

/// A row lining three ORIENTATIONS with three different blocks. Weight-only
/// fixtures are what let a ceiling-taken-for-a-wall live: the two rules
/// then write the same block and differ only in how much of it there is.
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

/// A row layering a SUBSURFACE under its floor: moss on top, marble for
/// the two courses below it.
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
                        // A course top: solid, carved air directly above.
                        let (Some(here), Some(above)) = (at(wy, x, z), at(wy + 1, x, z)) else {
                            continue;
                        };
                        if here == air
                            || above != air
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
                            if deep == air {
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
    assert!(courses > 300, "only {courses} course tops swept");
    // Vacuous unless the sweep actually crossed a course split by a
    // section plane, which is the case the depth argument exists for.
    assert!(split > 0, "no course top swept on a section-floor plane");
}

#[test]
fn submerged_floor_patterns_keep_one_surface_across_section_boundaries() {
    let mut pack: serde_json::Value = serde_json::from_str(LAYERED).unwrap();
    let row = pack["underground_biomes"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|row| row["underground_biome"] == "faces:layered")
        .unwrap();
    row["aquifer"] = serde_json::json!({"level":-20,"barrier":"petramond:stone"});
    row["lining"]["faces"]["floor_submerged"] = serde_json::json!({
        "block":"petramond:dirt", "pattern":{
            "material":["lt","x",0], "palette":["petramond:sand","petramond:sandstone"]
        }
    });
    let table = underground::test_table(&[&pack.to_string()]);
    let row = table.id("faces:layered").unwrap();
    let mut floors = 0;
    let mut splits = 0;
    for seed in [0x312, 0x1d001, 0x2beef] {
        let field = CaveField::with_tables(
            seed,
            table,
            crate::data::excavations::test_table(&[LINING_ROOMS], table),
        );
        for (cx, cz) in [(0, 0), (-3, 2), (7, -5), (11, 9)] {
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
                            || here == Block::Water.id()
                            || at(y + 1, x, z) != Block::Water.id()
                            || field.underground_biome_at(wx, y, wz) != row
                        {
                            continue;
                        }
                        floors += 1;
                        splits += usize::from(y.rem_euclid(16) == 0);
                        assert_eq!(
                            here,
                            if wx < 0 {
                                Block::Sandstone.id()
                            } else {
                                Block::Sand.id()
                            }
                        );
                        for depth in 1..=2 {
                            let below = at(y - depth, x, z);
                            if below == Block::Air.id() || below == Block::Water.id() {
                                break;
                            }
                            assert_eq!(
                                below,
                                Block::Marble.id(),
                                "submerged course has a second surface at {wx},{},{wz}",
                                y - depth
                            );
                        }
                    }
                }
            }
        }
    }
    assert!(
        floors > 0 && splits > 0,
        "the fixture must exercise submerged, split courses"
    );
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
                        // A cell that is a floor too is repainted by the
                        // floor rule, which outranks both side rules.
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
                            if deep == air {
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
        assert!(course > 400, "only {course} course cells (seed {seed:#x})");
        assert!(ceilings > 100, "only {ceilings} ceilings (seed {seed:#x})");
    }
    // Coverage, not a shape pin: the assertions above are vacuous unless
    // the sweep reached a ceiling standing on a section FLOOR, which is
    // the only plane where the walk cannot remember what is under it.
    assert!(boundary > 0, "no ceiling swept on a section-floor plane");
}

/// The two batch paths must produce the same blocks. They share the column
/// walk, which is necessary and not sufficient: the walk's carry is seeded
/// at the box floor and flushed at the box top, and those are different
/// voxels for a 256-tall chunk and a 16-tall section.
#[test]
fn the_two_batch_carve_paths_line_a_cave_identically() {
    // Banded to positive Y so the whole-column path, which starts at y=0,
    // walks the row at all.
    const BANDED: &str = r#"{
  "underground_biomes": [
    {
      "underground_biome": "faces:band",
      "y": [
        0,
        40
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
    let table = underground::test_table(&[BANDED]);
    let (air, stone) = (Block::Air.id(), Block::Stone.id());
    let csurf = vec![60i32; CHUNK_SX * CHUNK_SZ];
    let ssurf = vec![60i32; SECTION_SIZE * SECTION_SIZE];
    // Rock that is NOT all stone. Only a stone cell takes a lining, so a
    // fixture of pure stone cannot see a course whose reach depends on
    // block ids the neighbouring batch can read and this one cannot.
    let rock = |wx: i32, wy: i32, wz: i32| {
        if (wx * 31 + wy * 7 + wz * 13).rem_euclid(5) == 0 {
            Block::Tuff.id()
        } else {
            stone
        }
    };
    let mut lined = 0usize;
    for seed in [0x312u32, 0x1D001] {
        let field = CaveField::with_tables(
            seed,
            table,
            crate::data::excavations::test_table(&[LINING_ROOMS], table),
        );
        for (cx, cz) in [(0, 0), (5, -3)] {
            let mut chunk = Chunk::new(cx, cz);
            for z in 0..CHUNK_SZ {
                for y in 0..CHUNK_SY {
                    for x in 0..CHUNK_SX {
                        let (wx, wz) = (
                            cx * CHUNK_SX as i32 + x as i32,
                            cz * CHUNK_SZ as i32 + z as i32,
                        );
                        chunk.blocks_slice_mut()[idx(x, y, z)] = rock(wx, y as i32, wz);
                    }
                }
            }
            field.carve_chunk(&mut chunk, &csurf);
            for cy in 0..3i32 {
                let mut s = Section::new(cx, cy, cz);
                s.edit_ids_bulk(|dst| {
                    for z in 0..SECTION_SIZE {
                        for ly in 0..SECTION_SIZE {
                            for x in 0..SECTION_SIZE {
                                let (wx, wz) = (
                                    cx * SECTION_SIZE as i32 + x as i32,
                                    cz * SECTION_SIZE as i32 + z as i32,
                                );
                                let wy = cy * SECTION_SIZE as i32 + ly as i32;
                                dst[section_idx(x, ly, z)] = rock(wx, wy, wz);
                            }
                        }
                    }
                });
                field.carve_section(&mut s, &ssurf);
                for z in 0..SECTION_SIZE {
                    for ly in 0..SECTION_SIZE {
                        for x in 0..SECTION_SIZE {
                            let wy = cy as usize * SECTION_SIZE + ly;
                            let want = chunk.blocks_slice()[idx(x, wy, z)];
                            let got = s.block_raw(x, ly, z);
                            assert_eq!(
                                got, want,
                                "section and chunk disagree at ({x},{wy},{z}) of chunk \
                                     ({cx},{cz}) seed {seed:#x}"
                            );
                            let base = rock(
                                cx * SECTION_SIZE as i32 + x as i32,
                                wy as i32,
                                cz * SECTION_SIZE as i32 + z as i32,
                            );
                            lined += (got != base && got != air) as usize;
                        }
                    }
                }
            }
        }
    }
    assert!(lined > 500, "only {lined} lined cells compared");
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
