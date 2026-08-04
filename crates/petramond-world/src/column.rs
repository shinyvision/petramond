//! Per-column (`cx,cz`) data for the cubic-chunks world: the inherently-2D facts
//! shared by every section in a vertical stack — surface biome, the visible
//! surface heightmap, and direct-skylight cover.
//!
//! A [`Column`] is cheap and can be built analytically (biome from the climate
//! classifier, surface height from the density zero-crossing) WITHOUT materializing
//! any [`crate::section::Section`], so the world can ensure it exists the moment any
//! section in the column is touched. See [`crate::chunk::Chunk`] for the column data
//! this replaces (`heightmap`, `biomes`).

use crate::chunk::{CHUNK_SX, CHUNK_SZ, WORLD_MIN_Y};

/// Sentinel height for a column with no matching block at all (e.g. open sky all
/// the way down). One below the world floor so "is there ground?" reads false.
pub const NO_SURFACE: i32 = WORLD_MIN_Y - 1;

/// The 2D per-column data, each a `16×16` grid indexed `z * CHUNK_SX + x`.
pub struct Column {
    /// Highest non-air world Y per `(x,z)` column, or [`NO_SURFACE`] if none. World
    /// Y (so it may be negative), hence `i32` rather than the old `u16`.
    surface_heightmap: Box<[i32; CHUNK_SX * CHUNK_SZ]>,
    /// Highest world Y that interrupts zero-loss direct skylight. Usually the
    /// same as `surface_heightmap`, but clear blocks such as glass and panes
    /// raise the visible surface without raising this cover.
    sky_cover: Box<[i32; CHUNK_SX * CHUNK_SZ]>,
    /// Biome id per `(x,z)` column (`Biome::from_id`).
    biomes: Box<[u8; CHUNK_SX * CHUNK_SZ]>,
    /// Cached `(min, max)` of [`sky_cover`](Self::sky_cover), `None` when a
    /// write has invalidated it. The skylight planner asks nine columns for
    /// this range on EVERY light bake — on the main thread — and recomputing
    /// it means 2304 comparisons per bake over maps that barely ever change.
    sky_cover_range: std::cell::Cell<Option<(i32, i32)>>,
}

impl Default for Column {
    fn default() -> Self {
        Self::new()
    }
}

impl Column {
    pub fn new() -> Self {
        Self {
            surface_heightmap: Box::new([NO_SURFACE; CHUNK_SX * CHUNK_SZ]),
            sky_cover: Box::new([NO_SURFACE; CHUNK_SX * CHUNK_SZ]),
            biomes: Box::new([0u8; CHUNK_SX * CHUNK_SZ]),
            sky_cover_range: std::cell::Cell::new(Some((NO_SURFACE, NO_SURFACE))),
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
    /// non-air block.
    #[inline]
    pub fn surface_y(&self, x: usize, z: usize) -> i32 {
        self.surface_heightmap[z * CHUNK_SX + x]
    }

    #[inline]
    pub fn set_surface_y(&mut self, x: usize, z: usize, wy: i32) {
        self.surface_heightmap[z * CHUNK_SX + x] = wy;
    }

    pub fn surface_heightmap_slice(&self) -> &[i32] {
        &self.surface_heightmap[..]
    }

    /// Highest cell that stops full direct skylight, or [`NO_SURFACE`] when
    /// the column is clear through the loaded world range.
    #[inline]
    pub fn sky_cover_y(&self, x: usize, z: usize) -> i32 {
        self.sky_cover[z * CHUNK_SX + x]
    }

    #[inline]
    pub fn set_sky_cover_y(&mut self, x: usize, z: usize, wy: i32) {
        self.sky_cover[z * CHUNK_SX + x] = wy;
        self.sky_cover_range.set(None);
    }

    pub fn sky_cover_slice(&self) -> &[i32] {
        &self.sky_cover[..]
    }

    /// The `(min, max)` sky-cover height over this column's 256 cells, memoized
    /// until the next write.
    pub fn sky_cover_range(&self) -> (i32, i32) {
        if let Some(range) = self.sky_cover_range.get() {
            return range;
        }
        let (mut lo, mut hi) = (i32::MAX, i32::MIN);
        for &h in self.sky_cover.iter() {
            lo = lo.min(h);
            hi = hi.max(h);
        }
        let range = (lo, hi);
        self.sky_cover_range.set(Some(range));
        range
    }
}
