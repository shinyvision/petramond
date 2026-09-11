use super::*;
use crate::data::underground;
use crate::noise::settings::CAVE_MIN_Y;

#[test]
fn stretched_rooms_keep_their_bounds_and_shared_corner_values() {
    let json = PACK.replace("\"spacing\": 64", "\"spacing\": 160").replace(
        "\"flatten\": 0.45",
        "\"flatten\": 0.45, \"stretch\": [2, 4]",
    );
    let table = underground::test_table(&[&json]);
    let excavations = crate::data::excavations::test_table(&[&json], table);
    let ch = *excavations.rows[0].chamber().unwrap();
    let mut count = 0;
    for seed in [0x312, 0xbeef] {
        let f = field(&json, table, seed);
        for room in rolled(&f) {
            count += 1;
            let [x, y, z] = room.center;
            assert!(room.ex <= ch.reach_xz());
            let small = f.chamber_field([x - 16, y - 8, z - 16], [x + 16, y + 8, z + 16]);
            let big = f.chamber_field([x - 160, y - 80, z - 160], [x + 160, y + 80, z + 160]);
            for dx in (-16..=16).step_by(4) {
                for dz in (-16..=16).step_by(4) {
                    let k = f.knead_sample(x + dx, y, z + dz);
                    let a = small.at(x + dx, y, z + dz, k);
                    let b = big.at(x + dx, y, z + dz, k);
                    assert_eq!(
                        (a.0.to_bits(), a.1.to_bits()),
                        (b.0.to_bits(), b.1.to_bits())
                    );
                }
            }
            for k in [-0.55, 0.0, 0.55] {
                for d in [-ch.reach_xz(), ch.reach_xz()] {
                    assert_eq!(room.at(x + d, y, z, k), (0.0, 0.0));
                    assert_eq!(room.at(x, y, z + d, k), (0.0, 0.0));
                }
            }
            let lobe = room.lobes[0];
            let distance = lobe.rx * 1.8;
            let along = lobe.at(
                y,
                distance * lobe.axis[0],
                0.0,
                distance * lobe.axis[1],
                8.0,
                1.0,
                1.0,
            );
            let across = lobe.at(
                y,
                -distance * lobe.axis[1],
                0.0,
                distance * lobe.axis[0],
                8.0,
                1.0,
                1.0,
            );
            assert!(
                along > across,
                "the horizontal axis must change the carved shape"
            );
        }
    }
    assert!(count > 0);
}

