use super::{blended_fog_color, Biome, BIOME_COUNT, SKY_FOG_BLEND_SPAN_BLOCKS};

/// The mod-facing biome vocabulary (`mod_api::biome`) mirrors this
/// compiled table. Worldgen hooks hand mods raw biome ids; the ABI names
/// are their only sanctioned addressing, so a drifted or missing entry
/// must fail HERE, not in a mod at runtime.
#[test]
fn mod_api_biome_vocabulary_matches_the_engine_table() {
    assert_eq!(
        mod_api::biome::BIOME_NAMES.len(),
        BIOME_COUNT,
        "append new biomes to mod_api::biome in the same change"
    );
    for id in 1..=BIOME_COUNT as u8 {
        let biome = Biome::from_id(id);
        assert_eq!(mod_api::biome::name(id), Some(biome.name()));
        assert_eq!(mod_api::biome::by_name(biome.name()), Some(id));
    }
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
        blended_fog_color(12.25, -4.75, |_, _| Biome::Forest),
        Biome::Forest.fog_color(),
    );
}

#[test]
fn blended_fog_color_uses_ten_block_border_window() {
    let boundary = |wx: i32, _wz: i32| {
        if wx < 0 {
            Biome::Plains
        } else {
            Biome::Desert
        }
    };

    assert_color_close(
        blended_fog_color(-(SKY_FOG_BLEND_SPAN_BLOCKS as f64) - 1.0, 0.5, boundary),
        Biome::Plains.fog_color(),
    );
    assert_color_close(
        blended_fog_color(SKY_FOG_BLEND_SPAN_BLOCKS as f64 + 1.0, 0.5, boundary),
        Biome::Desert.fog_color(),
    );

    let midpoint = blended_fog_color(0.0, 0.5, boundary);
    let plains = Biome::Plains.fog_color();
    let desert = Biome::Desert.fog_color();
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
        (false, false) => Biome::Plains,
        (true, false) => Biome::Desert,
        (false, true) => Biome::Swamp,
        (true, true) => Biome::SnowyTundra,
    };
    let actual = blended_fog_color(0.0, 0.0, quadrant);
    let colors = [
        Biome::Plains.fog_color(),
        Biome::Desert.fog_color(),
        Biome::Swamp.fog_color(),
        Biome::SnowyTundra.fog_color(),
    ];
    let expected = [
        colors.iter().map(|c| c[0]).sum::<f32>() * 0.25,
        colors.iter().map(|c| c[1]).sum::<f32>() * 0.25,
        colors.iter().map(|c| c[2]).sum::<f32>() * 0.25,
    ];

    assert_color_close(actual, expected);
}
