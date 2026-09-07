//! Tree placement vocabulary: how densely a biome roots trees, how far apart
//! they stand, and which species each accepted site grows.
//!
//! Every value here is ROW DATA: the `trees` field of a biome's `biomes.json`
//! row, parsed by `data::tree_profiles`. A pack retunes a biome's woodland by
//! overriding its row; the engine's own biomes state theirs the same way.

use petramond_world::biome::Biome;
use petramond_world::chunk::CHUNK_SY;

use crate::data::bounds::{unit, unit_range, within};
use crate::feature::ConfiguredFeature;
use crate::rng::FeatureRng;

/// Chebyshev radius bound on every rule's terrain reads and every spacing
/// scan. The feature candidate window is sized from it, so any selector that
/// reads a neighbouring column must declare a radius at or below it.
pub const MAX_TREE_SPACING_RADIUS: i32 = 10;

/// Terrain the trunk needs under it before a site is accepted.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Default, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TreeSupport {
    /// The anchor column alone.
    #[default]
    None,
    /// The whole redwood base footprint must be within one block of the anchor.
    RedwoodBase,
}

/// A weighted species draw: one `next_i32` over the summed weights, so a row
/// author reads the table as percentages when the weights sum to 100. A single
/// species draws nothing — its geometry stream starts where the pick would.
#[derive(Clone)]
pub struct SpeciesTable {
    /// `(exclusive cumulative weight, species)` in row order.
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
    /// Build from `(weight, species)` pairs; `Err` names the invariant a bad
    /// table breaks.
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

/// A smooth world-anchored value field on a square lattice, used to carve a
/// biome's woodland into species territories with gradual mixed edges.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct GroveLattice {
    /// Positional stream salt; two rows naming the same field share a pattern
    /// across their biomes' common border.
    pub salt: u64,
    /// Broad lattice period in blocks. A detail lattice at a third of it
    /// breaks the broad cells' regularity (see `feature::tree_select::groves`).
    pub period: i32,
    /// Share of the blended field the detail lattice contributes, in 0..=1.
    pub detail_weight: f32,
    /// Field values mapped onto `chance.0 ..= chance.1`; below the band the
    /// rule fires at `chance.0`, above it at `chance.1`.
    pub transition: (f32, f32),
    /// Probability the rule claims a site at the two ends of the transition.
    pub chance: (f32, f32),
}

/// The lattice period below which corner values stop reading as territory.
pub const MIN_GROVE_PERIOD: i32 = 12;
/// Detail share that keeps broad cells dominant without a visible grid.
pub const DEFAULT_GROVE_DETAIL_WEIGHT: f32 = 0.2;

/// Where a species-selection rule applies.
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum Territory {
    /// Inside the lattice field's high band, with probability from the field.
    Grove(GroveLattice),
    /// Within `radius` (Chebyshev) of a column of `biome`.
    NearbyBiome { biome: Biome, radius: i32 },
}

/// When a rule's territory can be answered during a placement pass. Ordered:
/// a rule is answerable at any stage at or after its own.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum RuleStage {
    /// From the site's coordinates alone: decided for every candidate, so the
    /// rule may carry its own density and spacing.
    Candidate,
    /// Only by reading neighbouring terrain: decided for ACCEPTED origins only
    /// (spacing probes never read a neighbourhood), so the rule picks species
    /// and inherits the profile's density and spacing.
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

/// One species-selection rule. Rules are evaluated in row order and the first
/// whose territory holds decides the site; the profile's base species table is
/// the fallback.
#[derive(Clone, Debug)]
pub struct SelectionRule {
    pub territory: Territory,
    pub species: SpeciesTable,
    /// Candidate-stage rules only (`None` = the profile's).
    pub density: Option<f32>,
    /// Candidate-stage rules only (`None` = the profile's).
    pub spacing_radius: Option<i32>,
}

/// A biome's tree placement profile.
#[derive(Clone, Debug)]
pub struct TreeProfile {
    /// Per-column chance of rooting a tree before spacing competition.
    pub density: f32,
    /// Chebyshev radius the tree reserves against neighbours.
    pub spacing_radius: i32,
    /// Blocks of world height a rooted tree needs above its anchor.
    pub height_clearance: i32,
    pub support: TreeSupport,
    /// The species drawn where no rule claims the site. Empty only when
    /// `density` is zero.
    pub species: Option<SpeciesTable>,
    pub rules: Box<[SelectionRule]>,
}

/// The treeless profile a row states nothing beyond.
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
    /// The largest density any candidate-stage decision can reach: a roll at
    /// or above it can be rejected before any rule is consulted.
    pub fn peak_density(&self) -> f32 {
        self.rules
            .iter()
            .filter_map(|r| r.density)
            .fold(self.density, f32::max)
    }

    /// Whether a rule that is decided only for accepted origins sits ahead of
    /// `fired` (the candidate-stage rule index that claimed the site, `None`
    /// for the base table) and could still take the site. Such a rule's
    /// species inherit the profile's spacing, so the candidate reserves it.
    pub fn deferred_rule_may_win(&self, fired: Option<usize>) -> bool {
        let limit = fired.unwrap_or(self.rules.len());
        self.rules[..limit]
            .iter()
            .any(|r| r.territory.stage() == RuleStage::Accepted)
    }

    /// Structural invariants every loaded row must satisfy. Load-time only;
    /// the hot path trusts them.
    pub fn validate(&self) -> Result<(), String> {
        unit("density", self.density)?;
        spacing("spacing_radius", self.spacing_radius)?;
        within(
            "height_clearance",
            self.height_clearance,
            1..=CHUNK_SY as i32 - 1,
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

/// The loaded profile of `biome`.
#[inline]
pub fn profile(biome: Biome) -> &'static TreeProfile {
    crate::data::tree_profiles::profile(biome)
}

#[cfg(test)]
mod tests;
