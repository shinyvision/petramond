use super::*;

/// A pack may recolour an engine biome, but a new biome key is refused —
/// the id space is chunk-serialized and enum-closed.
#[test]
fn packs_may_override_colours_but_not_add_biomes() {
    let base = std::fs::read_to_string(
        crate::assets::candidate_paths("biomes.json")
            .into_iter()
            .find(|p| p.exists())
            .expect("shipped biomes.json"),
    )
    .unwrap();
    let recolour = r#"{"biomes": [{"biome": "petramond:forest",
            "fog_color": [0.1, 0.2, 0.3], "grass_color": [0.1, 0.2, 0.3],
            "foliage_color": [0.1, 0.2, 0.3], "water_color": [0.1, 0.2, 0.3]}]}"#;
    let table = parse_layers(&[&base, recolour]).expect("override loads");
    assert_eq!(table.rows().len(), ENGINE_BIOME_COUNT);
    let forest = &table.rows()[(Biome::Forest.id() - 1) as usize];
    assert_eq!(forest.grass_color, [0.1, 0.2, 0.3]);

    let addition = r#"{"biomes": [{"biome": "mymod:crystal_fields",
            "fog_color": [0.1, 0.2, 0.3], "grass_color": [0.1, 0.2, 0.3],
            "foliage_color": [0.1, 0.2, 0.3], "water_color": [0.1, 0.2, 0.3]}]}"#;
    let err = match parse_layers(&[&base, addition]) {
        Ok(_) => panic!("additions must be refused"),
        Err(e) => e,
    };
    assert!(err.contains("engine-defined"), "{err}");
}

/// The `trees` object rides the row untouched (worldgen parses it) and an
/// omitted one reads as absent, not as an empty profile text.
#[test]
fn the_trees_field_is_carried_verbatim_and_absent_when_omitted() {
    let base = std::fs::read_to_string(
        crate::assets::candidate_paths("biomes.json")
            .into_iter()
            .find(|p| p.exists())
            .expect("shipped biomes.json"),
    )
    .unwrap();
    let row = |trees: &str| {
        format!(
            r#"{{"biomes": [{{"biome": "petramond:desert",
                "fog_color": [0.1, 0.2, 0.3], "grass_color": [0.1, 0.2, 0.3],
                "foliage_color": [0.1, 0.2, 0.3], "water_color": [0.1, 0.2, 0.3]{trees}}}]}}"#
        )
    };
    let desert = |table: &crate::registry::Catalog<BiomeDef>| {
        table.rows()[(Biome::Desert.id() - 1) as usize].trees
    };
    let stated = parse_layers(&[
        &base,
        &row(r#", "trees": {"density": 0.5, "anything": [1]}"#),
    ])
    .expect("loads");
    let text = desert(&stated).expect("stated");
    let value: serde_json::Value = serde_json::from_str(text).unwrap();
    assert_eq!(value["density"], 0.5);
    assert_eq!(value["anything"], serde_json::json!([1]));
    assert!(desert(&parse_layers(&[&base, &row("")]).unwrap()).is_none());
}

/// The ambient map is validated per entry (namespaced key, density in
/// range) and a row that omits it drives nothing.
#[test]
fn ambient_densities_validate_per_entry() {
    let row = |ambient: &str| {
        format!(
            r#"{{"biomes": [{{"biome": "petramond:forest",
                "fog_color": [0.1, 0.2, 0.3], "grass_color": [0.1, 0.2, 0.3],
                "foliage_color": [0.1, 0.2, 0.3], "water_color": [0.1, 0.2, 0.3],
                "ambient": {ambient}}}]}}"#
        )
    };
    let base = std::fs::read_to_string(
        crate::assets::candidate_paths("biomes.json")
            .into_iter()
            .find(|p| p.exists())
            .expect("shipped biomes.json"),
    )
    .unwrap();
    let forest = |table: &crate::registry::Catalog<BiomeDef>| {
        table.rows()[(Biome::Forest.id() - 1) as usize].ambient
    };
    let ok = parse_layers(&[&base, &row(r#"{"mymod:fireflies": 0.3, "mymod:moths": 1}"#)])
        .expect("valid entries load");
    assert_eq!(
        forest(&ok),
        &[("mymod:fireflies", 0.3), ("mymod:moths", 1.0)]
    );
    assert!(forest(&parse_layers(&[&base, &row("{}")]).unwrap()).is_empty());
    for bad in [r#"{"fireflies": 0.3}"#, r#"{"mymod:fireflies": 1.5}"#] {
        let err = parse_layers(&[&base, &row(bad)]).err().expect("refused");
        assert!(err.contains("ambient"), "{err}");
    }
}
