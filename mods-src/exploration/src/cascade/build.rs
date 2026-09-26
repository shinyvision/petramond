//! The build pipeline: one trace against its probed terrain, stage by stage,
//! into a containment-proven basin chain.

use std::collections::BTreeSet;

use super::basin::{adopt_pits, Basins, Survey};
use super::flood::{reach_stays_in_extent, Geometry, Reach};
use super::notch::cut_notches;
use super::seal::{rim_is_a_wall, seal};
use super::site::Trace;
use super::OPEN_PERCENT;

/// A basin chain built against the probed terrain, containment-proven with no
/// giants. [`Built::finish`] folds the giants in and assembles the feature.
pub struct Built {
    pub(super) basins: Basins,
    pub(super) silt: BTreeSet<[i32; 3]>,
    pub(super) cuts: BTreeSet<[i32; 3]>,
    /// The no-giant reachable set (proof of the base geometry).
    pub(super) reach: Reach,
    /// World box the probes cover, for the caller to gather giants against.
    pub domain_lo: [i32; 3],
    pub domain_hi: [i32; 3],
}

impl Trace {
    /// Build the basin chain against the probed terrain, or say why not.
    ///
    /// `terrain` answers the probed cells (`None` past them). The build is
    /// OPTIMISTIC by design: an unsealable rim edge retreats the water, only
    /// terrain with no usable descent rejects. The stages, in order: grow the
    /// chain, adopt enclosed pits, seal the rim (retreating where it cannot
    /// hold), cut the spill notches, then the gates — rim height, visibility,
    /// and the containment proof.
    pub fn build(
        &self,
        terrain: &impl Fn([i32; 3]) -> Option<bool>,
    ) -> Result<Built, &'static str> {
        let survey = Survey::new(self, terrain);
        let mut basins = survey.grow_chain(self)?;
        adopt_pits(&mut basins, terrain);
        let sealed = seal(&mut basins, terrain)?;
        let mut silt = sealed.silt;
        let cuts = cut_notches(&basins, &mut silt, terrain)?;
        // Measured AFTER the notches, on what actually stands.
        if rim_is_a_wall(&silt, &sealed.rim_dam) {
            return Err("rim is a wall, not a lip");
        }
        if !water_is_open(&basins, terrain) {
            return Err("water hidden under rock");
        }
        let wet = basins.wet();
        let reach = Geometry {
            basins: &basins,
            wet: &wet,
            silt: &silt,
            cuts: &cuts,
            bodies: &BTreeSet::new(),
        }
        .flood(terrain)?;
        if !reach_stays_in_extent(&wet, &reach) {
            return Err("reach leaves the basin's own extent");
        }
        let (domain_lo, domain_hi) = survey.domain();
        Ok(Built {
            basins,
            silt,
            cuts,
            reach,
            domain_lo,
            domain_hi,
        })
    }
}

/// Enough of the water must lie under open cave. Overhanging shores are
/// welcome; a flooded crack is not.
fn water_is_open(basins: &Basins, terrain: &impl Fn([i32; 3]) -> Option<bool>) -> bool {
    let mut surface = 0usize;
    let mut open_sky = 0usize;
    for (&(x, z), &(pi, _)) in &basins.cols {
        let s = basins.pools[pi];
        surface += 1;
        if (1..=2).all(|h| terrain([x, s + h, z]) == Some(false)) {
            open_sky += 1;
        }
    }
    surface != 0 && open_sky * 100 >= surface * OPEN_PERCENT
}

impl Built {
    /// Columns each pool holds (a pool the seal retreat emptied keeps its
    /// index with zero).
    #[cfg(test)]
    pub fn live_counts(&self) -> Vec<usize> {
        self.basins.counts()
    }
}
