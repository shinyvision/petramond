mod_sdk::pack_keys! {
    pub WIND_PARAM: ShaderParam = "weather:wind";
    pub SKY_PARAM: ShaderParam = "weather:sky";
    pub FLUX_PARAM: ShaderParam = "weather:flux";

    pub RAIN_BUNDLE: Emitter = "weather:rain";
    pub SNOW_BUNDLE: Emitter = "weather:snow";
    pub RAIN_LOOP: Sound = "weather:rain_loop";

    pub LEAF_TAG: Tag = "petramond:leaves";

    pub SNOW_LAYER: Block = "petramond:snow_layer";
    pub ICE: Block = "petramond:ice";
    pub PACKED_ICE: Block = "petramond:packed_ice";
    pub WATER: Block = "petramond:water";
}

#[cfg(test)]
mod tests {
    use mod_sdk::biome;
    use mod_sdk::json::Value;

    use crate::SNOWY_BIOMES;

    #[test]
    fn every_declared_pack_id_is_shipped() {
        pack_check::assert_declared(&[super::PACK_KEYS]);
    }

    fn snowy_names() -> Vec<&'static str> {
        let mut names: Vec<_> = SNOWY_BIOMES
            .iter()
            .map(|&id| biome::name(id).expect("an engine biome"))
            .collect();
        names.sort_unstable();
        names
    }

    fn emitter_biomes(emitters: &Value, emitter: &str, field: &str) -> Vec<String> {
        let row = emitters
            .get("emitters")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .find(|row| row.get("emitter").and_then(Value::as_str) == Some(emitter))
            .unwrap_or_else(|| panic!("{emitter} has a row"));
        let mut names: Vec<String> = row
            .get("ambient")
            .and_then(|ambient| ambient.get(field))
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("{emitter} lists {field}"))
            .iter()
            .filter_map(|name| name.as_str().map(str::to_owned))
            .collect();
        names.sort_unstable();
        names
    }

    #[test]
    fn the_emitter_rows_filter_by_the_same_snowy_biomes() {
        let emitters = Value::parse(include_str!("../pack/particle_emitters.json"))
            .expect("particle_emitters.json parses");
        assert_eq!(
            emitter_biomes(&emitters, super::SNOW_BUNDLE, "biomes"),
            snowy_names()
        );
        assert_eq!(
            emitter_biomes(&emitters, super::RAIN_BUNDLE, "exclude_biomes"),
            snowy_names()
        );
    }

    #[test]
    fn every_snowy_biome_is_snow_covered_by_the_engine() {
        let biomes = Value::parse(include_str!("../../../assets/biomes.json"))
            .expect("assets/biomes.json parses");
        let covered: Vec<&str> = biomes
            .get("biomes")
            .and_then(Value::as_array)
            .expect("a biomes array")
            .iter()
            .filter(|row| {
                row.get("generation")
                    .and_then(|g| g.get("snow"))
                    .and_then(Value::as_str)
                    == Some("always")
            })
            .filter_map(|row| row.get("biome").and_then(Value::as_str))
            .filter_map(|key| key.strip_prefix("petramond:"))
            .collect();
        for name in snowy_names() {
            assert!(
                covered.contains(&name),
                "{name} is snowy to the weather mod but not snow-covered in assets/biomes.json"
            );
        }
    }
}
