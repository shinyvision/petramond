//! `BiomeSource` — maps a climate sample + surface height to a biome.
//!
//! Strata P2: `CascadeBiomeSource` delegates to the existing `biome::biome_at`
//! ordered cascade, so biome selection is byte-parity. A parameter-space
//! `MultiNoiseBiomeSource` (climate/params.rs) can be added later as an opt-in,
//! screenshot-gated behaviour change without touching the pipeline — an ordered
//! cascade and a nearest-match search have different decision boundaries, so
//! that swap is never sold as parity.

use crate::biome::{Biome, Climate};

pub trait BiomeSource: Send + Sync {
    fn pick(&self, c: &Climate, surf_y: i32) -> Biome;
}

/// The parity path: the exact `biome_at` cascade, moved not reinterpreted.
pub struct CascadeBiomeSource;

impl BiomeSource for CascadeBiomeSource {
    #[inline]
    fn pick(&self, c: &Climate, surf_y: i32) -> Biome {
        crate::biome::biome_at(*c, surf_y)
    }
}

/// Shared zero-sized instance referenced by the driver as `&'static dyn`.
pub static CASCADE: CascadeBiomeSource = CascadeBiomeSource;
