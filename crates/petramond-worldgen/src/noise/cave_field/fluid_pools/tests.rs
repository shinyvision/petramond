use super::*;
use crate::data::underground;

/// A synthetic pool row: certain at every cell, so a small sweep always
/// holds pools whatever the shipped rows are tuned to.
fn pool_row(key: &str, fluid: &str) -> String {
    format!(
        r#"{{"fluid_pool":"{key}","fluid":"{fluid}","anchor_y":-40,"chance":1.0,
        "height_scale":1000.0,"max_y":0,"reach":16,"max_depth":10,"max_drop":24,
        "max_sink":16,"budget":2000,"surface_clearance":32}}"#
    )
}

fn field_with_pools(seed: u32, rows: &[(&str, &str)]) -> CaveField {
    let rows: Vec<String> = rows.iter().map(|(k, f)| pool_row(k, f)).collect();
    let layer = format!(r#"{{"fluid_pools":[{}]}}"#, rows.join(","));
    CaveField::with_table(seed, underground::synthetic_table(&[&layer]))
}

const TOP: i32 = 0;

/// A pool belongs to its lattice cell, never to the box that asked: two boxes
/// sharing a cell answer the same fluid for it, or the section carve and the
/// whole-column carve would disagree wherever a pool crosses a batch edge.
#[test]
fn overlapping_boxes_answer_the_same_fluid() {
    let field = field_with_pools(0x1A7A_7001, &[("test:lava", "petramond:lava")]);
    let mut filled = 0;
    for (cx, cz) in [(-1, -1), (0, -1), (-1, 0), (0, 0)] {
        let wide = Pools::gather(
            &field,
            [cx * 16 - 16, CAVE_MIN_Y, cz * 16 - 16],
            [cx * 16 + 31, TOP, cz * 16 + 31],
        );
        let narrow = Pools::gather(
            &field,
            [cx * 16, CAVE_MIN_Y, cz * 16],
            [cx * 16 + 15, TOP, cz * 16 + 15],
        );
        for y in CAVE_MIN_Y..=TOP {
            for z in cz * 16..cz * 16 + 16 {
                for x in cx * 16..cx * 16 + 16 {
                    let fluid = wide.fluid_at([x, y, z]);
                    assert_eq!(fluid, narrow.fluid_at([x, y, z]), "at {x},{y},{z}");
                    filled += usize::from(fluid != Block::Air.id());
                }
            }
        }
    }
    assert!(filled > 0, "the swept boxes hold no pool");
}

/// The property the design rests on: every filled cell's lateral and lower
/// neighbours are filled too or rock the cave left standing, so nothing is
/// added to seal a pool and none pours when the world loads it. Surfaces
/// stand at more than one height: the level is the hollow's, not a plane's.
#[test]
fn a_pool_is_closed_sideways_and_downward_by_the_cave_alone() {
    let field = field_with_pools(0x1A7A_7002, &[("test:lava", "petramond:lava")]);
    let (lo, hi) = ([-48, CAVE_MIN_Y - 1, -48], [47, TOP + 1, 47]);
    let pools = Pools::gather(&field, lo, hi);
    let lat = field.build_lattice_filtered(lo[0], lo[1], lo[2], hi[0], hi[1], hi[2], FLOOD_FIELDS);
    let air = Block::Air.id();
    let mut surfaces = std::collections::BTreeSet::new();
    for y in CAVE_MIN_Y..=TOP {
        for z in -47..=46 {
            for x in -47..=46 {
                if pools.fluid_at([x, y, z]) == air {
                    continue;
                }
                if pools.fluid_at([x, y + 1, z]) == air {
                    surfaces.insert(y);
                }
                for [dx, dy, dz] in [[-1, 0, 0], [1, 0, 0], [0, 0, -1], [0, 0, 1], [0, -1, 0]] {
                    let next = [x + dx, y + dy, z + dz];
                    if pools.fluid_at(next) != air {
                        continue;
                    }
                    assert_ne!(
                        probe(
                            &field,
                            &Pools::default(),
                            &mut Col::new(&lat, next[0], next[2]),
                            next[1]
                        ),
                        Probe::Open,
                        "fluid at {x},{y},{z} pours into {next:?}"
                    );
                }
            }
        }
    }
    assert!(surfaces.len() > 1, "pool surfaces only at {surfaces:?}");
}

/// Two rows over the same caves: the earlier row claims first, and a later
/// row's pool never touches it — two generated fluids never meet at load.
#[test]
fn a_later_row_never_touches_an_earlier_rows_pool() {
    let field = field_with_pools(
        0x1A7A_7003,
        &[
            ("test:lava", "petramond:lava"),
            ("test:water", "petramond:water"),
        ],
    );
    let (lo, hi) = ([-48, CAVE_MIN_Y, -48], [47, TOP, 47]);
    let pools = Pools::gather(&field, lo, hi);
    let (air, lava) = (Block::Air.id(), Block::Lava.id());
    let mut first = 0usize;
    for y in CAVE_MIN_Y..=TOP {
        for z in -47..=46 {
            for x in -47..=46 {
                let here = pools.fluid_at([x, y, z]);
                if here == air {
                    continue;
                }
                first += usize::from(here == lava);
                for [dx, dy, dz] in [
                    [-1, 0, 0],
                    [1, 0, 0],
                    [0, 0, -1],
                    [0, 0, 1],
                    [0, -1, 0],
                    [0, 1, 0],
                ] {
                    let next = pools.fluid_at([x + dx, y + dy, z + dz]);
                    assert!(
                        next == air || next == here,
                        "{here} at {x},{y},{z} touches {next}"
                    );
                }
            }
        }
    }
    assert!(first > 0, "no pool of the first row");
}

#[test]
fn zz_dip_census() {
    let layer = r#"{"fluid_pools":[]}"#;
    for seed in [0x2au32, 786, 0x1A7A_F0FA, 0x1234_5678, 0xDEAD_BEEF] {
        let field = CaveField::with_table(seed, underground::synthetic_table(&[layer]));
        let (mut worst, mut worst_at, mut samples, mut below_zero) = (i32::MIN, [0, 0], 0, 0);
        let mut hist = std::collections::BTreeMap::new();
        for gz in -40..40 {
            for gx in -40..40 {
                let origin = [gx * 64, 0, gz * 64];
                let roof = roof(&field, origin, POOL_CELL + 16, 0);
                let mut surf = i32::MAX;
                for z in origin[2] - 17..=origin[2] + 32 {
                    for x in origin[0] - 17..=origin[0] + 32 {
                        surf = surf.min(field.density_surface(x, z));
                    }
                }
                samples += 1;
                below_zero += usize::from(surf < 0);
                let dip = roof - surf;
                *hist.entry((dip / 8) * 8).or_insert(0usize) += 1;
                if dip > worst {
                    worst = dip;
                    worst_at = [origin[0], origin[2]];
                }
            }
        }
        eprintln!("DIP seed {seed:#x} squares {samples} worst {worst} at {worst_at:?} surf<0 {below_zero} hist {hist:?}");
    }
}
