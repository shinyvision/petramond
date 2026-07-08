//! Data-driven render shader hooks loaded from mod packs.
//!
//! The first hook is deliberately narrow: one active sky shader, selected by
//! pack load order. It may declare named `vec4<f32>` parameter slots; mods write
//! those names through the tick-side shader-param host call.

use std::path::PathBuf;

use serde::Deserialize;

use super::uniforms::SHADER_PARAM_SLOTS;

/// Fixed texture slots available to pack sky shaders. Slot `i` binds a
/// texture/sampler pair at group 1 bindings `i * 2` and `i * 2 + 1`.
pub(crate) const SKY_TEXTURE_SLOTS: usize = 4;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SkyShaderSpec {
    pub(crate) source: String,
    pub(crate) path: PathBuf,
    pub(crate) params: Vec<String>,
    pub(crate) textures: Vec<String>,
    pub(crate) sky_light_param: Option<String>,
}

#[derive(Deserialize)]
struct ShaderCatalog {
    #[serde(default)]
    sky: Option<SkyShaderRow>,
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

fn parse_catalog(text: &str) -> serde_json::Result<ShaderCatalog> {
    serde_json::from_str(text)
}

fn sky_shader_from_row(row: SkyShaderRow, catalog_path: &std::path::Path) -> Option<SkyShaderSpec> {
    if row.params.len() > SHADER_PARAM_SLOTS {
        log::warn!(
            "ignoring {} sky shader '{}': {} params exceeds the {SHADER_PARAM_SLOTS}-slot limit",
            catalog_path.display(),
            row.shader,
            row.params.len()
        );
        return None;
    }
    if row.textures.len() > SKY_TEXTURE_SLOTS {
        log::warn!(
            "ignoring {} sky shader '{}': {} textures exceeds the {SKY_TEXTURE_SLOTS}-slot limit",
            catalog_path.display(),
            row.shader,
            row.textures.len()
        );
        return None;
    }
    if let Some(bad) = row
        .params
        .iter()
        .find(|key| !crate::registry::is_namespaced(key))
    {
        log::warn!(
            "ignoring {} sky shader '{}': shader param '{bad}' is not namespaced",
            catalog_path.display(),
            row.shader
        );
        return None;
    }
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
    let Some((bytes, path)) = crate::assets::read_bytes(&row.shader) else {
        log::warn!(
            "ignoring {} sky shader '{}': WGSL source not found",
            catalog_path.display(),
            row.shader
        );
        return None;
    };
    let source = match String::from_utf8(bytes) {
        Ok(source) => source,
        Err(e) => {
            log::warn!(
                "ignoring {} sky shader '{}': source is not UTF-8: {e}",
                catalog_path.display(),
                row.shader
            );
            return None;
        }
    };
    Some(SkyShaderSpec {
        source,
        path,
        params: row.params,
        textures: row.textures,
        sky_light_param: row.sky_light_param,
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
