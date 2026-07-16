//! Biome colour rows — a layered catalog (`assets/biomes.json`).
//!
//! The storybook biome palette. Curation rules, applied 2026-07 with the
//! worldgen stylization pass (they govern edits to `biomes.json`):
//!
//! - GRASS/FOLIAGE greens are warm-shifted (toward yellow-green) and sit in a
//!   few deliberate families — lush (plains/meadow), deep woodland (forest),
//!   golden dry (savanna/desert scrub), cool sage (taiga/cold), muted alpine
//!   (peaks) — instead of one neon green with per-biome noise.
//! - WATER is one turquoise family across the world, varied only slightly per
//!   biome (murky in swamp/wetland, ink-deep in deep ocean), so water always
//!   reads as inviting storybook water.
//! - FOG colours are airier (lighter, softer) than their biome mood suggests:
//!   they feed the atmosphere haze and the sky horizon, and distance must
//!   LIGHTEN. Only mood biomes (swamp, redwood) keep a denser tinted fog.
//!
//! The biome ID SPACE stays compiled and closed: ids are serialized into
//! chunk bytes and the [`Biome`] enum is matched across worldgen, so a pack
//! may OVERRIDE an engine row's colours but cannot add biomes.

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
            Ok(BiomeDef {
                biome,
                name: key.strip_prefix("petramond:").expect("engine biome key"),
                fog_color: r.fog_color,
                grass_color: r.grass_color,
                foliage_color: r.foliage_color,
                water_color: r.water_color,
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
mod tests {
    use super::*;

    /// A pack may recolour an engine biome, but a new biome key is refused —
    /// the id space is chunk-serialized and enum-closed.
    #[test]
    fn packs_may_override_colours_but_not_add_biomes() {
        let base = std::fs::read_to_string(
            crate::assets::candidate_paths("biomes.json")
                .into_iter()
                .find(|p| p.exists())
                .expect("shipped biomes.json"),
        )
        .unwrap();
        let recolour = r#"{"biomes": [{"biome": "petramond:forest",
            "fog_color": [0.1, 0.2, 0.3], "grass_color": [0.1, 0.2, 0.3],
            "foliage_color": [0.1, 0.2, 0.3], "water_color": [0.1, 0.2, 0.3]}]}"#;
        let table = parse_layers(&[&base, recolour]).expect("override loads");
        assert_eq!(table.rows().len(), ENGINE_BIOME_COUNT);
        let forest = &table.rows()[(Biome::Forest.id() - 1) as usize];
        assert_eq!(forest.grass_color, [0.1, 0.2, 0.3]);

        let addition = r#"{"biomes": [{"biome": "mymod:crystal_fields",
            "fog_color": [0.1, 0.2, 0.3], "grass_color": [0.1, 0.2, 0.3],
            "foliage_color": [0.1, 0.2, 0.3], "water_color": [0.1, 0.2, 0.3]}]}"#;
        let err = match parse_layers(&[&base, addition]) {
            Ok(_) => panic!("additions must be refused"),
            Err(e) => e,
        };
        assert!(err.contains("engine-defined"), "{err}");
    }
}
