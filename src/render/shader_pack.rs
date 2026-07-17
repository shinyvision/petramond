//! Data-driven render shader hooks loaded from mod packs.
//!
//! Two hooks exist:
//!
//! - one active `sky` shader, selected by pack load order (highest layer
//!   wins) — replaces the built-in sky background;
//! - any number of `environment` shaders, one per pack layer, COMPOSED in
//!   pack load order — each becomes a full-screen depth-aware pass drawn
//!   after all depth-writing world geometry (volumetrics: clouds, auroras,
//!   fog volumes). An invalid environment row is skipped, never substituted.
//!
//! Both may declare named `vec4<f32>` parameter slots; mods write those names
//! through the tick-side shader-param host call.

use std::path::PathBuf;

use serde::Deserialize;

use super::uniforms::SHADER_PARAM_SLOTS;

/// Fixed texture slots available to pack sky and environment shaders. Slot
/// `i` binds a texture/sampler pair at group 1 bindings `i * 2` and
/// `i * 2 + 1`.
pub(crate) const SKY_TEXTURE_SLOTS: usize = 4;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SkyShaderSpec {
    pub(crate) source: String,
    pub(crate) path: PathBuf,
    pub(crate) params: Vec<String>,
    pub(crate) textures: Vec<String>,
    pub(crate) sky_light_param: Option<String>,
}

/// One full-screen composed volumetric pass supplied by a pack.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct EnvironmentShaderSpec {
    pub(crate) source: String,
    pub(crate) path: PathBuf,
    pub(crate) params: Vec<String>,
    pub(crate) textures: Vec<String>,
}

#[derive(Deserialize)]
struct ShaderCatalog {
    #[serde(default)]
    sky: Option<SkyShaderRow>,
    #[serde(default)]
    environment: Option<EnvironmentShaderRow>,
}

#[derive(Deserialize)]
struct SkyShaderRow {
    shader: String,
    #[serde(default)]
    params: Vec<String>,
    #[serde(default)]
    textures: Vec<String>,
    #[serde(default)]
    sky_light_param: Option<String>,
}

#[derive(Deserialize)]
struct EnvironmentShaderRow {
    shader: String,
    #[serde(default)]
    params: Vec<String>,
    #[serde(default)]
    textures: Vec<String>,
}

pub(crate) fn active_sky_shader() -> Option<SkyShaderSpec> {
    active_sky_shader_from_layers(crate::assets::read_layers("shaders.json"))
}

fn active_sky_shader_from_layers(
    layers: impl IntoIterator<Item = (String, PathBuf)>,
) -> Option<SkyShaderSpec> {
    let mut chosen = None;
    for (text, path) in layers {
        match parse_catalog(&text) {
            Ok(catalog) => {
                if let Some(row) = catalog.sky {
                    chosen = Some((row, path));
                }
            }
            Err(e) => log::warn!("ignoring {}: invalid shaders.json: {e}", path.display()),
        }
    }
    let (row, catalog_path) = chosen?;
    sky_shader_from_row(row, &catalog_path)
}

/// Every pack layer's `environment` row, in pack load order (base first).
/// Unlike the sky, environment passes COMPOSE — each valid row becomes one
/// full-screen pass; invalid rows are skipped with a warning.
pub(crate) fn environment_shaders() -> Vec<EnvironmentShaderSpec> {
    environment_shaders_from_layers(crate::assets::read_layers("shaders.json"))
}

fn environment_shaders_from_layers(
    layers: impl IntoIterator<Item = (String, PathBuf)>,
) -> Vec<EnvironmentShaderSpec> {
    let mut specs = Vec::new();
    for (text, path) in layers {
        match parse_catalog(&text) {
            Ok(catalog) => {
                if let Some(row) = catalog.environment {
                    if let Some(spec) = environment_shader_from_row(row, &path) {
                        specs.push(spec);
                    }
                }
            }
            Err(e) => log::warn!("ignoring {}: invalid shaders.json: {e}", path.display()),
        }
    }
    specs
}

fn parse_catalog(text: &str) -> serde_json::Result<ShaderCatalog> {
    serde_json::from_str(text)
}

/// The row checks both shader kinds share: slot limits, namespaced params,
/// and a readable UTF-8 WGSL source. Returns the source text + its path.
fn checked_shader_source(
    kind: &str,
    shader: &str,
    params: &[String],
    textures: &[String],
    catalog_path: &std::path::Path,
) -> Option<(String, PathBuf)> {
    if params.len() > SHADER_PARAM_SLOTS {
        log::warn!(
            "ignoring {} {kind} shader '{shader}': {} params exceeds the {SHADER_PARAM_SLOTS}-slot limit",
            catalog_path.display(),
            params.len()
        );
        return None;
    }
    if textures.len() > SKY_TEXTURE_SLOTS {
        log::warn!(
            "ignoring {} {kind} shader '{shader}': {} textures exceeds the {SKY_TEXTURE_SLOTS}-slot limit",
            catalog_path.display(),
            textures.len()
        );
        return None;
    }
    if let Some(bad) = params.iter().find(|key| !crate::registry::is_namespaced(key)) {
        log::warn!(
            "ignoring {} {kind} shader '{shader}': shader param '{bad}' is not namespaced",
            catalog_path.display(),
        );
        return None;
    }
    let Some((bytes, path)) = crate::assets::read_bytes(shader) else {
        log::warn!(
            "ignoring {} {kind} shader '{shader}': WGSL source not found",
            catalog_path.display(),
        );
        return None;
    };
    match String::from_utf8(bytes) {
        Ok(source) => Some((source, path)),
        Err(e) => {
            log::warn!(
                "ignoring {} {kind} shader '{shader}': source is not UTF-8: {e}",
                catalog_path.display(),
            );
            None
        }
    }
}

fn sky_shader_from_row(row: SkyShaderRow, catalog_path: &std::path::Path) -> Option<SkyShaderSpec> {
    if let Some(key) = row.sky_light_param.as_ref() {
        if !row.params.iter().any(|p| p == key) {
            log::warn!(
                "ignoring {} sky shader '{}': sky_light_param '{key}' is not in params",
                catalog_path.display(),
                row.shader
            );
            return None;
        }
    }
    let (source, path) =
        checked_shader_source("sky", &row.shader, &row.params, &row.textures, catalog_path)?;
    Some(SkyShaderSpec {
        source,
        path,
        params: row.params,
        textures: row.textures,
        sky_light_param: row.sky_light_param,
    })
}

fn environment_shader_from_row(
    row: EnvironmentShaderRow,
    catalog_path: &std::path::Path,
) -> Option<EnvironmentShaderSpec> {
    let (source, path) = checked_shader_source(
        "environment",
        &row.shader,
        &row.params,
        &row.textures,
        catalog_path,
    )?;
    Some(EnvironmentShaderSpec {
        source,
        path,
        params: row.params,
        textures: row.textures,
    })
}

#[cfg(test)]
mod tests {
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
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("mods-src");
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
        // Standalone ENGINE shaders too (concat-composed ones can't parse
        // alone): grade.wgsl has no fallback path — a typo would panic the
        // renderer at construction, so catch it here instead.
        for standalone in [
            "grade.wgsl",
            "crosshair.wgsl",
            "ui.wgsl",
            "env_downsample.wgsl",
            "env_composite.wgsl",
        ] {
            let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("src/shaders")
                .join(standalone);
            let source = std::fs::read_to_string(&path).expect("engine shader reads");
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
}
