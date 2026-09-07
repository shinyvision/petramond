use super::*;

#[test]
fn shaders_json_accepts_sky_entry_shape() {
    let catalog = parse_catalog(
        r#"{
            "sky": {
                "shader": "shaders/daynight_sky.wgsl",
                "params": ["petramond:time", "petramond:light"],
                "textures": [
                    "textures/environment/sun.png",
                    "textures/environment/moon_phases.png"
                ],
                "sky_light_param": "petramond:light"
            }
        }"#,
    )
    .expect("catalog parses");
    let row = catalog.sky.expect("sky row");
    assert_eq!(row.shader, "shaders/daynight_sky.wgsl");
    assert_eq!(row.params, ["petramond:time", "petramond:light"]);
    assert_eq!(
        row.textures,
        [
            "textures/environment/sun.png",
            "textures/environment/moon_phases.png"
        ]
    );
    assert_eq!(row.sky_light_param.as_deref(), Some("petramond:light"));
}

/// Parse + validate every bundled mod pack's WGSL with the same naga
/// wgpu embeds, so a shader typo fails `cargo test` instead of the first
/// windowed launch. Sources are read from `mods-src/*/pack` (the tracked
/// tree — `mods/` is build output and may be absent).
#[test]
fn bundled_pack_shaders_parse_and_validate() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("workspace root above crates/petramond-render")
        .join("mods-src");
    let mut checked = 0;
    for entry in std::fs::read_dir(&root).expect("mods-src exists") {
        let dir = entry.expect("dir entry").path().join("pack/shaders");
        let Ok(shaders) = std::fs::read_dir(&dir) else {
            continue;
        };
        for shader in shaders {
            let path = shader.expect("shader entry").path();
            if path.extension().is_none_or(|e| e != "wgsl") {
                continue;
            }
            let source = std::fs::read_to_string(&path).expect("shader reads");
            let module = naga::front::wgsl::parse_str(&source)
                .unwrap_or_else(|e| panic!("{} fails to parse: {e}", path.display()));
            naga::valid::Validator::new(
                naga::valid::ValidationFlags::all(),
                naga::valid::Capabilities::default(),
            )
            .validate(&module)
            .unwrap_or_else(|e| panic!("{} fails validation: {e:?}", path.display()));
            checked += 1;
        }
    }
    assert!(checked >= 1, "the weather pack ships at least clouds.wgsl");
    // Engine shaders have no fallback path; validate composed sources as used.
    for standalone in [
        "grade.wgsl",
        "crosshair.wgsl",
        "ui.wgsl",
        "env_downsample.wgsl",
        "env_composite.wgsl",
    ] {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("shaders")
            .join(standalone);
        let source = if standalone == "env_downsample.wgsl" {
            crate::pipeline::scaler_sources(false).0
        } else if standalone == "env_composite.wgsl" {
            crate::pipeline::scaler_sources(false).1
        } else if standalone == "grade.wgsl" {
            crate::pipeline::GRADE_SHADER.to_owned()
        } else {
            std::fs::read_to_string(&path).expect("engine shader reads")
        };
        let module = naga::front::wgsl::parse_str(&source)
            .unwrap_or_else(|e| panic!("{standalone} fails to parse: {e}"));
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::default(),
        )
        .validate(&module)
        .unwrap_or_else(|e| panic!("{standalone} fails validation: {e:?}"));
    }
}

#[test]
fn environment_rows_compose_in_layer_order_and_skip_invalid() {
    let a = r#"{
        "environment": {
            "shader": "shaders/daynight_sky.wgsl",
            "params": ["a:one"]
        }
    }"#;
    // Invalid: bare (non-namespaced) param — skipped, not substituted.
    let bad = r#"{
        "environment": {
            "shader": "shaders/daynight_sky.wgsl",
            "params": ["bare"]
        }
    }"#;
    // A layer with only a sky row contributes no environment pass.
    let sky_only = r#"{ "sky": { "shader": "shaders/daynight_sky.wgsl" } }"#;
    let b = r#"{
        "environment": {
            "shader": "shaders/daynight_sky.wgsl",
            "params": ["b:one", "b:two"]
        }
    }"#;

    let specs = environment_shaders_from_layers([
        (a.into(), PathBuf::from("assets/shaders.json")),
        (bad.into(), PathBuf::from("mods/bad/shaders.json")),
        (sky_only.into(), PathBuf::from("mods/sky/shaders.json")),
        (b.into(), PathBuf::from("mods/b/shaders.json")),
    ]);

    assert_eq!(specs.len(), 2, "valid rows compose; invalid/absent skip");
    assert_eq!(specs[0].params, ["a:one"]);
    assert_eq!(specs[1].params, ["b:one", "b:two"]);
}

#[test]
fn active_sky_shader_uses_the_highest_priority_catalog_layer() {
    let base = r#"{
        "sky": {
            "shader": "shaders/daynight_sky.wgsl",
            "params": ["base:time"],
            "sky_light_param": "base:time"
        }
    }"#;
    let pack = r#"{
        "sky": {
            "shader": "shaders/daynight_sky.wgsl",
            "params": ["pack:time", "pack:light"],
            "sky_light_param": "pack:light"
        }
    }"#;

    let spec = active_sky_shader_from_layers([
        (base.into(), PathBuf::from("assets/shaders.json")),
        (pack.into(), PathBuf::from("mods/pack/shaders.json")),
    ])
    .expect("pack layer selects an active sky shader");

    assert_eq!(spec.params, ["pack:time", "pack:light"]);
    assert_eq!(spec.sky_light_param.as_deref(), Some("pack:light"));
    assert!(spec.path.ends_with("shaders/daynight_sky.wgsl"));
}
