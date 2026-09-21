use super::*;

fn base() -> String {
    let (text, _) = crate::assets::read_base_text("particle_emitters.json")
        .expect("assets/particle_emitters.json must ship");
    text
}

/// The shipped catalog must load fully — the startup gate as a test.
#[test]
fn shipped_particle_emitters_json_loads_fully() {
    let defs = parse_layers(&[&base()])
        .unwrap_or_else(|e| panic!("shipped catalog: {e}"))
        .rows();
    assert_eq!(defs.len(), ENGINE_EMITTER_NAMES.len());
    for (i, d) in defs.iter().enumerate() {
        assert_eq!(d.id, i as u8);
        assert_eq!(d.key, ENGINE_EMITTER_NAMES[i]);
        let kinds = usize::from(!d.rows.is_empty())
            + usize::from(d.burst.is_some())
            + usize::from(d.ambient.is_some());
        assert_eq!(kinds, 1, "every bundle is exactly one kind");
    }
}

#[test]
fn burst_bundles_validate() {
    // (count, up_speed, bias) — the fields the bad cases vary.
    let splash = |count: &str, up: &str, bias: &str| {
        format!(
            r#"{{"emitters": [{{"emitter": "mymod:pop", "burst": {{
                    "count_per_intensity": {count},
                    "up_speed": {up}, "radial_speed": [0.5, 1.5],
                    "lifetime": [0.4, 0.8], "size": [0.05, 0.1],
                    "color": [[0.1, 0.1, 0.5], [0.4, 0.8, 1.0]],
                    "color_bias": {bias}, "die_on_contact": true }} }}]}}"#
        )
    };
    let ok = splash("3.0", "[1.0, 2.0]", "2.0");
    let defs = parse_layers(&[&base(), ok.as_str()])
        .expect("burst bundle loads")
        .rows();
    let d = defs.last().unwrap();
    assert!(d.burst.is_some() && d.rows.is_empty());

    for (bad, why) in [
        (splash("0.0", "[1.0, 2.0]", "2.0"), "zero count scaling"),
        (splash("3.0", "[2.0, 1.0]", "2.0"), "reversed range"),
        (splash("3.0", "[1.0, 2.0]", "100.0"), "out-of-range bias"),
        (
            r#"{"emitters": [{"emitter": "mymod:pop", "burst": {
                    "count_per_intensity": 3.0,
                    "up_speed": [1.0, 2.0], "radial_speed": [0.5, 1.5],
                    "lifetime": [0.4, 0.8], "size": [0.05, 0.1],
                    "color": [[0.1, 0.1, 0.5], [0.4, 0.8, 1.0]] },
                    "particles": [{"rate": 2.0, "lifetime": [0.4, 0.8], "size": [0.05, 0.1],
                        "color": [[1, 1, 1], [1, 1, 1]], "alpha": [0.5, 0.8]}] }]}"#
                .to_owned(),
            "both burst and particles",
        ),
    ] {
        assert!(
            parse_layers(&[&base(), bad.as_str()]).is_err(),
            "{why} must fail the load"
        );
    }
}

