//! Per-column (`cx,cz`) data for the cubic-chunks world: the inherently-2D facts
//! shared by every section in a vertical stack — surface biome and the surface
//! heightmap (and, later, sky-occlusion bookkeeping for cubic skylight).
//!
//! A [`Column`] is cheap and can be built analytically (biome from the climate
//! classifier, surface height from the density zero-crossing) WITHOUT materializing
//! any [`crate::section::Section`], so the world can ensure it exists the moment any
//! section in the column is touched. See [`crate::chunk::Chunk`] for the column data
//! this replaces (`heightmap`, `biomes`).

use crate::chunk::{CHUNK_SX, CHUNK_SZ, WORLD_MIN_Y};

/// Sentinel surface height for a column with no solid block at all (e.g. open sky
/// all the way down). One below the world floor so "is there ground?" reads false.
pub const NO_SURFACE: i32 = WORLD_MIN_Y - 1;

/// The 2D per-column data: surface heightmap + biome id, each a `16×16` grid
/// indexed `z * CHUNK_SX + x`.
pub struct Column {
    /// Highest non-air world Y per `(x,z)` column, or [`NO_SURFACE`] if none. World
    /// Y (so it may be negative), hence `i32` rather than the old `u16`.
    heightmap: Box<[i32; CHUNK_SX * CHUNK_SZ]>,
    /// Biome id per `(x,z)` column (`Biome::from_id`).
    biomes: Box<[u8; CHUNK_SX * CHUNK_SZ]>,
}

impl Column {
    pub fn new() -> Self {
        Self {
            heightmap: Box::new([NO_SURFACE; CHUNK_SX * CHUNK_SZ]),
            biomes: Box::new([0u8; CHUNK_SX * CHUNK_SZ]),
        }
    }

    // --- Biome ------------------------------------------------------------------

    #[inline]
    pub fn biome_at(&self, x: usize, z: usize) -> u8 {
        self.biomes[z * CHUNK_SX + x]
    }

    #[inline]
    pub fn set_biome(&mut self, x: usize, z: usize, b: u8) {
        self.biomes[z * CHUNK_SX + x] = b;
    }

    // --- Surface heightmap ------------------------------------------------------

    /// Highest non-air world Y at `(x,z)`, or [`NO_SURFACE`] if the column has no
    /// solid block.
    #[inline]
    pub fn surface_y(&self, x: usize, z: usize) -> i32 {
        self.heightmap[z * CHUNK_SX + x]
    }

    #[inline]
    pub fn set_surface_y(&mut self, x: usize, z: usize, wy: i32) {
        self.heightmap[z * CHUNK_SX + x] = wy;
    }

    pub fn heightmap_slice(&self) -> &[i32] {
        &self.heightmap[..]
    }

    /// Raise the surface to `wy` if a solid block was just placed above the current
    /// surface. Cheap incremental update for block placement; lowering (breaking the
    /// top block) needs a downward rescan the world performs with section access.
    #[inline]
    pub fn raise_surface(&mut self, x: usize, z: usize, wy: i32) {
        let i = z * CHUNK_SX + x;
        if wy > self.heightmap[i] {
            self.heightmap[i] = wy;
        }
    }
}
