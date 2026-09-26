//! Biome rows — a layered catalog (`assets/biomes.json`): the colour set, the
//! `ambient` density map (which ambient particle bundles this biome drives,
//! and how strongly — see [`crate::particle_emitters`]), and the `trees`
//! placement profile and `generation` rules (surface stack, ground cover,
//! snow, behaviour flags), both carried verbatim for the worldgen layer to
//! parse. The row is a biome's single definition.
//!
//! Authored linear-light tints keep distinct colour families: spring greens,
//! deep woodland, golden dry grass, cool conifers and subdued alpine plants.
//! Water is blue/teal with darker swamp and deep-ocean variants. Horizon colours
//! retain biome atmosphere without bleaching the scene into a pastel wash.
//!
//! The biome id space is a registry: engine rows hold their frozen ids
//! (`ENGINE_BIOMES`), and a pack ADDS a biome with a namespaced row, which
//! registers the next id after the engine range in load order (a pack may
//! also OVERRIDE an engine row). Ids ride chunk bytes as one byte, so the
//! catalog caps at 255 biomes (id 0 is unassigned).

use std::collections::BTreeMap;

use serde::Deserialize;

use super::definition::BiomeDef;
use super::Biome;

/// Engine biome keys in frozen id order (`ENGINE_BIOMES[id - 1]` is `id`'s
/// key, matching the [`Biome`] consts; biome id 0 is unassigned).
/// Append-only; never reorder.
const ENGINE_BIOMES: &[&str] = &[
    "petramond:ocean",
    "petramond:beach",
    "petramond:river",
    "petramond:desert",
    "petramond:plains",
    "petramond:savanna",
    "petramond:forest",
    "petramond:swamp",
    "petramond:taiga",
    "petramond:snowy_tundra",
    "petramond:snowy_taiga",
    "petramond:mountains",
    "petramond:snowy_peaks",
    "petramond:deep_ocean",
    "petramond:foothills",
    "petramond:wetland",
    "petramond:redwood_forest",
    "petramond:old_growth_taiga",
    "petramond:meadow",
    "petramond:grove",
    "petramond:snowy_slopes",
    "petramond:windswept_hills",
    "petramond:stony_peaks",
    "petramond:wooded_hills",
    "petramond:mountain_edge",
    "petramond:desert_lakes",
    "petramond:snowy_plains",
];

/// Registered biomes cap at 255: the id is `row index + 1` in one byte.
const BIOME_ID_CAP: usize = u8::MAX as usize;

pub(super) const ENGINE_BIOME_COUNT: usize = ENGINE_BIOMES.len();

/// One biome row as written in `biomes.json`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawBiomeDef {
    biome: String,
    fog_color: [f32; 3],
    grass_color: [f32; 3],
    foliage_color: [f32; 3],
    water_color: [f32; 3],
    /// Ambient bundle key → density in `0..=1`: the bundles this biome drives
    /// on every client, and how thickly. Omitted bundles are absent here.
    #[serde(default)]
    ambient: BTreeMap<String, f32>,
    /// Tree placement profile. This layer only carries it: the vocabulary
    /// (density, spacing, species tables, selection rules) is worldgen's.
    #[serde(default)]
    trees: Option<serde_json::Value>,
    /// Generation rules (surface stack, ground cover, snow, flags). Carried
    /// like `trees`; worldgen owns the vocabulary.
    #[serde(default)]
    generation: Option<serde_json::Value>,
}

#[derive(Deserialize)]
struct RawFile {
    biomes: Vec<RawBiomeDef>,
}

/// The biome catalog stage of every content registry.
pub(crate) static CATALOG: crate::content::Slot<crate::registry::Catalog<BiomeDef>> =
    crate::content::Slot::new(crate::content::stage::BIOMES, &[], load);

fn load(
    reg: &crate::content::ContentRegistry,
) -> Result<crate::registry::Catalog<BiomeDef>, String> {
    crate::registry::read_catalog(reg.packs(), "biomes.json", "biome", parse_layers)
}

fn catalog() -> &'static crate::registry::Catalog<BiomeDef> {
    CATALOG.current()
}

fn parse_layers(texts: &[&str]) -> Result<crate::registry::Catalog<BiomeDef>, String> {
    crate::registry::load_catalog_with_capacity(
        texts,
        |text| serde_json::from_str::<RawFile>(text).map(|f| f.biomes),
        |r| &r.biome,
        ENGINE_BIOMES,
        "biome",
        BIOME_ID_CAP,
        |r, index, names| {
            let key = names.name(index).expect("index resolved from this table");
            let mut ambient = Vec::with_capacity(r.ambient.len());
            for (bundle, density) in r.ambient {
                if !crate::registry::is_namespaced(&bundle) {
                    return Err(format!(
                        "biome '{}': ambient bundle '{bundle}' must be a namespaced key",
                        r.biome
                    ));
                }
                if !density.is_finite() || !(0.0..=1.0).contains(&density) {
                    return Err(format!(
                        "biome '{}': ambient density for '{bundle}' must be in 0..=1",
                        r.biome
                    ));
                }
                ambient.push((&*bundle.leak(), density));
            }
            Ok(BiomeDef {
                biome: Biome(index as u8 + 1),
                key,
                name: engine_name(key).unwrap_or(key),
                fog_color: r.fog_color,
                grass_color: r.grass_color,
                foliage_color: r.foliage_color,
                water_color: r.water_color,
                ambient: Box::leak(ambient.into_boxed_slice()),
                trees: r.trees.map(|v| &*v.to_string().leak()),
                generation: r.generation.map(|v| &*v.to_string().leak()),
            })
        },
    )
}

/// The bare half of an engine key (`"petramond:forest"` -> `"forest"`),
/// `None` for a pack key.
fn engine_name(key: &str) -> Option<&str> {
    key.strip_prefix(crate::registry::ENGINE_NAMESPACE)
        .and_then(|rest| rest.strip_prefix(':'))
}

/// The number of registered biomes (engine plus pack rows).
#[inline]
pub(super) fn count() -> usize {
    catalog().rows().len()
}

/// The id registered under a namespaced key, or under an engine biome's
/// bare name.
pub(super) fn id_of(name: &str) -> Option<u8> {
    let index = if crate::registry::is_namespaced(name) {
        catalog().id(name)
    } else {
        catalog().id(&format!("{}:{name}", crate::registry::ENGINE_NAMESPACE))
    }?;
    Some(index as u8 + 1)
}

/// The row of a registered biome. Every [`Biome`] value is registered: the
/// consts name engine rows and [`Biome::from_id`] maps unknown ids to one.
#[inline]
pub(super) fn def(biome: Biome) -> &'static BiomeDef {
    &catalog().rows()[usize::from(biome.id()) - 1]
}

#[cfg(test)]
mod tests;
