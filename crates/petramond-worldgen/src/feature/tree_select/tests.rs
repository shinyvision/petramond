use super::*;
use crate::biome::trees::{GroveLattice, SelectionRule, DEFAULT_GROVE_DETAIL_WEIGHT};
use crate::feature::stub_species;

fn species() -> SpeciesTable {
    SpeciesTable::new(&[(1, stub_species())]).unwrap()
}

fn nearby(radius: i32) -> SelectionRule {
    SelectionRule {
        territory: Territory::NearbyBiome {
            biome: Biome::River,
            radius,
        },
        species: species(),
        density: None,
        spacing_radius: None,
    }
}

fn grove(chance: f32) -> SelectionRule {
    SelectionRule {
        territory: Territory::Grove(GroveLattice {
            salt: 917,
            period: 96,
            detail_weight: DEFAULT_GROVE_DETAIL_WEIGHT,
            transition: (0.4, 0.6),
            chance: (chance, chance),
        }),
        species: species(),
        density: Some(0.37),
        spacing_radius: Some(3),
    }
}

fn profile(rules: Vec<SelectionRule>) -> TreeProfile {
    TreeProfile {
        density: 0.1,
        spacing_radius: 8,
        species: Some(species()),
        rules: rules.into_boxed_slice(),
        ..TreeProfile::default()
    }
}

fn same(a: &SpeciesTable, b: &SpeciesTable) -> bool {
    std::ptr::eq(a, b)
}

/// Flat plains with one River column at `target` (relative to `origin`), that
/// asserts every read stays within `radius` of the origin.
struct Habitat {
    origin: (i32, i32),
    target: Option<(i32, i32)>,
    radius: i32,
    reads: usize,
}

impl FeatureField for Habitat {
    fn column_at(&mut self, x: i32, z: i32) -> (i32, Biome) {
        let p = (x - self.origin.0, z - self.origin.1);
        assert!(
            p.0.abs().max(p.1.abs()) <= self.radius,
            "habitat read escaped its declared neighbourhood"
        );
        self.reads += 1;
        (
            70,
            if Some(p) == self.target {
                Biome::River
            } else {
                Biome::Plains
            },
        )
    }
}

#[test]
fn a_terrain_rule_is_local_translation_invariant_and_falls_back_to_the_base_table() {
    let radius = 7;
    let profile = profile(vec![nearby(radius)]);
    for origin in [(0, 0), (-17, 31), (100_000, -100_000)] {
        for target in [Some((radius, 0)), Some((-radius, -radius)), None] {
            let mut field = Habitat {
                origin,
                target,
                radius,
                reads: 0,
            };
            let mut candidates = TreeCandidates::new(1, origin.0, origin.1);
            let picked = candidates.accepted_species(&profile, &mut field, origin.0, origin.1);
            let expected = if target.is_some() {
                &profile.rules[0].species
            } else {
                profile.species.as_ref().unwrap()
            };
            assert!(same(picked, expected));
            assert!(field.reads > 0);
        }
    }
}

#[test]
fn terrain_rules_wait_for_acceptance_and_reserve_the_base_spacing_meanwhile() {
    let profile = profile(vec![nearby(7), grove(1.0)]);
    let mut field = Habitat {
        origin: (0, 0),
        target: Some((7, 0)),
        radius: 0,
        reads: 0,
    };
    let mut candidates = TreeCandidates::new(1, 0, 0);
    let at_candidate = candidates.select(&profile, RuleStage::Candidate, &mut field, 0, 0);
    assert_eq!(
        field.reads, 0,
        "candidate probes never read a neighbourhood"
    );
    assert_eq!(
        at_candidate.rule,
        Some(1),
        "the grove decides density and spacing"
    );
    assert_eq!(at_candidate.density, 0.37);
    assert!(profile.deferred_rule_may_win(at_candidate.rule));

    field.radius = 7;
    let accepted = candidates.select(&profile, RuleStage::Accepted, &mut field, 0, 0);
    assert_eq!(
        accepted.rule,
        Some(0),
        "at acceptance the earlier terrain rule wins"
    );
    assert!(same(accepted.species.unwrap(), &profile.rules[0].species));
}

