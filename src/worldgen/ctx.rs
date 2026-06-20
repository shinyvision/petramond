//! Per-call generation scratch.
//!
//! `ColumnGrid` memoizes the per-column results of the BiomeAssign stage (surface
//! height + biome) so the fill stage doesn't resample. It is a stack-local in
//! `ChunkGenerator::generate` — never shared between calls or threads, no interior
//! mutability.

use crate::biome::Biome;
use crate::chunk::{CHUNK_SX, CHUNK_SZ};

const N: usize = CHUNK_SX * CHUNK_SZ;

pub struct ColumnGrid {
    pub surf: [i32; N],
    pub biome: [Biome; N],
}

impl Default for ColumnGrid {
    fn default() -> Self {
        Self {
            surf: [0; N],
            biome: [Biome::Ocean; N],
        }
    }
}
