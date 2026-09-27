use super::{blended_fog_color, Biome, ENGINE_BIOME_COUNT, SKY_FOG_BLEND_SPAN_BLOCKS};

#[test]
fn mod_api_biome_vocabulary_matches_the_engine_table() {
    assert_eq!(
        mod_api::biome::BIOME_NAMES.len(),
        ENGINE_BIOME_COUNT,
        "append new biomes to mod_api::biome in the same change"
    );
    for id in 1..=ENGINE_BIOME_COUNT as u8 {
        let biome = Biome::from_id(id);
        assert_eq!(mod_api::biome::name(id), Some(biome.name()));
        assert_eq!(mod_api::biome::by_name(biome.name()), Some(id));
    }
}

#[test]
fn engine_consts_name_their_frozen_rows() {
    let consts = [
        (Biome::OCEAN, "ocean"),
        (Biome::BEACH, "beach"),
        (Biome::RIVER, "river"),
        (Biome::DESERT, "desert"),
        (Biome::PLAINS, "plains"),
        (Biome::SAVANNA, "savanna"),
        (Biome::FOREST, "forest"),
        (Biome::SWAMP, "swamp"),
        (Biome::TAIGA, "taiga"),
        (Biome::SNOWY_TUNDRA, "snowy_tundra"),
        (Biome::SNOWY_TAIGA, "snowy_taiga"),
        (Biome::MOUNTAINS, "mountains"),
        (Biome::SNOWY_PEAKS, "snowy_peaks"),
        (Biome::DEEP_OCEAN, "deep_ocean"),
        (Biome::FOOTHILLS, "foothills"),
        (Biome::WETLAND, "wetland"),
        (Biome::REDWOOD_FOREST, "redwood_forest"),
        (Biome::OLD_GROWTH_TAIGA, "old_growth_taiga"),
        (Biome::MEADOW, "meadow"),
        (Biome::GROVE, "grove"),
        (Biome::SNOWY_SLOPES, "snowy_slopes"),
        (Biome::WINDSWEPT_HILLS, "windswept_hills"),
        (Biome::STONY_PEAKS, "stony_peaks"),
        (Biome::WOODED_HILLS, "wooded_hills"),
        (Biome::MOUNTAIN_EDGE, "mountain_edge"),
        (Biome::DESERT_LAKES, "desert_lakes"),
        (Biome::SNOWY_PLAINS, "snowy_plains"),
    ];
    assert_eq!(consts.len(), ENGINE_BIOME_COUNT);
    for (i, (biome, name)) in consts.into_iter().enumerate() {
        assert_eq!(usize::from(biome.id()), i + 1, "{name}");
        assert_eq!(biome.name(), name);
        assert_eq!(biome.key(), format!("petramond:{name}"));
        assert_eq!(Biome::from_name(name), Some(biome));
        assert_eq!(Biome::from_name(biome.key()), Some(biome));
        assert_eq!(Biome::from_id(biome.id()), biome);
    }
}

#[test]
fn unregistered_ids_fall_back_to_ocean() {
    assert_eq!(Biome::from_id(0), Biome::OCEAN);
    assert_eq!(Biome::from_id(u8::MAX), Biome::OCEAN);
    assert_eq!(Biome::all().count(), super::count());
    assert!(Biome::from_name("mymod:nowhere").is_none());
}

fn assert_color_close(actual: [f32; 3], expected: [f32; 3]) {
    for i in 0..3 {
        assert!(
            (actual[i] - expected[i]).abs() < 1e-5,
            "channel {i}: got {}, expected {}",
            actual[i],
            expected[i]
        );
    }
}

#[test]
fn blended_fog_color_is_exact_in_uniform_biome_area() {
    assert_color_close(
        blended_fog_color(12.25, -4.75, |_, _| Biome::FOREST),
        Biome::FOREST.fog_color(),
    );
}

#[test]
fn blended_fog_color_uses_ten_block_border_window() {
    let boundary = |wx: i32, _wz: i32| {
        if wx < 0 {
            Biome::PLAINS
        } else {
            Biome::DESERT
        }
    };

    assert_color_close(
        blended_fog_color(-(SKY_FOG_BLEND_SPAN_BLOCKS as f64) - 1.0, 0.5, boundary),
        Biome::PLAINS.fog_color(),
    );
    assert_color_close(
        blended_fog_color(SKY_FOG_BLEND_SPAN_BLOCKS as f64 + 1.0, 0.5, boundary),
        Biome::DESERT.fog_color(),
    );

    let midpoint = blended_fog_color(0.0, 0.5, boundary);
    let plains = Biome::PLAINS.fog_color();
    let desert = Biome::DESERT.fog_color();
    assert_color_close(
        midpoint,
        [
            (plains[0] + desert[0]) * 0.5,
            (plains[1] + desert[1]) * 0.5,
            (plains[2] + desert[2]) * 0.5,
        ],
    );
}

#[test]
fn blended_fog_color_handles_multi_biome_intersections() {
    let quadrant = |wx: i32, wz: i32| match (wx >= 0, wz >= 0) {
        (false, false) => Biome::PLAINS,
        (true, false) => Biome::DESERT,
        (false, true) => Biome::SWAMP,
        (true, true) => Biome::SNOWY_TUNDRA,
    };
    let actual = blended_fog_color(0.0, 0.0, quadrant);
    let colors = [
        Biome::PLAINS.fog_color(),
        Biome::DESERT.fog_color(),
        Biome::SWAMP.fog_color(),
        Biome::SNOWY_TUNDRA.fog_color(),
    ];
    let expected = [
        colors.iter().map(|c| c[0]).sum::<f32>() * 0.25,
        colors.iter().map(|c| c[1]).sum::<f32>() * 0.25,
        colors.iter().map(|c| c[2]).sum::<f32>() * 0.25,
    ];

    assert_color_close(actual, expected);
}