#[test]
fn rule_order_is_the_only_precedence() {
    let grove_first = profile(vec![grove(1.0), nearby(7)]);
    let mut field = Habitat {
        origin: (0, 0),
        target: Some((7, 0)),
        radius: 7,
        reads: 0,
    };
    let mut candidates = TreeCandidates::new(1, 0, 0);
    let accepted = candidates.select(&grove_first, RuleStage::Accepted, &mut field, 0, 0);
    assert_eq!(accepted.rule, Some(0));
    assert_eq!(field.reads, 0, "a claiming earlier rule ends the walk");
    assert!(!grove_first.deferred_rule_may_win(Some(0)));
}

#[test]
fn a_grove_rule_carries_its_own_density_spacing_and_species_or_none_of_them() {
    let mut field = Habitat {
        origin: (0, 0),
        target: None,
        radius: 0,
        reads: 0,
    };
    for (chance, claimed) in [(1.0, true), (0.0, false)] {
        let profile = profile(vec![grove(chance)]);
        let mut candidates = TreeCandidates::new(871, 0, 0);
        for (x, z) in [(-20, 31), (0, -1), (18, 16), (41, 40)] {
            let selection = candidates.select(&profile, RuleStage::Candidate, &mut field, x, z);
            assert_eq!(selection.rule.is_some(), claimed);
            let (species, density, spacing) = if claimed {
                (&profile.rules[0].species, 0.37, 3)
            } else {
                (
                    profile.species.as_ref().unwrap(),
                    profile.density,
                    profile.spacing_radius,
                )
            };
            assert!(same(selection.species.unwrap(), species));
            assert_eq!(selection.density, density);
            assert_eq!(selection.spacing_radius, spacing);
        }
    }
    assert_eq!(field.reads, 0);
}

#[test]
fn profiles_without_rules_touch_neither_terrain_nor_lattices() {
    let mut field = Habitat {
        origin: (0, 0),
        target: None,
        radius: 0,
        reads: 0,
    };
    let profile = profile(vec![]);
    let mut candidates = TreeCandidates::new(1, 0, 0);
    let selection = candidates.select(&profile, RuleStage::Accepted, &mut field, 0, 0);
    assert!(selection.rule.is_none());
    assert_eq!(field.reads, 0);
    assert_eq!(candidates.groves.lattice_count(), 0);
}

#[test]
fn dense_small_trees_cannot_displace_large_crowns() {
    let large = TreeCandidate {
        anchor: 70,
        biome: Biome::Forest,
        density: 0.1,
        spacing_radius: 7,
        priority: 1,
    };
    let small = TreeCandidate {
        spacing_radius: 2,
        priority: u64::MAX,
        ..large
    };
    assert!(tree_candidate_beats(large, -16, 3, small, -15, 3));
    assert!(!tree_candidate_beats(small, -15, 3, large, -16, 3));
    let peer = TreeCandidate {
        priority: 2,
        ..large
    };
    assert!(tree_candidate_beats(peer, -15, 3, large, -16, 3));
    assert!(tree_candidate_beats(large, -16, 3, large, -15, 3));
    assert!(!tree_candidate_beats(large, -15, 3, large, -16, 3));
}

#[test]
fn the_candidate_window_replays_present_and_absent_sites_without_more_terrain_reads() {
    struct FlatForest(usize);
    impl FeatureField for FlatForest {
        fn column_at(&mut self, _: i32, _: i32) -> (i32, Biome) {
            self.0 += 1;
            (70, Biome::Forest)
        }
    }
    let (ox, oz) = (-48, 96);
    let mut field = FlatForest(0);
    let mut candidates = TreeCandidates::new(786, ox, oz);
    let window = candidates.window;
    let mut expected = Vec::new();
    for z in window.z_min..window.z_max {
        for x in window.x_min..window.x_max {
            expected.push((x, z, candidates.at(&mut field, x, z)));
        }
    }
    let reads = field.0;
    assert!(
        expected.iter().any(|(_, _, c)| c.is_some()),
        "a forest roots trees"
    );
    let mut reference = FlatForest(0);
    let mut replay = TreeCandidates::new(786, ox, oz);
    for (x, z, value) in expected.into_iter().rev() {
        assert_eq!(candidates.at(&mut field, x, z), value);
        assert_eq!(replay.at(&mut reference, x, z), value);
    }
    assert_eq!(field.0, reads);
}
