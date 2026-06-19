//! Per-call generation scratch.
//!
//! `ColumnGrid` memoizes the per-column results of the BiomeAssign stage
//! (surface height, biome, river strength) so the fill stages don't resample.
//! It is a stack-local in `ChunkGenerator::generate` — never shared between
//! calls or threads, no interior mutability.

use crate::biome::Biome;
use crate::chunk::{CHUNK_SX, CHUNK_SZ};

const N: usize = CHUNK_SX * CHUNK_SZ;

pub struct ColumnGrid {
    pub surf: [i32; N],
    pub biome: [Biome; N],
    pub river: [f32; N],
    /// Overhang carve amplitude (blocks). 0 => column is a pure heightfield (the
    /// 3-D carve is skipped entirely — every ocean/plains/most foothill column).
    pub overhang_amp: [f32; N],
    /// Inclusive Y band the 3-D carve runs in (below `band_lo` is a hard solid
    /// anchor, above `band_hi` is air) — keeps overhang debris impossible.
    pub band_lo: [i32; N],
    pub band_hi: [i32; N],
}

impl Default for ColumnGrid {
    fn default() -> Self {
        Self {
            surf: [0; N],
            biome: [Biome::Ocean; N],
            river: [0.0; N],
            overhang_amp: [0.0; N],
            band_lo: [0; N],
            band_hi: [0; N],
        }
    }
}