#[test]
fn pack_bundles_register_after_engine_rows_and_validate() {
    let glow = r#"{"emitter": "mymod:glow", "tint": [1.0, 0.9, 0.6], "body_self_lit": 0.4, "particles": [
            {"rate": 2.0, "lifetime": [0.4, 0.8], "size": [0.05, 0.1],
             "color": [[0.9, 0.9, 0.2], [1.0, 1.0, 0.6]], "alpha": [0.5, 0.8]}]}"#;
    let pack = format!(r#"{{"emitters": [{glow}]}}"#);
    let defs = parse_layers(&[&base(), pack.as_str()])
        .expect("pack bundle loads")
        .rows();
    let d = defs.last().unwrap();
    assert_eq!(d.key, "mymod:glow");
    assert_eq!(d.tint, Some([1.0, 0.9, 0.6]));
    assert_eq!(d.body_self_lit, 0.4);
    assert_eq!(d.rows.len(), 1);

    for (bad, why) in [
        (
            glow.replace("\"body_self_lit\": 0.4", "\"body_self_lit\": 1.1"),
            "out-of-range body lighting",
        ),
        (
            r#"{"emitter": "mymod:glow", "particles": []}"#.to_owned(),
            "no particle rows",
        ),
        (
            r#"{"emitter": "mymod:glow", "tint": [2.0, 0.0, 0.0], "particles": [
                    {"rate": 2.0, "lifetime": [0.4, 0.8], "size": [0.05, 0.1],
                     "color": [[1, 1, 1], [1, 1, 1]], "alpha": [0.5, 0.8]}]}"#
                .to_owned(),
            "out-of-range tint",
        ),
        (
            r#"{"emitter": "mymod:glow", "particles": [
                    {"rate": 2.0, "lifetime": [0.8, 0.4], "size": [0.05, 0.1],
                     "color": [[1, 1, 1], [1, 1, 1]], "alpha": [0.5, 0.8]}]}"#
                .to_owned(),
            "a row failing shared emitter validation",
        ),
        (
            r#"{"emitter": "bareglow", "particles": [
                    {"rate": 2.0, "lifetime": [0.4, 0.8], "size": [0.05, 0.1],
                     "color": [[1, 1, 1], [1, 1, 1]], "alpha": [0.5, 0.8]}]}"#
                .to_owned(),
            "a bare (un-namespaced) new key",
        ),
    ] {
        let pack = format!(r#"{{"emitters": [{bad}]}}"#);
        assert!(
            parse_layers(&[&base(), pack.as_str()]).is_err(),
            "{why} must fail the load"
        );
    }
}

