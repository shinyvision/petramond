use super::*;
use crate::feature::stub_species;
use petramond_world::biome::Biome;

fn table(weights: &[u32]) -> SpeciesTable {
    let weighted: Vec<_> = weights.iter().map(|&w| (w, stub_species())).collect();
    SpeciesTable::new(&weighted).unwrap()
}

fn lattice() -> GroveLattice {
    GroveLattice {
        salt: 9,
        period: 96,
        detail_weight: DEFAULT_GROVE_DETAIL_WEIGHT,
        transition: (0.4, 0.6),
        chance: (0.0, 1.0),
    }
}

fn rule(territory: Territory, density: Option<f32>, spacing_radius: Option<i32>) -> SelectionRule {
    SelectionRule {
        territory,
        species: table(&[1]),
        density,
        spacing_radius,
    }
}

fn wooded(rules: Vec<SelectionRule>) -> TreeProfile {
    TreeProfile {
        density: 0.01,
        spacing_radius: 6,
        species: Some(table(&[1])),
        rules: rules.into_boxed_slice(),
        ..TreeProfile::default()
    }
}

#[test]
fn weighted_species_split_one_roll_by_cumulative_weight() {
    let species = table(&[11, 30, 59]);
    let mut counts = [0usize; 3];
    let mut rng = FeatureRng::from_state(77);
    for _ in 0..20_000 {
        let picked = species.pick(&mut rng);
        let i = (0..3)
            .find(|&i| std::ptr::eq(species.entries[i].1, picked))
            .expect("a table entry");
        counts[i] += 1;
    }
    let shares = counts.map(|c| c as f32 / 20_000.0);
    for (share, weight) in shares.iter().zip([0.11, 0.30, 0.59]) {
        assert!((share - weight).abs() < 0.02, "shares {shares:?}");
    }
}

#[test]
fn a_single_species_draws_nothing_and_a_mixed_table_draws_exactly_once() {
    let mut rng = FeatureRng::from_state(5);
    let untouched = rng;
    table(&[7]).pick(&mut rng);
    assert_eq!(rng.next_u64(), { untouched }.next_u64());

    let mut rng = FeatureRng::from_state(5);
    let mut reference = rng;
    table(&[1, 1]).pick(&mut rng);
    reference.next_u64();
    assert_eq!(rng.next_u64(), reference.next_u64());
}

#[test]
fn species_tables_refuse_empty_and_zero_weight_rows() {
    assert!(SpeciesTable::new(&[]).is_err());
    assert!(SpeciesTable::new(&[(0, stub_species())]).is_err());
}

#[test]
fn peak_density_covers_every_candidate_stage_rule() {
    let profile = wooded(vec![
        rule(Territory::Grove(lattice()), Some(0.2), Some(1)),
        rule(Territory::Grove(lattice()), None, None),
    ]);
    assert_eq!(profile.peak_density(), 0.2);
    assert_eq!(wooded(vec![]).peak_density(), 0.01);
}

#[test]
fn a_deferred_rule_reserves_spacing_only_while_it_can_still_win() {
    let nearby = Territory::NearbyBiome {
        biome: Biome::River,
        radius: 4,
    };
    let grove = Territory::Grove(lattice());
    let deferred_first = wooded(vec![
        rule(nearby, None, None),
        rule(grove, Some(0.5), Some(1)),
    ]);
    assert!(deferred_first.deferred_rule_may_win(None));
    assert!(deferred_first.deferred_rule_may_win(Some(1)));
    let grove_first = wooded(vec![
        rule(grove, Some(0.5), Some(1)),
        rule(nearby, None, None),
    ]);
    assert!(grove_first.deferred_rule_may_win(None));
    assert!(!grove_first.deferred_rule_may_win(Some(0)));
    assert!(!wooded(vec![]).deferred_rule_may_win(None));
}

#[test]
fn validation_names_the_field_that_breaks_a_bound() {
    let nearby = Territory::NearbyBiome {
        biome: Biome::River,
        radius: 4,
    };
    let err = wooded(vec![rule(nearby, Some(0.3), None)])
        .validate()
        .unwrap_err();
    assert!(
        err.starts_with("rules[0]:") && err.contains("species only"),
        "{err}"
    );

    let mut flat = lattice();
    flat.transition = (0.5, 0.5);
    let err = wooded(vec![rule(Territory::Grove(flat), None, None)])
        .validate()
        .unwrap_err();
    assert!(err.contains("grove.transition"), "{err}");

    let mut wide = wooded(vec![]);
    wide.spacing_radius = MAX_TREE_SPACING_RADIUS + 1;
    let err = wide.validate().unwrap_err();
    assert!(err.starts_with("spacing_radius:"), "{err}");

    let rooted_nothing = TreeProfile {
        density: 0.5,
        ..TreeProfile::default()
    };
    assert!(rooted_nothing
        .validate()
        .unwrap_err()
        .starts_with("species:"));
    assert!(TreeProfile::default().validate().is_ok());
}
