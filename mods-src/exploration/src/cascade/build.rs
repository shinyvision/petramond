use std::collections::BTreeSet;

use super::basin::{adopt_pits, Basins, Survey};
use super::flood::{reach_stays_in_extent, Geometry, Reach};
use super::notch::cut_notches;
use super::seal::{rim_is_a_wall, seal};
use super::site::Trace;
use super::OPEN_PERCENT;

pub struct Built {
    pub(super) basins: Basins,
    pub(super) silt: BTreeSet<[i32; 3]>,
    pub(super) cuts: BTreeSet<[i32; 3]>,
    pub(super) reach: Reach,
    pub domain_lo: [i32; 3],
    pub domain_hi: [i32; 3],
}

impl Trace {
    /// `terrain` gives the probed cells, `None` past that.
    ///
    /// Rim edge won't seal? The water just backs off, that's fine. The only real failure is no
    /// usable descent anywhere.
    ///
    /// Order: grow chain, adopt pits, seal rim, cut notches, then the gates for rim height,
    /// visibility and containment.
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
    #[cfg(test)]
    pub fn live_counts(&self) -> Vec<usize> {
        self.basins.counts()
    }
}