#[test]
fn ambient_biome_filters_resolve_and_validate() {
    let with_filter = |extra: &str| {
        format!(
            r#"{{"emitters": [{{"emitter": "mymod:ashfall", "ambient": {{
                    "count_per_intensity": 100, "max_count": 200, "radius": 16,
                    "height": [4, 16], "fall_speed": [2, 4],
                    "size": [0.05, 0.08], "alpha": [0.5, 0.8],
                    "color": [[0.3, 0.3, 0.3], [0.5, 0.5, 0.5]]{extra} }} }}]}}"#
        )
    };
    // Allow-list resolves to a set admitting exactly its members.
    let ok = with_filter(r#", "biomes": ["snowy_plains", "snowy_taiga"]"#);
    let defs = parse_layers(&[&base(), ok.as_str()]).expect("filter loads");
    let allow = &defs
        .rows()
        .last()
        .unwrap()
        .ambient
        .as_ref()
        .unwrap()
        .biome_allow;
    assert!(allow.is_some());
    assert!(biome_allowed(allow, mod_api::biome::SNOWY_PLAINS));
    assert!(!biome_allowed(allow, mod_api::biome::PLAINS));
    // Exclusion admits the complement.
    let ok = with_filter(r#", "exclude_biomes": ["desert"]"#);
    let defs = parse_layers(&[&base(), ok.as_str()]).expect("exclusion loads");
    let allow = &defs
        .rows()
        .last()
        .unwrap()
        .ambient
        .as_ref()
        .unwrap()
        .biome_allow;
    assert!(!biome_allowed(allow, mod_api::biome::DESERT));
    assert!(biome_allowed(allow, mod_api::biome::PLAINS));
    // No filter = all biomes.
    let defs = parse_layers(&[&base(), with_filter("").as_str()]).expect("no filter");
    assert!(defs
        .rows()
        .last()
        .unwrap()
        .ambient
        .as_ref()
        .unwrap()
        .biome_allow
        .is_none());
    // Unknown names and double declarations fail the load.
    for (bad, why) in [
        (
            with_filter(r#", "biomes": ["nope_biome"]"#),
            "unknown biome",
        ),
        (
            with_filter(r#", "biomes": ["desert"], "exclude_biomes": ["plains"]"#),
            "both list kinds",
        ),
    ] {
        assert!(
            parse_layers(&[&base(), bad.as_str()]).is_err(),
            "{why} must fail the load"
        );
    }
}

/// A flight row is validated as a flight: the falling-kind fields are
/// refused, its names resolve at load, and a palette is per-entry checked.
#[test]
fn flight_rows_validate_and_resolve_their_names() {
    let row = |extra: &str, orbit: &str, occupancy: &str, names: &str| {
        format!(
            r#"{{"emitters": [{{"emitter": "mymod:moth", "ambient": {{
                    "radius": 20, "height": [6, 6], "size": [0.2, 0.3],
                    "alpha": [1, 1], "color": [[1, 1, 1], [1, 1, 1]], {extra}
                    "motion": {{"flight": {{"spacing": 6, "occupancy": {occupancy},
                        "orbit": {orbit}, "hover": [1, 0.3], "speed": [0.3, 0.6]{names} }} }} }} }}]}}"#
        )
    };
    let ok = parse_layers(&[
        &base(),
        &row(
            "",
            "[1, 1]",
            "0.5",
            r#", "sprite": "dirt", "ground_tags": ["soil", "mymod:ash"]"#,
        ),
    ])
    .expect("a flight row loads");
    let moth = ok.rows()[ok.id("mymod:moth").unwrap() as usize]
        .ambient
        .as_ref()
        .unwrap();
    let flight = moth.motion.flight().expect("flight kind");
    assert_eq!(flight.sprite_tile, Tile::from_name("dirt"));
    assert_eq!(flight.ground.len(), 2);
    assert!(
        flight.ground.contains(&BlockTag::SOIL),
        "engine tag resolved"
    );
    let bad = [
        (
            r#""fall_speed": [1, 2],"#,
            "[1, 1]",
            "0.5",
            "",
            "never falls",
        ),
        (
            r#""count_per_intensity": 10, "max_count": 10,"#,
            "[1, 1]",
            "0.5",
            "",
            "never falls",
        ),
        (
            "",
            "[1, 1]",
            "0.5",
            r#", "sprite": "no_such_tile""#,
            "no tile",
        ),
        ("", "[4, 4]", "0.5", "", "inside half the spacing"),
        ("", "[1, 1]", "0", "", "occupancy"),
        (
            r#""palette": [{"weight": 0, "color": [1, 1, 1]}],"#,
            "[1, 1]",
            "0.5",
            "",
            "palette weights",
        ),
        (
            r#""palette": [{"weight": 1, "color": [2, 1, 1]}],"#,
            "[1, 1]",
            "0.5",
            "",
            "palette color",
        ),
    ];
    for (extra, orbit, occupancy, names, expect) in bad {
        let err = parse_layers(&[&base(), &row(extra, orbit, occupancy, names)])
            .err()
            .expect(expect);
        assert!(err.contains(expect), "{err}");
    }
}

/// The biome-density table: a bundle some biome names is driven and reads
/// 0 in every biome that omits it; a bundle no biome names reads 1
/// everywhere; unknown or non-ambient keys are refused.
#[test]
fn biome_density_table_covers_driven_bundles_only() {
    let catalog = parse_layers(&[&base()]).unwrap();
    let torch = catalog.id("petramond:torch_flame").unwrap() as u8;
    let butterfly = catalog.id("petramond:butterfly").unwrap() as u8;
    let rows: [(u8, &[(&str, f32)]); 2] = [
        (3, &[("petramond:butterfly", 0.25)]),
        (9, &[("petramond:butterfly", 1.0)]),
    ];
    let table = BiomeTable::build(&catalog, rows.iter().map(|&(b, e)| (b, e))).unwrap();
    assert_eq!(&*table.driven, &[butterfly]);
    assert_eq!(table.intensity(butterfly, 3), 0.25);
    assert_eq!(table.intensity(butterfly, 9), 1.0);
    assert_eq!(table.intensity(butterfly, 4), 0.0, "omitted biome = absent");
    assert_eq!(
        table.intensity(torch, 3),
        1.0,
        "undriven bundles are not thinned"
    );
    assert_eq!(
        table.intensity(200, 3),
        1.0,
        "unknown ids read as untouched"
    );
    let unknown: [(u8, &[(&str, f32)]); 1] = [(3, &[("mymod:nothing", 0.5)])];
    let err = BiomeTable::build(&catalog, unknown.iter().map(|&(b, e)| (b, e)))
        .err()
        .unwrap();
    assert!(err.contains("unknown"), "{err}");
    let burst = catalog
        .rows()
        .iter()
        .find(|b| b.burst.is_some())
        .expect("a burst bundle ships")
        .key;
    let not_ambient: [(u8, &[(&str, f32)]); 1] = [(3, &[(burst, 0.5)])];
    let err = BiomeTable::build(&catalog, not_ambient.iter().map(|&(b, e)| (b, e)))
        .err()
        .unwrap();
    assert!(err.contains("not an ambient"), "{err}");
}

