//! Mod-declared procedural shapes (Layer 3): the `shapes.json` catalog a pack
//! ships to declare custom shape kinds its WASM bakes. A block row references
//! one by name (`"shape": "mymod:gate"`); the geometry comes from the pack's
//! bake (see the shape bake ABI), while this row carries the static metadata the
//! engine needs WITHOUT dispatching — the light shape, the nav profile, and
//! whether the block is a grass-decay participant — plus the fallback the
//! failure policy freezes a trapped bake to.
//!
//! The catalog is empty unless a pack ships `shapes.json`; engine shapes are the
//! compiled families, never rows here.

use std::sync::LazyLock;

use serde::Deserialize;

/// How a custom shape's cells participate in light propagation when the sim bake
/// has not (yet) produced an aperture — the simple declared tier.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CustomLight {
    /// Passes light like open air (the default).
    Open,
    /// Blocks light like a full cube.
    OpaqueCube,
    /// The sim bake supplies a per-half-cell aperture; the flood reads it.
    CustomAperture,
}

/// One `shapes.json` row: a custom shape kind's static declaration.
#[derive(Debug, PartialEq)]
pub struct CustomShapeDef {
    pub key: &'static str,
    /// Light behaviour (simple tier, or a marker that the bake supplies an
    /// aperture).
    pub light_shape: CustomLight,
    /// Whether navigation reads a cell of this shape as solid (the fence rule
    /// generalised): `nav_profile: "solid"`.
    pub nav_solid: bool,
    /// Whether the block participates in grass-decay / neighbour-update fan-out.
    pub grass_decay_eligible: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawCustomShapeDef {
    key: String,
    #[serde(default = "default_light")]
    light_shape: CustomLight,
    /// `"solid"` reads the cell solid to nav; anything else (or absent) does not.
    #[serde(default)]
    nav_profile: Option<String>,
    #[serde(default = "default_true")]
    grass_decay_eligible: bool,
}

fn default_light() -> CustomLight {
    CustomLight::Open
}

fn default_true() -> bool {
    true
}

#[derive(Deserialize)]
struct RawCustomShapeFile {
    shapes: Vec<RawCustomShapeDef>,
}

/// The loaded custom-shape catalog — id-ordered, or empty when no pack ships
/// `shapes.json`. Loads once; a malformed `shapes.json` fails loudly at startup
/// (the pack should have been disabled at admission).
fn defs() -> &'static [CustomShapeDef] {
    static DEFS: LazyLock<&'static [CustomShapeDef]> = LazyLock::new(|| {
        let layers = crate::assets::read_layers("shapes.json");
        if layers.is_empty() {
            return &[];
        }
        let texts: Vec<&str> = layers.iter().map(|(s, _)| s.as_str()).collect();
        crate::registry::load_catalog(
            &texts,
            |t| serde_json::from_str::<RawCustomShapeFile>(t).map(|f| f.shapes),
            |r| &r.key,
            &[], // no engine custom shapes — the compiled families cover those
            "shape",
            |r, id, names| {
                Ok(CustomShapeDef {
                    key: names.name(id).expect("id resolved from this table"),
                    light_shape: r.light_shape,
                    nav_solid: matches!(r.nav_profile.as_deref(), Some("solid")),
                    grass_decay_eligible: r.grass_decay_eligible,
                })
            },
        )
        .unwrap_or_else(|e| panic!("shapes.json: {e}"))
        .rows()
    });
    &DEFS
}

/// The custom shape declared under `key`, or `None` — used by the loader to
/// resolve a block row's `"shape": "mod:key"` reference.
pub(super) fn by_key(key: &str) -> Option<&'static CustomShapeDef> {
    defs().iter().find(|d| d.key == key)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> &'static [CustomShapeDef] {
        crate::registry::load_catalog(
            &[text],
            |t| serde_json::from_str::<RawCustomShapeFile>(t).map(|f| f.shapes),
            |r| &r.key,
            &[],
            "shape",
            |r, id, names| {
                Ok(CustomShapeDef {
                    key: names.name(id).expect("id from table"),
                    light_shape: r.light_shape,
                    nav_solid: matches!(r.nav_profile.as_deref(), Some("solid")),
                    grass_decay_eligible: r.grass_decay_eligible,
                })
            },
        )
        .expect("shapes parse")
        .rows()
    }

    #[test]
    fn shapes_json_declares_custom_shapes_with_metadata_and_defaults() {
        let defs = parse(
            r#"{"shapes":[
                {"key":"mymod:gate","light_shape":"opaque_cube","nav_profile":"solid","grass_decay_eligible":false},
                {"key":"mymod:vine"}
            ]}"#,
        );
        assert_eq!(defs.len(), 2);
        assert_eq!(defs[0].key, "mymod:gate");
        assert_eq!(defs[0].light_shape, CustomLight::OpaqueCube);
        assert!(defs[0].nav_solid);
        assert!(!defs[0].grass_decay_eligible);
        // Defaults: open light, non-solid nav, grass-decay eligible.
        assert_eq!(defs[1].light_shape, CustomLight::Open);
        assert!(!defs[1].nav_solid);
        assert!(defs[1].grass_decay_eligible);
    }
}
