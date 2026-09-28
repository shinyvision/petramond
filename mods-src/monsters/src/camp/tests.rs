use mod_sdk::build::{Derived, Families, Heights, Noise2, Site};

use super::*;

fn families() -> Families {
    let mut rows = Vec::new();
    for wood in ["oak", "spruce", "birch", "jungle", "acacia", "redwood"] {
        for (suffix, form) in [
            ("planks", "block"),
            ("log", "log"),
            ("slab", "slab"),
            ("stairs", "stairs"),
            ("fence", "fence"),
            ("door", "door"),
        ] {
            rows.push((
                format!("petramond:{wood}_{suffix}"),
                wood.to_string(),
                form.to_string(),
            ));
        }
    }
    for (family, block) in [
        ("cobblestone", "cobblestone"),
        ("stone", "stone"),
        ("stone_bricks", "stone_bricks"),
        ("marble", "marble"),
        ("polished_marble", "polished_marble"),
    ] {
        rows.push((
            format!("petramond:{block}"),
            family.to_string(),
            "block".to_string(),
        ));
        rows.push((
            format!("petramond:{family}_slab"),
            family.to_string(),
            "slab".to_string(),
        ));
        rows.push((
            format!("petramond:{family}_stairs"),
            family.to_string(),
            "stairs".to_string(),
        ));
    }
    Families::from_rows(rows)
}

/// Rolling land in one biome, well above sea level.
struct Land {
    biome: u8,
    amplitude: f32,
}

impl Terrain for Land {
    fn heights(&self, min: [i32; 2], max: [i32; 2]) -> Option<Heights> {
        let (a, b) = (Noise2(11), Noise2(29));
        Some(Heights::from_fn(min, max, |x, z| {
            80 + (a.at(x as f32 / 19.0, z as f32 / 19.0) * self.amplitude
                + b.at(x as f32 / 7.0, z as f32 / 7.0) * 0.8)
                .round() as i32
        }))
    }

    fn biomes(&self, columns: Vec<[i32; 2]>) -> Option<Vec<u8>> {
        Some(vec![self.biome; columns.len()])
    }

    fn fluid(&self, positions: Vec<[i32; 3]>) -> Option<Vec<bool>> {
        Some(vec![false; positions.len()])
    }
}

const SEA: i32 = 62;

fn camp_at<'a>(families: &'a Families, land: &Land, seed: u32) -> Option<(Box<Camp<'a>>, Plan)> {
    let site = Site {
        cell: [seed as i32, 3],
        center: [seed as i32 * 320 + 160, 900],
    };
    match Camp::survey(site.center, SEA, families, land, GRID.rng(seed, site)) {
        Survey::Camp(mut camp) => {
            let (plan, _) = camp.build();
            Some((camp, plan))
        }
        _ => None,
    }
}

#[test]
fn every_camp_style_builds_within_the_reach_its_sections_consult() {
    let families = families();
    for style in style::STYLE_BIOMES {
        let land = Land {
            biome: style,
            amplitude: 2.5,
        };
        for seed in 0..4 {
            let (camp, plan) = camp_at(&families, &land, seed * 7 + style as u32)
                .unwrap_or_else(|| panic!("biome {style} seed {seed} built nothing on open land"));
            let natural = |x: i32, z: i32| camp.natural.get(x, z).unwrap();
            let (lo, hi) = (
                GEN_FILTER.surface_offsets.unwrap()[0],
                GEN_FILTER.surface_offsets.unwrap()[1],
            );
            for ([x, y, z], _) in plan.cells() {
                assert!(
                    (x - camp.center[0]).abs() <= REACH && (z - camp.center[1]).abs() <= REACH,
                    "biome {style} seed {seed}: ({x}, {z}) past the site's reach"
                );
                assert!(
                    y >= natural(x, z) + lo && y <= natural(x, z) + hi,
                    "biome {style} seed {seed}: y {y} outside the dispatch band at ({x}, {z})"
                );
            }
            assert!(!camp.gates.is_empty() && camp.gates.len() <= 3);
            assert!(!camp.towers.is_empty());
        }
    }
}

#[test]
fn gates_keep_two_blocks_of_headroom() {
    let families = families();
    for seed in 0..24 {
        let land = Land {
            biome: style::STYLE_BIOMES[seed as usize % style::STYLE_BIOMES.len()],
            amplitude: 2.0,
        };
        let Some((camp, plan)) = camp_at(&families, &land, seed) else {
            continue;
        };
        for gate in &camp.gates {
            let c = camp.ring[gate.i];
            let floor = camp.g(c) + 1;
            for y in [floor, floor + 1] {
                let blocked = plan.get([c[0], y, c[1]]).is_some_and(|m| m.occupies());
                assert!(!blocked, "seed {seed}: gate at {c:?} blocked at y {y}");
            }
        }
    }
}

#[test]
fn a_site_derives_the_same_plan_every_time() {
    let families = families();
    let land = Land {
        biome: mod_sdk::biome::FOREST,
        amplitude: 3.0,
    };
    let site = Site {
        cell: [2, -5],
        center: [800, -1500],
    };
    let plan = |_| match derive(9, SEA, site, &families, &land, &mut |_| {}) {
        Derived::Plan(plan) => plan.encode(),
        _ => panic!("no camp"),
    };
    assert_eq!(plan(0), plan(1));
}

#[test]
fn water_and_cliffs_turn_a_site_away() {
    let families = families();
    let wet = Land {
        biome: mod_sdk::biome::OCEAN,
        amplitude: 1.0,
    };
    assert!(camp_at(&families, &wet, 1).is_none());
    let cliffs = Land {
        biome: mod_sdk::biome::PLAINS,
        amplitude: 40.0,
    };
    assert!(camp_at(&families, &cliffs, 1).is_none());
}
