//! Biome rows — a layered catalog (`assets/biomes.json`): the colour set, the
//! `ambient` density map (which ambient particle bundles this biome drives,
//! and how strongly — see [`crate::particle_emitters`]) and the `trees`
//! placement profile, carried verbatim for the worldgen layer to parse.
//!
//! Authored linear-light tints keep distinct colour families: spring greens,
//! deep woodland, golden dry grass, cool conifers and subdued alpine plants.
//! Water is blue/teal with darker swamp and deep-ocean variants. Horizon colours
//! retain biome atmosphere without bleaching the scene into a pastel wash.
//!
//! The biome ID SPACE stays compiled and closed: ids are serialized into
//! chunk bytes and the [`Biome`] enum is matched across worldgen, so a pack
//! may OVERRIDE an engine row's colours but cannot add biomes.

use std::collections::BTreeMap;
use std::sync::LazyLock;

use serde::Deserialize;

use super::definition::BiomeDef;
use super::Biome;

/// Engine biomes in frozen id order (`ENGINE_BIOMES[id - 1]` is `id`'s biome;
/// biome id 0 is unassigned). Append-only; never reorder.
const ENGINE_BIOMES: &[(Biome, &str)] = &[
    (Biome::Ocean, "petramond:ocean"),
    (Biome::Beach, "petramond:beach"),
    (Biome::River, "petramond:river"),
    (Biome::Desert, "petramond:desert"),
    (Biome::Plains, "petramond:plains"),
    (Biome::Savanna, "petramond:savanna"),
    (Biome::Forest, "petramond:forest"),
    (Biome::Swamp, "petramond:swamp"),
    (Biome::Taiga, "petramond:taiga"),
    (Biome::SnowyTundra, "petramond:snowy_tundra"),
    (Biome::SnowyTaiga, "petramond:snowy_taiga"),
    (Biome::Mountains, "petramond:mountains"),
    (Biome::SnowyPeaks, "petramond:snowy_peaks"),
    (Biome::DeepOcean, "petramond:deep_ocean"),
    (Biome::Foothills, "petramond:foothills"),
    (Biome::Wetland, "petramond:wetland"),
    (Biome::RedwoodForest, "petramond:redwood_forest"),
    (Biome::OldGrowthTaiga, "petramond:old_growth_taiga"),
    (Biome::Meadow, "petramond:meadow"),
    (Biome::Grove, "petramond:grove"),
    (Biome::SnowySlopes, "petramond:snowy_slopes"),
    (Biome::WindsweptHills, "petramond:windswept_hills"),
    (Biome::StonyPeaks, "petramond:stony_peaks"),
    (Biome::WoodedHills, "petramond:wooded_hills"),
    (Biome::MountainEdge, "petramond:mountain_edge"),
    (Biome::DesertLakes, "petramond:desert_lakes"),
    (Biome::SnowyPlains, "petramond:snowy_plains"),
];

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
}

#[derive(Deserialize)]
struct RawFile {
    biomes: Vec<RawBiomeDef>,
}

fn catalog() -> &'static crate::registry::Catalog<BiomeDef> {
    static TABLE: LazyLock<crate::registry::Catalog<BiomeDef>> =
        LazyLock::new(|| crate::registry::read_catalog("biomes.json", "biome", parse_layers));
    &TABLE
}

fn parse_layers(texts: &[&str]) -> Result<crate::registry::Catalog<BiomeDef>, String> {
    let engine_names: Vec<&'static str> = ENGINE_BIOMES.iter().map(|(_, n)| *n).collect();
    crate::registry::load_catalog(
        texts,
        |text| serde_json::from_str::<RawFile>(text).map(|f| f.biomes),
        |r| &r.biome,
        &engine_names,
        "biome",
        |r, id, _| {
            let Some(&(biome, key)) = ENGINE_BIOMES.get(id as usize) else {
                return Err(format!(
                    "biome '{}': biomes are engine-defined (their ids are serialized into \
                     chunk bytes); packs may only override engine rows",
                    r.biome
                ));
            };
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
                biome,
                name: key.strip_prefix("petramond:").expect("engine biome key"),
                fog_color: r.fog_color,
                grass_color: r.grass_color,
                foliage_color: r.foliage_color,
                water_color: r.water_color,
                ambient: Box::leak(ambient.into_boxed_slice()),
                trees: r.trees.map(|v| &*v.to_string().leak()),
            })
        },
    )
}

#[inline]
pub(super) fn from_id(id: u8) -> Biome {
    ENGINE_BIOMES
        .get(id.saturating_sub(1) as usize)
        .map_or(Biome::Ocean, |&(b, _)| b)
}

#[inline]
pub(super) fn def(biome: Biome) -> &'static BiomeDef {
    &catalog().rows()[(biome.id() - 1) as usize]
}

#[cfg(test)]
mod tests;
