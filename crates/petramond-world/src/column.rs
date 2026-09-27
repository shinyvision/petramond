use std::sync::atomic::{AtomicU64, Ordering};

use crate::chunk::{CHUNK_SX, CHUNK_SZ, WORLD_MIN_Y};

pub const NO_SURFACE: i32 = WORLD_MIN_Y - 1;

pub struct Column {
    surface_heightmap: Box<[i32; CHUNK_SX * CHUNK_SZ]>,
    sky_cover: Box<[i32; CHUNK_SX * CHUNK_SZ]>,
    biomes: Box<[u8; CHUNK_SX * CHUNK_SZ]>,
    sky_cover_range: AtomicU64,
}

const RANGE_STALE: u64 = pack_range(0, -1);

const fn pack_range(lo: i32, hi: i32) -> u64 {
    ((lo as u32 as u64) << 32) | hi as u32 as u64
}

impl Clone for Column {
    fn clone(&self) -> Self {
        Self {
            surface_heightmap: self.surface_heightmap.clone(),
            sky_cover: self.sky_cover.clone(),
            biomes: self.biomes.clone(),
            sky_cover_range: AtomicU64::new(self.sky_cover_range.load(Ordering::Relaxed)),
        }
    }
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
            sky_cover_range: AtomicU64::new(pack_range(NO_SURFACE, NO_SURFACE)),
        }
    }

    #[inline]
    pub fn biome_at(&self, x: usize, z: usize) -> u8 {
        self.biomes[z * CHUNK_SX + x]
    }

    #[inline]
    pub fn set_biome(&mut self, x: usize, z: usize, b: u8) {
        self.biomes[z * CHUNK_SX + x] = b;
    }

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

    #[inline]
    pub fn sky_cover_y(&self, x: usize, z: usize) -> i32 {
        self.sky_cover[z * CHUNK_SX + x]
    }

    #[inline]
    pub fn set_sky_cover_y(&mut self, x: usize, z: usize, wy: i32) {
        self.sky_cover[z * CHUNK_SX + x] = wy;
        *self.sky_cover_range.get_mut() = RANGE_STALE;
    }

    pub fn sky_cover_slice(&self) -> &[i32] {
        &self.sky_cover[..]
    }

    pub fn sky_cover_range(&self) -> (i32, i32) {
        let packed = self.sky_cover_range.load(Ordering::Relaxed);
        if packed != RANGE_STALE {
            return ((packed >> 32) as u32 as i32, packed as u32 as i32);
        }
        let (mut lo, mut hi) = (i32::MAX, i32::MIN);
        for &h in self.sky_cover.iter() {
            lo = lo.min(h);
            hi = hi.max(h);
        }
        self.sky_cover_range
            .store(pack_range(lo, hi), Ordering::Relaxed);
        (lo, hi)
    }
}
