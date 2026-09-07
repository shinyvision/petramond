use super::*;
use crate::biome::trees::DEFAULT_GROVE_DETAIL_WEIGHT;

fn lattice() -> GroveLattice {
    GroveLattice {
        salt: 917,
        period: 96,
        detail_weight: DEFAULT_GROVE_DETAIL_WEIGHT,
        transition: (0.4, 0.6),
        chance: (0.0, 1.0),
    }
}

/// A 68-block window centred near `(x, z)`, as a placement pass would open.
fn window_around(x: i32, z: i32) -> Window {
    Window {
        x_min: x - 34,
        x_max: x + 34,
        z_min: z - 34,
        z_max: z + 34,
    }
}

fn chance_at(seed: u32, x: i32, z: i32) -> f32 {
    GroveField::new(seed, window_around(x, z)).chance(&lattice(), x, z)
}

#[test]
fn grove_answers_are_independent_of_memo_state_and_visit_order() {
    let centres = [(0, 0), (-97, 31), (511, -320), (-1_000_000, 2_000_000)];
    for seed in [7, 918, u32::MAX] {
        for (cx, cz) in centres {
            let window = window_around(cx, cz);
            let points: Vec<(i32, i32)> = (-33..34)
                .step_by(11)
                .flat_map(|dx| (-33..34).step_by(13).map(move |dz| (cx + dx, cz + dz)))
                .collect();
            let mut forward = GroveField::new(seed, window);
            let expected: Vec<(f32, bool)> = points
                .iter()
                .map(|&(x, z)| {
                    (
                        forward.chance(&lattice(), x, z),
                        forward.claims(&lattice(), x, z),
                    )
                })
                .collect();
            let mut reverse = GroveField::new(seed, window);
            for (i, &(x, z)) in points.iter().enumerate().rev() {
                assert_eq!(expected[i].1, reverse.claims(&lattice(), x, z));
                assert_eq!(expected[i].0, reverse.chance(&lattice(), x, z));
                assert_eq!(expected[i].0, chance_at(seed, x, z));
            }
        }
    }
}

#[test]
fn grove_field_is_continuous_at_positive_and_negative_lattice_edges() {
    let period = 97;
    for edge in [-194, -97, 0, 97, 194, 97_000_000] {
        for offset in [-31, 0, 27] {
            for (x, z) in [(edge, offset), (offset, edge)] {
                let mut field = GroveField::new(871, window_around(x, z));
                let centre = field.sample(711, period, x, z);
                for (dx, dz) in [(-1, 0), (1, 0), (0, -1), (0, 1)] {
                    let next = field.sample(711, period, x + dx, z + dz);
                    assert!((centre - next).abs() <= 1.5 / period as f32);
                }
            }
        }
    }
}

#[test]
fn neighbouring_sites_share_species_bias_and_distant_groves_can_differ() {
    for seed in [1, 7, 918] {
        let mut near = 0.0;
        let mut far = 0.0;
        let mut low = 1.0_f32;
        let mut high = 0.0_f32;
        for z in (-512..512).step_by(64) {
            for x in (-512..512).step_by(64) {
                let chance = chance_at(seed, x, z);
                low = low.min(chance);
                high = high.max(chance);
                near += (chance - chance_at(seed, x + 8, z)).abs();
                far += (chance - chance_at(seed, x + 384, z)).abs();
                assert!((0.0..=1.0).contains(&chance));
            }
        }
        assert!(
            low < 0.1 && high > 0.9,
            "missing distinct species territories"
        );
        assert!(
            near < far * 0.5,
            "species bias is scattered instead of clustered"
        );
    }
}

#[test]
fn one_lattice_pair_serves_every_site_of_a_pass() {
    let mut field = GroveField::new(3, window_around(40, -40));
    assert_eq!(field.lattice_count(), 0);
    for (x, z) in [(40, -40), (7, -73), (73, -7)] {
        field.claims(&lattice(), x, z);
    }
    assert_eq!(field.lattice_count(), 2, "one broad and one detail lattice");
}
