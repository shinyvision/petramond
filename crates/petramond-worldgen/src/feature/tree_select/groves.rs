//! Grove territories: a smooth world-anchored lattice field, sampled per tree
//! site, that decides whether a `Territory::Grove` rule claims the site.

use crate::biome::trees::GroveLattice;
use crate::rng::FeatureRng;

/// Salt xor for the detail lattice, so it never mirrors the broad one.
const DETAIL_SALT: u64 = 0xD37A_11ED_670E_0001;
/// Salt xor for the per-site claim draw: territories must not consume the
/// density, spacing or geometry streams.
const CHOICE_SALT: u64 = 0x0000_7A3E_670E_C401;
/// The detail lattice repeats this many times per broad period.
const DETAIL_PERIODS_PER_BROAD: i32 = 3;

/// Corner values of one `(salt, period)` lattice over a placement window, in
/// a dense grid. Corners are pure functions of `(seed, salt, lattice x, z)`,
/// so the grid only dedupes work — it can never change an answer.
struct LatticeCorners {
    salt: u64,
    period: i32,
    x0: i32,
    z0: i32,
    nx: usize,
    values: Box<[f32]>,
}

impl LatticeCorners {
    fn new(salt: u64, period: i32, window: Window) -> Self {
        let x0 = window.x_min.div_euclid(period);
        let z0 = window.z_min.div_euclid(period);
        // The last in-window block lies in cell `max.div_euclid(period)`, whose
        // far corner is one lattice step beyond.
        let nx = ((window.x_max - 1).div_euclid(period) + 1 - x0 + 1) as usize;
        let nz = ((window.z_max - 1).div_euclid(period) + 1 - z0 + 1) as usize;
        Self {
            salt,
            period,
            x0,
            z0,
            nx,
            values: vec![f32::NAN; nx * nz].into_boxed_slice(),
        }
    }

    fn corner(&mut self, seed: u32, x: i32, z: i32) -> f32 {
        let i = (z - self.z0) as usize * self.nx + (x - self.x0) as usize;
        let v = &mut self.values[i];
        if v.is_nan() {
            *v = FeatureRng::positional(seed, self.salt, x, 0, z).next_f32();
        }
        *v
    }
}

/// The world-coordinate rectangle a placement pass samples, `[min, max)`.
#[derive(Copy, Clone)]
pub(super) struct Window {
    pub(super) x_min: i32,
    pub(super) x_max: i32,
    pub(super) z_min: i32,
    pub(super) z_max: i32,
}

impl Window {
    #[inline]
    pub(super) fn contains(&self, wx: i32, wz: i32) -> bool {
        (self.x_min..self.x_max).contains(&wx) && (self.z_min..self.z_max).contains(&wz)
    }
}

/// Per-pass lattice corner memo for every grove rule the pass meets.
pub(super) struct GroveField {
    seed: u32,
    window: Window,
    lattices: Vec<LatticeCorners>,
}

impl GroveField {
    pub(super) fn new(seed: u32, window: Window) -> Self {
        Self {
            seed,
            window,
            lattices: Vec::new(),
        }
    }

    /// Whether `lattice`'s territory claims the site at `(wx, wz)`.
    pub(super) fn claims(&mut self, lattice: &GroveLattice, wx: i32, wz: i32) -> bool {
        let chance = self.chance(lattice, wx, wz);
        FeatureRng::positional(self.seed, lattice.salt ^ CHOICE_SALT, wx, 0, wz).chance(chance)
    }

    /// The claim probability at a site: the blended field mapped through the
    /// lattice's transition band onto its chance range.
    pub(super) fn chance(&mut self, lattice: &GroveLattice, wx: i32, wz: i32) -> f32 {
        let broad = self.sample(lattice.salt, lattice.period, wx, wz);
        let detail = self.sample(
            lattice.salt ^ DETAIL_SALT,
            lattice.period / DETAIL_PERIODS_PER_BROAD,
            wx,
            wz,
        );
        let territory = broad * (1.0 - lattice.detail_weight) + detail * lattice.detail_weight;
        let (lo, hi) = lattice.transition;
        let blend = smooth(((territory - lo) / (hi - lo)).clamp(0.0, 1.0));
        lattice.chance.0 + (lattice.chance.1 - lattice.chance.0) * blend
    }

    /// Bilinear value noise on a `period`-block lattice. Integer lattice
    /// coordinates keep negative and distant sites exact.
    pub(super) fn sample(&mut self, salt: u64, period: i32, wx: i32, wz: i32) -> f32 {
        debug_assert!(
            self.window.contains(wx, wz),
            "grove sample outside the placement window"
        );
        let x = wx.div_euclid(period);
        let z = wz.div_euclid(period);
        let tx = smooth(wx.rem_euclid(period) as f32 / period as f32);
        let tz = smooth(wz.rem_euclid(period) as f32 / period as f32);
        let seed = self.seed;
        let lattice = self.lattice(salt, period);
        let c00 = lattice.corner(seed, x, z);
        let c10 = lattice.corner(seed, x + 1, z);
        let c01 = lattice.corner(seed, x, z + 1);
        let c11 = lattice.corner(seed, x + 1, z + 1);
        let a = c00 + (c10 - c00) * tx;
        let b = c01 + (c11 - c01) * tx;
        a + (b - a) * tz
    }

    fn lattice(&mut self, salt: u64, period: i32) -> &mut LatticeCorners {
        let index = match self
            .lattices
            .iter()
            .position(|l| l.salt == salt && l.period == period)
        {
            Some(i) => i,
            None => {
                self.lattices
                    .push(LatticeCorners::new(salt, period, self.window));
                self.lattices.len() - 1
            }
        };
        &mut self.lattices[index]
    }

    #[cfg(test)]
    pub(super) fn lattice_count(&self) -> usize {
        self.lattices.len()
    }
}

fn smooth(t: f32) -> f32 {
    t * t * (3.0 - 2.0 * t)
}

#[cfg(test)]
mod tests;
