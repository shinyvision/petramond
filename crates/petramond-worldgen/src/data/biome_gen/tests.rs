use super::*;
use crate::rng::FeatureRng;

#[test]
fn every_biome_has_its_rules_in_id_order() {
    for (i, spec) in specs().iter().enumerate() {
        assert_eq!(spec.biome.id() as usize, i + 1);
    }
    assert_eq!(specs().len(), petramond_world::biome::count());
}

#[test]
fn malformed_generation_rules_are_refused() {
    let biome = Biome::PLAINS;
    assert!(parse(biome, None).is_err());
    let with = |extra: &str| format!(r#"{{"surface": "petramond:stone"{extra}}}"#);
    assert!(parse(biome, Some(&with(""))).is_ok());
    for bad in [
        r#", "vegetation": {"sand_cover": {"chance": 0.5, "roll": [[50, "petramond:cactus"]]}}"#,
        r#", "vegetation": {"sand_cover": {"chance": 1.5, "roll": [[100, "petramond:cactus"]]}}"#,
        r#", "vegetation": {"flowers": {"palette": [], "coverage": 0.1, "density": 0.1}}"#,
        r#", "vegetation": {"moss": 1}"#,
        r#", "flags": ["floating"]"#,
        r#", "snow": "sometimes""#,
    ] {
        assert!(parse(biome, Some(&with(bad))).is_err(), "accepted {bad}");
    }
    let spec = parse(
        biome,
        Some(&with(
            r#", "snow": {"above_surface_y": 90}, "flags": ["wet", "no_beach"]"#,
        )),
    )
    .expect("valid rules load");
    assert_eq!(spec.snow_cover, SnowCover::AboveSurfaceY(90));
    assert!(spec.flags.wet && !spec.flags.beach_base && !spec.flags.ocean);
}

#[test]
fn surface_bands_past_the_skin_limit_are_refused() {
    let limit = crate::surface::MAX_SKIN_BAND_DEPTH;
    let rule = |depth: i32| {
        format!(
            r#"{{"surface": [{{"if": {{"depth_from_top": {depth}}}, "then": "petramond:dirt"}},
                "petramond:stone"]}}"#
        )
    };
    let at = parse(Biome::PLAINS, Some(&rule(limit))).expect("a band at the limit loads");
    assert_eq!(at.surface.deepest_band(), Some(limit as u32));
    assert!(parse(Biome::PLAINS, Some(&rule(limit + 1))).is_err());
    for spec in specs() {
        assert!(spec.surface.deepest_band().unwrap_or(0) <= limit as u32);
    }
}

#[test]
fn a_covers_entry_reproduces_the_compiled_mycelium_roll() {
    let spec = parse(
        Biome::PLAINS,
        Some(
            r#"{"surface": "petramond:mycelium", "vegetation": {"covers": [
                {"on": ["petramond:mycelium"], "roll": {"chance": 0.1, "roll": [
                    [55, "petramond:red_mushroom"], [100, "petramond:brown_mushroom"]]}}]}}"#,
        ),
    )
    .expect("a covers entry loads");
    let covers = spec.vegetation.covers;
    assert_eq!(covers.len(), 1);
    assert_eq!(covers[0].on, vec![Block::Mycelium]);
    assert!(!covers[0].clustered);
    let compiled = |rng: &mut FeatureRng| {
        if !rng.chance(0.10) {
            return None;
        }
        Some(if rng.next_i32(0, 99) < 55 {
            Block::RedMushroom
        } else {
            Block::BrownMushroom
        })
    };
    let mut planted = 0;
    for i in 0..4000 {
        let mut data = FeatureRng::positional(7, 0x5EED, i, 0, -i);
        let mut code = data;
        let picked = covers[0].roll.pick(&mut data);
        assert_eq!(picked, compiled(&mut code), "column {i}");
        assert_eq!(
            data.next_u64(),
            code.next_u64(),
            "column {i}: same draw count"
        );
        planted += usize::from(picked.is_some());
    }
    assert!(planted > 0);

    let empty_on = r#"{"surface": "petramond:stone", "vegetation": {"covers": [
        {"on": [], "roll": {"chance": 0.1, "roll": [[100, "petramond:red_mushroom"]]}}]}}"#;
    assert!(parse(Biome::PLAINS, Some(empty_on)).is_err());
}
