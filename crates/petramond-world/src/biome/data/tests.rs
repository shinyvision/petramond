use super::*;

/// A pack may recolour an engine biome (no new id) and ADD a biome under
/// its own namespaced key, which registers the next id after the engine
/// range; a bare or unknown engine-namespace key is refused.
#[test]
fn packs_may_override_engine_rows_and_add_biomes() {
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
    let forest = &table.rows()[(Biome::FOREST.id() - 1) as usize];
    assert_eq!(forest.grass_color, [0.1, 0.2, 0.3]);
    assert_eq!((forest.key, forest.name), ("petramond:forest", "forest"));

    let addition = r#"{"biomes": [{"biome": "mymod:crystal_fields",
            "fog_color": [0.4, 0.5, 0.6], "grass_color": [0.1, 0.2, 0.3],
            "foliage_color": [0.1, 0.2, 0.3], "water_color": [0.1, 0.2, 0.3],
            "generation": {"surface": "petramond:stone"}}]}"#;
    let table = parse_layers(&[&base, addition]).expect("a namespaced addition loads");
    assert_eq!(table.rows().len(), ENGINE_BIOME_COUNT + 1);
    let index = table.id("mymod:crystal_fields").expect("registered");
    assert_eq!(usize::from(index), ENGINE_BIOME_COUNT, "after the engine range");
    let added = &table.rows()[usize::from(index)];
    assert_eq!(added.biome.id() as usize, ENGINE_BIOME_COUNT + 1);
    assert_eq!((added.key, added.name), ("mymod:crystal_fields", "mymod:crystal_fields"));
    assert_eq!(added.fog_color, [0.4, 0.5, 0.6]);
    assert!(added.generation.is_some());

    for bad in ["crystal_fields", "petramond:crystal_fields"] {
        let row = format!(
            r#"{{"biomes": [{{"biome": "{bad}",
                "fog_color": [0.1, 0.2, 0.3], "grass_color": [0.1, 0.2, 0.3],
                "foliage_color": [0.1, 0.2, 0.3], "water_color": [0.1, 0.2, 0.3]}}]}}"#
        );
        assert!(parse_layers(&[&base, &row]).is_err(), "accepted '{bad}'");
    }
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
        table.rows()[(Biome::DESERT.id() - 1) as usize].trees
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

/// Every shipped row states its `generation` rules, carried verbatim like
/// `trees` for worldgen to parse.
#[test]
fn every_shipped_row_carries_its_generation_rules() {
    let base = std::fs::read_to_string(
        crate::assets::candidate_paths("biomes.json")
            .into_iter()
            .find(|p| p.exists())
            .expect("shipped biomes.json"),
    )
    .unwrap();
    let table = parse_layers(&[&base]).expect("shipped biomes load");
    for row in table.rows() {
        let text = row
            .generation
            .unwrap_or_else(|| panic!("biome '{}' has no generation", row.name));
        let value: serde_json::Value = serde_json::from_str(text).unwrap();
        assert!(value.get("surface").is_some(), "biome '{}'", row.name);
    }
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
        table.rows()[(Biome::FOREST.id() - 1) as usize].ambient
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
