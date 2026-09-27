use petramond_world::biome::Biome;
use petramond_world::chunk::{WORLD_MAX_Y, WORLD_MIN_Y};

use crate::data::bounds::{unit, unit_range, within};
use crate::feature::ConfiguredFeature;
use crate::rng::FeatureRng;

pub const MAX_TREE_SPACING_RADIUS: i32 = 10;

#[derive(Copy, Clone, Debug, PartialEq, Eq, Default, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TreeSupport {
    #[default]
    None,
    RedwoodBase,
}

#[derive(Clone)]
pub struct SpeciesTable {
    entries: Box<[(u32, &'static ConfiguredFeature)]>,
    total: u32,
}

impl std::fmt::Debug for SpeciesTable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SpeciesTable")
            .field(
                "weights",
                &self.entries.iter().map(|e| e.0).collect::<Vec<_>>(),
            )
            .field("total", &self.total)
            .finish()
    }
}

impl SpeciesTable {
    pub fn new(weighted: &[(u32, &'static ConfiguredFeature)]) -> Result<Self, String> {
        if weighted.is_empty() {
            return Err("species: at least one entry is required".into());
        }
        let mut total = 0u32;
        let mut entries = Vec::with_capacity(weighted.len());
        for &(weight, species) in weighted {
            if weight == 0 {
                return Err("species: every weight must be positive".into());
            }
            total = total
                .checked_add(weight)
                .ok_or("species: weights overflow")?;
            entries.push((total, species));
        }
        if total > i32::MAX as u32 {
            return Err(format!(
                "species: weights sum to {total}, above {}",
                i32::MAX
            ));
        }
        Ok(Self {
            entries: entries.into_boxed_slice(),
            total,
        })
    }

    pub fn pick(&self, rng: &mut FeatureRng) -> &'static ConfiguredFeature {
        if let [(_, only)] = self.entries.as_ref() {
            return only;
        }
        let roll = rng.next_i32(0, self.total as i32 - 1) as u32;
        self.entries
            .iter()
            .find(|(end, _)| roll < *end)
            .expect("the roll is below the summed weights")
            .1
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub struct GroveLattice {
    pub salt: u64,
    pub period: i32,
    pub detail_weight: f32,
    pub transition: (f32, f32),
    pub chance: (f32, f32),
}

pub const MIN_GROVE_PERIOD: i32 = 12;
pub const DEFAULT_GROVE_DETAIL_WEIGHT: f32 = 0.2;

#[derive(Copy, Clone, Debug, PartialEq)]
pub enum Territory {
    Grove(GroveLattice),
    NearbyBiome { biome: Biome, radius: i32 },
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum RuleStage {
    Candidate,
    Accepted,
}

impl Territory {
    pub fn stage(&self) -> RuleStage {
        match self {
            Territory::Grove(_) => RuleStage::Candidate,
            Territory::NearbyBiome { .. } => RuleStage::Accepted,
        }
    }
}

#[derive(Clone, Debug)]
pub struct SelectionRule {
    pub territory: Territory,
    pub species: SpeciesTable,
    pub density: Option<f32>,
    pub spacing_radius: Option<i32>,
}

#[derive(Clone, Debug)]
pub struct TreeProfile {
    pub density: f32,
    pub spacing_radius: i32,
    pub height_clearance: i32,
    pub support: TreeSupport,
    pub species: Option<SpeciesTable>,
    pub rules: Box<[SelectionRule]>,
}

impl Default for TreeProfile {
    fn default() -> Self {
        Self {
            density: 0.0,
            spacing_radius: 3,
            height_clearance: 14,
            support: TreeSupport::None,
            species: None,
            rules: Box::new([]),
        }
    }
}

impl TreeProfile {
    pub fn peak_density(&self) -> f32 {
        self.rules
            .iter()
            .filter_map(|r| r.density)
            .fold(self.density, f32::max)
    }

    pub fn deferred_rule_may_win(&self, fired: Option<usize>) -> bool {
        let limit = fired.unwrap_or(self.rules.len());
        self.rules[..limit]
            .iter()
            .any(|r| r.territory.stage() == RuleStage::Accepted)
    }

    pub fn validate(&self) -> Result<(), String> {
        unit("density", self.density)?;
        spacing("spacing_radius", self.spacing_radius)?;
        within(
            "height_clearance",
            self.height_clearance,
            1..=WORLD_MAX_Y - WORLD_MIN_Y - 1,
        )?;
        if self.density > 0.0 && self.species.is_none() {
            return Err("species: a profile with density above zero needs a species table".into());
        }
        for (i, rule) in self.rules.iter().enumerate() {
            rule.validate().map_err(|e| format!("rules[{i}]: {e}"))?;
        }
        Ok(())
    }
}

impl SelectionRule {
    fn validate(&self) -> Result<(), String> {
        match self.territory {
            Territory::Grove(lattice) => lattice.validate()?,
            Territory::NearbyBiome { radius, .. } => spacing("nearby_biome.radius", radius)?,
        }
        match self.territory.stage() {
            RuleStage::Candidate => {
                if let Some(d) = self.density {
                    unit("density", d)?;
                }
                if let Some(s) = self.spacing_radius {
                    spacing("spacing_radius", s)?;
                }
            }
            RuleStage::Accepted => {
                if self.density.is_some() || self.spacing_radius.is_some() {
                    return Err(
                        "a rule that reads neighbouring terrain is decided only for accepted \
                         origins, after density and spacing: it may state species only"
                            .into(),
                    );
                }
            }
        }
        Ok(())
    }
}

impl GroveLattice {
    fn validate(&self) -> Result<(), String> {
        if self.period < MIN_GROVE_PERIOD {
            return Err(format!(
                "grove.period: {} must be at least {MIN_GROVE_PERIOD}",
                self.period
            ));
        }
        unit("grove.detail_weight", self.detail_weight)?;
        unit_range("grove.transition", self.transition)?;
        if self.transition.0 == self.transition.1 {
            return Err("grove.transition: the band must be wider than zero".into());
        }
        unit_range("grove.chance", self.chance)
    }
}

fn spacing(field: &str, radius: i32) -> Result<(), String> {
    within(field, radius, 1..=MAX_TREE_SPACING_RADIUS)
}

#[inline]
pub fn profile(biome: Biome) -> &'static TreeProfile {
    crate::data::tree_profiles::profile(biome)
}

#[cfg(test)]
mod tests;