const PACK: &str = r#"{
  "underground_biomes": [
    {
      "underground_biome": "mymod:cathedral",
      "y": [
        -64,
        -16
      ],
      "lining": {
        "block": "petramond:moss_block",
        "shell": 1.4
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

fn field(
    layer: &str,
    table: &'static underground::UndergroundBiomes,
    seed: u32,
) -> super::super::cave_field::CaveField {
    super::super::cave_field::CaveField::with_tables(
        seed,
        table,
        crate::data::excavations::test_table(&[layer], table),
    )
}

/// Every room the seed actually rolls over a wide box — the only honest
/// population to sweep, since a hand-built `Room` cannot exercise the lobe
/// roll and a lobe offset missing from a reach bound is a carve seam.
fn rolled(f: &super::super::cave_field::CaveField) -> Vec<Room> {
    let g = f.chamber_field([-768, -64, -768], [768, -8, 768]);
    g.rooms.clone()
}

/// The invariant the whole module is arranged around: a corner's value may
/// not depend on which box asked for it. A superset gather must add only
/// exact zeros, and the surviving rooms must accumulate in the same order —
/// f64 addition is not associative, so "close enough" is a seam. BOTH lanes
/// are checked: the tunnel gain rides the same accumulation.
#[test]
fn a_chamber_corner_does_not_depend_on_the_box_it_was_gathered_for() {
    let table = underground::test_table(&[PACK]);
    for seed in [0x312u32, 0x1D001, 0xBEEF] {
        let f = field(PACK, table, seed);
        let step = CAVE_LATTICE_STEP;
        for (lo, hi) in [
            ([0, -48, 0], [16, -32, 16]),
            ([-64, -64, -64], [64, -16, 64]),
            ([-256, -60, 128], [-240, -44, 144]),
        ] {
            let small = f.chamber_field(lo, hi);
            // A box grown by two whole lattice cells of the chamber grid
            // gathers a strict superset of the candidates.
            let big = f.chamber_field(
                [lo[0] - 128, lo[1] - 128, lo[2] - 128],
                [hi[0] + 128, hi[1] + 128, hi[2] + 128],
            );
            let mut y = lo[1];
            while y <= hi[1] {
                let mut z = lo[2];
                while z <= hi[2] {
                    let mut x = lo[0];
                    while x <= hi[0] {
                        let k = f.knead_sample(x, y, z);
                        let (a, b) = (small.at(x, y, z, k), big.at(x, y, z, k));
                        assert_eq!(
                            (a.0.to_bits(), a.1.to_bits()),
                            (b.0.to_bits(), b.1.to_bits()),
                            "chamber at ({x},{y},{z}) differs by gather box (seed {seed:#x})"
                        );
                        x += step;
                    }
                    z += step;
                }
                y += step;
            }
        }
    }
}

/// The profile must be EXACTLY zero past the reach every bound is derived
/// from — the lattice-lane gate, the candidate window and the skip mask all
/// rest on it — and must saturate somewhere inside, or the room is a sponge
/// rather than a room.
///
/// Swept over ROLLED rooms rather than a literal, because the reach a lobed
/// room has to respect is the ROW's (`reach_xz` / `rise`), and only the roll
/// can put a satellite where that bound is tight.
#[test]
fn a_rolled_room_is_exactly_zero_past_its_declared_reach() {
    let table = underground::test_table(&[PACK]);
    let excavations = crate::data::excavations::test_table(&[PACK], table);
    let ch = *excavations.rows[0].chamber().unwrap();
    let mut rooms = 0usize;
    let mut rims = 0usize;
    for seed in [0x312u32, 0x1D001, 0x2BEEF] {
        let f = field(PACK, table, seed);
        for r in rolled(&f) {
            rooms += 1;
            let c = r.center;
            // The kneading field reaches about +-0.54; the reach must hold
            // for every value it can take, including the ones that widen
            // the ramp.
            let kneads = [-0.55, -0.2, 0.0, 0.2, 0.55];
            assert_eq!(
                r.at(c[0], c[1], c[2], 0.0).0,
                ch.strength,
                "the core must saturate"
            );
            for k in kneads {
                assert_eq!(
                    r.at(c[0], r.sill_y - 1, c[2], k),
                    (0.0, 0.0),
                    "below the sill"
                );
                for d in [ch.reach_xz(), ch.reach_xz() + 1, ch.reach_xz() + 97] {
                    for p in [
                        [c[0] + d, c[1], c[2]],
                        [c[0] - d, c[1], c[2]],
                        [c[0], c[1], c[2] + d],
                        [c[0], c[1], c[2] - d],
                    ] {
                        assert_eq!(
                            r.at(p[0], p[1], p[2], k),
                            (0.0, 0.0),
                            "horizontal reach {d} knead {k}"
                        );
                    }
                }
                // `rise` is taken over every radius the ROW can roll, so it
                // bounds this room whatever its lobes came out as.
                let rise = ch.extent_y().1;
                for d in [rise, rise + 1, rise + 97] {
                    assert_eq!(
                        r.at(c[0], c[1] + d, c[2], k),
                        (0.0, 0.0),
                        "vertical reach {d} knead {k}"
                    );
                }
            }
            // and it must actually blend, not step
            for k in 1..ch.feather as i32 {
                let v = r.at(c[0] + ch.r_min + k, c[1], c[2], 0.0).0;
                if v > 0.0 && v < ch.strength {
                    rims += 1;
                    break;
                }
            }
        }
    }
    assert!(
        rooms > 4,
        "only {rooms} rooms rolled; the sweep proves little"
    );
    assert_eq!(rims, rooms, "some room's rim is a step, not a blend");
}

/// Three things a room may never do: appear outside the territory its own
/// row declares (or a pack's cathedral turns up in another pack's biome),
/// stick out of that row's DEPTH band (or its roof is unlined, undressed,
/// and impossible to attribute to this file), or reach BELOW the carvable
/// floor — where it is sliced by bedrock into a dead-flat plane of bare
/// rock instead of tapering to its own sill. The last one is one `max()`
/// that a refactor loses in silence.
#[test]
fn a_room_stays_inside_its_row_territory_band_and_the_carvable_range() {
    let table = underground::test_table(&[PACK]);
    let excavations = crate::data::excavations::test_table(&[PACK], table);
    let placement = &excavations.rows[0].placement;
    let band = placement.y;
    let f = field(PACK, table, 0x1D001);
    let mut rooms = 0usize;
    for r in rolled(&f) {
        rooms += 1;
        assert_eq!(
            f.underground_biome_at(r.center[0], r.center[1], r.center[2]),
            placement.underground_biome.unwrap(),
            "room at {:?} is not in its own row's territory",
            r.center
        );
        assert!(
            r.sill_y >= CAVE_MIN_Y,
            "room {:?} sills at {} , below the carvable floor {CAVE_MIN_Y}",
            r.center,
            r.sill_y
        );
        assert!(
            r.sill_y >= band.0 && r.center[1] + r.ey <= band.1,
            "room {:?} sticks out of the band {band:?}",
            r.center
        );
        // and the profile agrees with the extents the band test used
        for k in [-0.55, 0.0, 0.55] {
            assert_eq!(r.at(r.center[0], band.0 - 1, r.center[2], k), (0.0, 0.0));
            assert_eq!(r.at(r.center[0], band.1 + 1, r.center[2], k), (0.0, 0.0));
        }
    }
    assert!(rooms > 0, "no room rolled; the test proves nothing");
}

/// A satellite lobe that clears the primary entirely leaves a detached
/// bubble: a sealed pocket, which is the exact failure the attachment work
/// exists to remove. The loader bounds `lobe_spread` against
/// `lobe_scale[0]` for that, and this is the roll actually obeying it —
/// the offsets are drawn per axis, so the diagonal is the case to check.
#[test]
fn no_rolled_satellite_lobe_can_detach_from_its_room() {
    let table = underground::test_table(&[PACK]);
    let f = field(PACK, table, 0x26AAC);
    let mut satellites = 0usize;
    for r in rolled(&f) {
        let p = r.lobes[0];
        for l in &r.lobes[1..r.n_lobes] {
            satellites += 1;
            // Normalised against the PRIMARY, both lobes are spheres: the
            // satellite is a sphere of radius `l.rx / p.rx` at this
            // distance from the origin.
            let d =
                ((l.off[0] / p.rx).powi(2) + (l.off[1] / p.ry).powi(2) + (l.off[2] / p.rx).powi(2))
                    .sqrt();
            assert!(
                d < 1.0 + l.rx / p.rx,
                "satellite at normalised distance {d} with radius {} is detached",
                l.rx / p.rx
            );
        }
    }
    assert!(
        satellites > 0,
        "no satellite rolled; the test proves nothing"
    );
}

#[test]
fn contact_admission_depends_on_the_geometry_source_and_survives_box_changes() {
    let json = PACK.replace(
        "\"spacing\": 64",
        "\"spacing\": 64, \"contact\": \"natural_cave\"",
    );
    let table = underground::test_table(&[&json]);
    let excavations = crate::data::excavations::test_table(&[&json], table);
    let lo = [-128, -48, -128];
    let hi = [128, 32, 128];
    let rejected = ChamberField::gather(
        &CandidateCache::default(),
        table,
        excavations,
        17,
        [lo, hi],
        |_, y, _| table.id_at([0.0, 0.3, 0.0, 0.0, 0.0, 0.5], y),
        |_| false,
    );
    assert!(rejected.is_empty());
    let accepted = ChamberField::gather(
        &CandidateCache::default(),
        table,
        excavations,
        17,
        [lo, hi],
        |_, y, _| table.id_at([0.0, 0.3, 0.0, 0.0, 0.0, 0.5], y),
        |_| true,
    );
    assert!(!accepted.is_empty());
    for p in accepted.centers() {
        let local = ChamberField::gather(
            &CandidateCache::default(),
            table,
            excavations,
            17,
            [p, p],
            |_, y, _| table.id_at([0.0, 0.3, 0.0, 0.0, 0.0, 0.5], y),
            |_| true,
        );
        assert_eq!(
            accepted.at(p[0], p[1], p[2], 0.0),
            local.at(p[0], p[1], p[2], 0.0)
        );
    }
}
