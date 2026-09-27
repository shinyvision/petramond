use std::collections::BTreeMap;

use serde::Deserialize;

use super::definition::BiomeDef;
use super::Biome;

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

const BIOME_ID_CAP: usize = u8::MAX as usize;

pub(super) const ENGINE_BIOME_COUNT: usize = ENGINE_BIOMES.len();

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawBiomeDef {
    biome: String,
    fog_color: [f32; 3],
    grass_color: [f32; 3],
    foliage_color: [f32; 3],
    water_color: [f32; 3],
    #[serde(default)]
    ambient: BTreeMap<String, f32>,
    #[serde(default)]
    trees: Option<serde_json::Value>,
    #[serde(default)]
    generation: Option<serde_json::Value>,
}

#[derive(Deserialize)]
struct RawFile {
    biomes: Vec<RawBiomeDef>,
}

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

fn engine_name(key: &str) -> Option<&str> {
    key.strip_prefix(crate::registry::ENGINE_NAMESPACE)
        .and_then(|rest| rest.strip_prefix(':'))
}

#[inline]
pub(super) fn count() -> usize {
    catalog().rows().len()
}

pub(super) fn id_of(name: &str) -> Option<u8> {
    let index = if crate::registry::is_namespaced(name) {
        catalog().id(name)
    } else {
        catalog().id(&format!("{}:{name}", crate::registry::ENGINE_NAMESPACE))
    }?;
    Some(index as u8 + 1)
}

#[inline]
pub(super) fn def(biome: Biome) -> &'static BiomeDef {
    &catalog().rows()[usize::from(biome.id()) - 1]
}

#[cfg(test)]
mod tests;
