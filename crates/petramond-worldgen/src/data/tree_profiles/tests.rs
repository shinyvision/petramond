use super::*;
use crate::biome::trees::{RuleStage, Territory};

const OAKS: &str = r#"[{"feature": "petramond:oak_small", "weight": 1}]"#;
const BIRCH: &str = r#"[{"feature": "petramond:birch", "weight": 1}]"#;

fn profile_of(text: &str) -> Result<TreeProfile, String> {
    parse(Some(text))
}

#[test]
fn every_shipped_biome_parses_and_rooted_ones_name_species() {
    for id in 1..=BIOME_COUNT as u8 {
        let profile = profile(Biome::from_id(id));
        assert!(profile.validate().is_ok());
        assert_eq!(profile.density > 0.0, profile.species.is_some());
    }
}

#[test]
fn an_absent_field_is_the_treeless_default() {
    let profile = parse(None).unwrap();
    assert_eq!(profile.density, 0.0);
    assert!(profile.species.is_none() && profile.rules.is_empty());
}

#[test]
fn rules_resolve_their_territory_stage_and_species() {
    let profile = profile_of(&format!(
        r#"{{"density": 0.5, "species": {OAKS},
            "rules": [{{"when": {{"nearby_biome": {{"biome": "petramond:river", "radius": 3}}}},
                       "species": {BIRCH}}}]}}"#
    ))
    .unwrap();
    assert_eq!(profile.rules.len(), 1);
    assert_eq!(profile.rules[0].territory.stage(), RuleStage::Accepted);
    assert_eq!(profile.rules[0].species.len(), 1);
}

#[test]
fn rows_naming_the_same_grove_field_share_one_lattice() {
    let grove = |field: &str| {
        let profile = profile_of(&format!(
            r#"{{"density": 0.1, "species": {OAKS},
                "rules": [{{"when": {{"grove": {{"field": "{field}", "period": 48,
                           "transition": [0.4, 0.6], "chance": [0.0, 1.0]}}}},
                           "species": {BIRCH}}}]}}"#
        ))
        .unwrap();
        match profile.rules[0].territory {
            Territory::Grove(l) => l,
            Territory::NearbyBiome { .. } => panic!("a grove rule"),
        }
    };
    assert_eq!(grove("mymod:stands"), grove("mymod:stands"));
    assert_ne!(grove("mymod:stands").salt, grove("mymod:stand").salt);
    assert_eq!(
        grove("mymod:stands").detail_weight,
        DEFAULT_GROVE_DETAIL_WEIGHT
    );
}

#[test]
fn bad_rows_fail_naming_the_field() {
    let cases = [
        (
            String::from(
                r#"{"density": 0.1, "species": [{"feature": "mymod:palm", "weight": 1}]}"#,
            ),
            "unknown worldgen feature 'mymod:palm'",
        ),
        (
            format!(
                r#"{{"density": 0.1, "species": {OAKS},
                    "rules": [{{"when": {{"nearby_biome": {{"biome": "mymod:lagoon", "radius": 3}}}},
                               "species": {BIRCH}}}]}}"#
            ),
            "rules[0]: nearby_biome: unknown biome 'mymod:lagoon'",
        ),
        (
            format!(
                r#"{{"density": 0.1, "species": {OAKS},
                    "rules": [{{"when": {{"nearby_biome": {{"biome": "petramond:river", "radius": 3}}}},
                               "density": 0.4, "species": {BIRCH}}}]}}"#
            ),
            "rules[0]",
        ),
        (
            format!(r#"{{"density": 2.0, "species": {OAKS}}}"#),
            "density: 2 must lie within 0..=1",
        ),
        (
            format!(r#"{{"density": 0.1, "species": {OAKS}, "shade": 1}}"#),
            "shade",
        ),
        (String::from(r#"{"density": 0.1}"#), "species:"),
    ];
    for (text, expected) in cases {
        let err = profile_of(&text).expect_err(expected);
        assert!(
            err.starts_with("trees: ") && err.contains(expected),
            "{err}"
        );
    }
}
