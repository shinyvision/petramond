//! The `trees` field of a biome row (`assets/biomes.json`), parsed into a
//! [`TreeProfile`].
//!
//! The biome layer carries the object verbatim (`Biome::trees`); this module
//! owns its vocabulary. A pack retunes a biome's woodland — density, spacing,
//! species mix, grove territories — by overriding that biome's row; species
//! are named `features.json` rows, so a pack-added tree species is placed by
//! naming it here. A row without `trees` roots nothing.

use std::sync::LazyLock;

use petramond_world::biome::Biome;
use serde::Deserialize;

use crate::biome::trees::{
    GroveLattice, SelectionRule, SpeciesTable, Territory, TreeProfile, TreeSupport,
    DEFAULT_GROVE_DETAIL_WEIGHT,
};
use crate::data::features;

/// The `trees` object as written on a biome row.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawProfile {
    #[serde(default)]
    density: f32,
    #[serde(default = "default_spacing_radius")]
    spacing_radius: i32,
    #[serde(default = "default_height_clearance")]
    height_clearance: i32,
    #[serde(default)]
    support: TreeSupport,
    #[serde(default)]
    species: Vec<RawSpecies>,
    #[serde(default)]
    rules: Vec<RawRule>,
}

fn default_spacing_radius() -> i32 {
    TreeProfile::default().spacing_radius
}

fn default_height_clearance() -> i32 {
    TreeProfile::default().height_clearance
}

fn default_detail_weight() -> f32 {
    DEFAULT_GROVE_DETAIL_WEIGHT
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawSpecies {
    feature: String,
    weight: u32,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawRule {
    #[serde(rename = "when")]
    territory: RawTerritory,
    species: Vec<RawSpecies>,
    density: Option<f32>,
    spacing_radius: Option<i32>,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
enum RawTerritory {
    Grove {
        /// Name of the shared lattice field; rows naming the same field
        /// continue one pattern across their biomes' border.
        field: String,
        period: i32,
        #[serde(default = "default_detail_weight")]
        detail_weight: f32,
        transition: (f32, f32),
        chance: (f32, f32),
    },
    NearbyBiome {
        biome: String,
        radius: i32,
    },
}

fn species_table(rows: &[RawSpecies]) -> Result<SpeciesTable, String> {
    let weighted = rows
        .iter()
        .map(|s| {
            features::by_name(&s.feature)
                .map(|f| (s.weight, f))
                .ok_or_else(|| format!("species: unknown worldgen feature '{}'", s.feature))
        })
        .collect::<Result<Vec<_>, _>>()?;
    SpeciesTable::new(&weighted)
}

/// A biome named by its registry key (`petramond:forest`, or a pack's
/// `mod_id:name`); catalogs never use bare names.
fn biome_named(key: &str) -> Result<Biome, String> {
    Some(key)
        .filter(|key| petramond_world::registry::is_namespaced(key))
        .and_then(Biome::from_name)
        .ok_or_else(|| format!("unknown biome '{key}'"))
}

impl RawRule {
    fn resolve(self, index: usize) -> Result<SelectionRule, String> {
        let context = |e: String| format!("rules[{index}]: {e}");
        let territory = match self.territory {
            RawTerritory::Grove {
                field,
                period,
                detail_weight,
                transition,
                chance,
            } => Territory::Grove(GroveLattice {
                salt: crate::salts::grove_field(&field),
                period,
                detail_weight,
                transition,
                chance,
            }),
            RawTerritory::NearbyBiome { biome, radius } => Territory::NearbyBiome {
                biome: biome_named(&biome).map_err(|e| context(format!("nearby_biome: {e}")))?,
                radius,
            },
        };
        Ok(SelectionRule {
            territory,
            species: species_table(&self.species).map_err(context)?,
            density: self.density,
            spacing_radius: self.spacing_radius,
        })
    }
}

impl RawProfile {
    fn resolve(self) -> Result<TreeProfile, String> {
        let species = if self.species.is_empty() {
            None
        } else {
            Some(species_table(&self.species)?)
        };
        let rules = self
            .rules
            .into_iter()
            .enumerate()
            .map(|(i, r)| r.resolve(i))
            .collect::<Result<Vec<_>, _>>()?;
        let profile = TreeProfile {
            density: self.density,
            spacing_radius: self.spacing_radius,
            height_clearance: self.height_clearance,
            support: self.support,
            species,
            rules: rules.into_boxed_slice(),
        };
        profile.validate()?;
        Ok(profile)
    }
}

/// Parse one biome row's `trees` text; `None` is the treeless default.
fn parse(trees: Option<&str>) -> Result<TreeProfile, String> {
    match trees {
        None => Ok(TreeProfile::default()),
        Some(text) => serde_json::from_str::<RawProfile>(text)
            .map_err(|e| format!("trees: {e}"))?
            .resolve()
            .map_err(|e| format!("trees: {e}")),
    }
}

fn table() -> &'static [TreeProfile] {
    static TABLE: LazyLock<Box<[TreeProfile]>> = LazyLock::new(|| {
        Biome::all()
            .map(|biome| {
                parse(biome.trees())
                    .unwrap_or_else(|e| panic!("biomes.json: biome '{}': {e}", biome.key()))
            })
            .collect()
    });
    &TABLE
}

/// The loaded tree profile of `biome`.
#[inline]
pub fn profile(biome: Biome) -> &'static TreeProfile {
    &table()[usize::from(biome.id()) - 1]
}

#[cfg(test)]
mod tests;