#[test]
fn missing_engine_row_is_a_load_error() {
    assert!(parse_layers(&[r#"{"emitters": []}"#]).is_err());
}

#[test]
fn ambient_bundles_validate_and_resolve_hit_bursts() {
    let ambient = |hit: &str, radius: &str| {
        format!(
            r#"{{"emitters": [{{"emitter": "mymod:rainfall", "ambient": {{
                    "count_per_intensity": 600, "max_count": 1500, "radius": {radius},
                    "height": [4, 20], "fall_speed": [16, 22], "drift_wind": 1.0,
                    "size": [0.03, 0.05], "stretch": 6.0, "alpha": [0.4, 0.7],
                    "color": [[0.55, 0.62, 0.75], [0.7, 0.78, 0.9]],
                    "hit": {hit} }} }}]}}"#
        )
    };

    // A valid ambient whose hit references the ENGINE water-splash burst.
    let ok = ambient(r#"{"burst": "petramond:water_splash"}"#, "24");
    let defs = parse_layers(&[&base(), ok.as_str()])
        .expect("ambient bundle loads")
        .rows();
    let d = defs.last().unwrap();
    let spec = d.ambient.as_ref().expect("ambient kind");
    assert!(matches!(&spec.hit, AmbientHit::Burst(k) if k == "petramond:water_splash"));
    assert!(d.rows.is_empty() && d.burst.is_none());

    // "die" is the default hit.
    let quiet = ambient(r#""die""#, "24");
    let defs = parse_layers(&[&base(), quiet.as_str()]).expect("die hit loads");
    assert_eq!(
        defs.rows().last().unwrap().ambient.as_ref().unwrap().hit,
        AmbientHit::Die
    );

    for (bad, why) in [
        (
            ambient(r#"{"burst": "mymod:nope"}"#, "24"),
            "an unknown hit bundle",
        ),
        (
            ambient(r#"{"burst": "petramond:torch_flame"}"#, "24"),
            "a hit bundle that is not a burst",
        ),
        (ambient(r#""die""#, "200"), "an out-of-range radius"),
        (
            ambient(r#"{"burst": "petramond:water_splash"}"#, "24")
                .replace(r#""radius": 24"#, r#""radius": 24, "kill": "interior""#),
            "a splash with no impact point (kill 'interior')",
        ),
        (
            r#"{"emitters": [{"emitter": "mymod:rainfall",
                    "ambient": {"count_per_intensity": 600, "max_count": 1500,
                        "radius": 24, "height": [4, 20], "fall_speed": [16, 22],
                        "size": [0.03, 0.05], "alpha": [0.4, 0.7],
                        "color": [[0.5, 0.5, 0.5], [0.7, 0.7, 0.7]]},
                    "burst": {"count_per_intensity": 3.0,
                        "up_speed": [1.0, 2.0], "radial_speed": [0.5, 1.5],
                        "lifetime": [0.4, 0.8], "size": [0.05, 0.1],
                        "color": [[0.1, 0.1, 0.5], [0.4, 0.8, 1.0]]} }]}"#
                .to_owned(),
            "declaring both ambient and burst",
        ),
    ] {
        assert!(
            parse_layers(&[&base(), bad.as_str()]).is_err(),
            "{why} must fail the load"
        );
    }
}
