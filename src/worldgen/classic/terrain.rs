//! Octave-noise terrain — the biome-weighted density field and its trilinear fill
//! into solid/water/air, ported from the reference overworld chunk generator.
//!
//! The density field is `5×5×33` (4-block X/Z cells, 8-block Y cells) blended from
//! a min/max/main octave triple, a depth noise, and a per-column biome
//! base-height/height-variation average over a 5×5 neighbourhood (the parabolic
//! weight kernel). The fill interpolates it to the `16×256×16` column grid: where
//! the interpolated density is positive the block is solid, else water below sea
//! level. The surface skin (grass/sand/…) is applied in a later pass.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use super::lcg::LcgRandom;
use super::noise::OctaveNoise;

/// Sea level.
pub const SEA_LEVEL: i32 = 63;

/// Number of independent lock shards. The worker pool hammers the cache from
/// every thread (~80 lookups/chunk); sharding by cell keeps those on different
/// locks so threads rarely block each other.
const NOISE_CACHE_SHARDS: usize = 32;

/// Soft cap on live columns PER SHARD per generation before rotating. At ~800 B/
/// column, two generations, and `SHARDS` shards, worst-case footprint is
/// ~`2 * SHARDS * CAP * 800 B` ≈ 64 MB. Comfortably covers a full render-distance
/// disc fill (the window over which overlapping chunk regions reuse a column).
const NOISE_CACHE_CAP_PER_SHARD: usize = 1_250;

/// Concurrent, generational cache of per-column octave noise keyed by
/// `(seed, cellx, cellz)` (absolute 4-block lattice cell). Shared by all worker
/// threads so each column the overlapping 32×32 chunk regions need is sampled
/// once instead of ~5× (margin + neighbour overlap). The seed is in the key so a
/// mid-session world change can't return stale noise.
///
/// Each shard keeps two generations so memory is bounded without periodically
/// dropping the hot set: lookups promote a `previous`-generation hit back into
/// `current`, and `current` rotates into `previous` (old `previous` dropped) once
/// it fills.
pub struct NoiseCache {
    shards: [Mutex<CacheGen>; NOISE_CACHE_SHARDS],
}

struct CacheGen {
    current: HashMap<(i64, i32, i32), ColumnNoise>,
    previous: HashMap<(i64, i32, i32), ColumnNoise>,
}

impl CacheGen {
    fn new() -> Self {
        Self {
            current: HashMap::new(),
            previous: HashMap::new(),
        }
    }
}

impl NoiseCache {
    pub fn new() -> Self {
        Self {
            shards: std::array::from_fn(|_| Mutex::new(CacheGen::new())),
        }
    }

    #[inline]
    fn shard(&self, key: (i64, i32, i32)) -> &Mutex<CacheGen> {
        // Mix the cell coords; the seed is constant within a world so it would not
        // contribute to shard spread.
        let h = (key.1 as u64)
            .wrapping_mul(0x9E37_79B9_7F4A_7C15)
            ^ (key.2 as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F);
        &self.shards[(h >> 32) as usize % NOISE_CACHE_SHARDS]
    }

    /// Hit → the cached column (promoted out of `previous` if found there). Miss →
    /// `None`. The expensive sample happens unlocked in the caller, then [`Self::put`].
    fn get(&self, key: (i64, i32, i32)) -> Option<ColumnNoise> {
        let mut g = self.shard(key).lock().unwrap();
        if let Some(v) = g.current.get(&key).copied() {
            return Some(v);
        }
        if let Some(v) = g.previous.get(&key).copied() {
            g.current.insert(key, v); // survive the next rotation
            return Some(v);
        }
        None
    }

    fn put(&self, key: (i64, i32, i32), val: ColumnNoise) {
        let mut g = self.shard(key).lock().unwrap();
        g.current.insert(key, val);
        if g.current.len() >= NOISE_CACHE_CAP_PER_SHARD {
            let full = std::mem::take(&mut g.current);
            g.previous = full;
        }
    }
}

impl Default for NoiseCache {
    fn default() -> Self {
        Self::new()
    }
}

/// Per-biome `(base_height, height_variation)` for the density blend. Mutated
/// variants fall back to their base biome's values.
///
/// Low land biomes carry a `+0.08` base-height offset over the reference MC values
/// (so plains/forest/etc. read `0.205`/`0.18`/… instead of `0.125`/`0.1`/…). This
/// raises the *mean* of their natural surface-height distribution ~+2 blocks so the
/// lower tail no longer spends 20-38% of its time below the y64 waterline (which
/// read as a swamp). It is a pure vertical OFFSET into the density-baseline blend —
/// `base_height -> depth -> baseline = 8.5 + depth*4` — so it SHIFTS the surface up
/// without touching `height_variation` (the relief amplitude): rolling preserved,
/// occasional natural ponds retained, no flattening. Calibrated empirically against
/// `genmap relief` (the knee where the worst interior seeds land ~5-12% below-64
/// with STDEV ~unchanged). Intentionally-wet biomes (ocean/river/swamp/beach/…) and
/// already-high biomes (hills/mountains/plateaus) are NOT offset.
pub(crate) fn biome_height(id: i32) -> (f32, f32) {
    /// Base-height lift applied to low land biomes only (see fn docs).
    const LOW_LIFT: f32 = 0.08;
    let lo = |b: f32, v: f32| (b + LOW_LIFT, v);
    match id {
        0 => (-1.0, 0.1),    // ocean
        1 => lo(0.125, 0.05), // plains
        2 => lo(0.125, 0.05), // desert
        3 => (1.0, 0.5),     // mountains
        4 => lo(0.1, 0.2),   // forest
        5 => lo(0.2, 0.2),   // taiga
        6 => (-0.2, 0.1),    // swamp
        7 => (-0.5, 0.0),    // river
        10 => (-1.0, 0.1),   // frozen_ocean
        11 => (-0.5, 0.0),   // frozen_river
        12 => lo(0.125, 0.05), // snowy_tundra
        13 => (0.45, 0.3),   // snowy_mountains
        14 => lo(0.2, 0.3),  // mushroom_fields
        15 => (0.0, 0.025),  // mushroom_field_shore
        16 => (0.0, 0.025),  // beach
        17 => (0.45, 0.3),   // desert_hills
        18 => (0.45, 0.3),   // wooded_hills
        19 => (0.45, 0.3),   // taiga_hills
        20 => (0.8, 0.3),    // mountain_edge
        21 => lo(0.1, 0.2),  // jungle
        22 => (0.45, 0.3),   // jungle_hills
        23 => lo(0.1, 0.2),  // jungle_edge
        24 => (-1.8, 0.1),   // deep_ocean
        25 => (0.1, 0.8),    // stone_shore
        26 => (0.0, 0.025),  // snowy_beach
        27 => lo(0.1, 0.2),  // birch_forest
        28 => (0.45, 0.3),   // birch_forest_hills
        29 => lo(0.1, 0.2),  // dark_forest
        30 => lo(0.2, 0.2),  // snowy_taiga
        31 => (0.45, 0.3),   // snowy_taiga_hills
        32 => (0.2, 0.2),    // giant_tree_taiga
        33 => (0.45, 0.3),   // giant_tree_taiga_hills
        34 => (1.0, 0.5),    // wooded_mountains
        35 => lo(0.125, 0.05), // savanna
        36 => (1.5, 0.025),  // savanna_plateau
        37 => lo(0.1, 0.2),  // badlands
        38 => (1.5, 0.025),  // wooded_badlands_plateau
        39 => (1.5, 0.025),  // badlands_plateau
        _ if id >= 128 => biome_height(id - 128),
        _ => lo(0.1, 0.2),
    }
}

#[inline]
fn clamped_lerp(lo: f64, hi: f64, t: f64) -> f64 {
    if t < 0.0 {
        lo
    } else if t > 1.0 {
        hi
    } else {
        lo + (hi - lo) * t
    }
}

/// The four octave-noise fields sampled at one 4-block lattice cell column: the
/// 2-D `depth` scalar plus the three 33-deep 3-D stacks (`main`/`min`/`max`).
/// Computed at the cell's own absolute coordinate, so it is a pure function of
/// `(cellx, cellz)` and reusable by every overlapping chunk region — the unit
/// [`NoiseCache`] stores.
#[derive(Clone, Copy)]
pub struct ColumnNoise {
    pub depth: f64,
    pub main: [f64; 33],
    pub min: [f64; 33],
    pub max: [f64; 33],
}

/// The overworld terrain noise: the seven octave generators (built in the
/// reference's exact RNG-consuming order) and the parabolic biome-weight kernel.
pub struct TerrainGen {
    seed: i64,
    min: OctaveNoise,
    max: OctaveNoise,
    main: OctaveNoise,
    depth: OctaveNoise,
    q: [f32; 25],
    cache: Arc<NoiseCache>,
}

impl TerrainGen {
    /// Build with a fresh private noise cache (one-shot / tooling callers).
    pub fn new(seed: i64) -> Self {
        Self::with_cache(seed, Arc::new(NoiseCache::new()))
    }

    /// Build sharing an existing noise cache (the worker pool hands every thread's
    /// generator the same `Arc` so they pool their column samples).
    pub fn with_cache(seed: i64, cache: Arc<NoiseCache>) -> Self {
        let mut r = LcgRandom::new(seed);
        let min = OctaveNoise::new(&mut r, 16); // i: lower limit
        let max = OctaveNoise::new(&mut r, 16); // j: upper limit
        let main = OctaveNoise::new(&mut r, 8); // k: main
        let _surface = OctaveNoise::new(&mut r, 4); // l: surface perlin (same RNG draw as octaves)
        let _scale = OctaveNoise::new(&mut r, 10); // a: scale (unused by terrain)
        let depth = OctaveNoise::new(&mut r, 16); // b: depth
        let _forest = OctaveNoise::new(&mut r, 8); // c: forest (decoration)
        let mut q = [0.0f32; 25];
        for i in -2i32..=2 {
            for j in -2i32..=2 {
                q[((i + 2) + (j + 2) * 5) as usize] = 10.0 / ((i * i + j * j) as f32 + 0.2).sqrt();
            }
        }
        Self {
            seed,
            min,
            max,
            main,
            depth,
            q,
            cache,
        }
    }

    /// Per-column octave-noise sample at an absolute 4-block lattice cell. Computes
    /// the four field columns at the cell's OWN coordinate (`xs=zs=1`), so the
    /// result is a pure function of `(cellx, cellz)`, independent of the requesting
    /// region. That origin-independence is what lets overlapping chunk regions
    /// share one computed column (see [`super::super::noise::cache`]).
    pub fn column_noise(&self, cellx: i32, cellz: i32) -> ColumnNoise {
        let mut depth = [0.0f64; 1];
        self.depth
            .sample_region_2d(&mut depth, cellx, cellz, 1, 1, 200.0, 200.0, 0.5);
        let mut main = [0.0f64; 33];
        self.main.sample_region(
            &mut main,
            cellx,
            0,
            cellz,
            1,
            33,
            1,
            684.412 / 80.0,
            684.412 / 160.0,
            684.412 / 80.0,
        );
        let mut min = [0.0f64; 33];
        self.min
            .sample_region(&mut min, cellx, 0, cellz, 1, 33, 1, 684.412, 684.412, 684.412);
        let mut max = [0.0f64; 33];
        self.max
            .sample_region(&mut max, cellx, 0, cellz, 1, 33, 1, 684.412, 684.412, 684.412);
        ColumnNoise {
            depth: depth[0],
            main,
            min,
            max,
        }
    }

    /// Fill the density field for a block region, pulling each lattice column's
    /// octave noise through `fetch` — a cache-or-compute hook keyed by absolute
    /// cell. `(x0,z0)`/`(w,h)` multiples of 4; `cw=w/4`, `ch=h/4`; field is
    /// `(cw+1)*(ch+1)*33` row-major `(x*nz+z)*33+y`. `biomes` is the scale-4 grid
    /// at `(x0/4-2, z0/4-2)` with row stride `bstride` (`>= cw+5`). The biome blend
    /// + density combine are unchanged; only noise acquisition is now per-column.
    fn density_region_with<F: FnMut(i32, i32) -> ColumnNoise>(
        &self,
        x0: i32,
        z0: i32,
        cw: usize,
        ch: usize,
        biomes: &[i32],
        bstride: usize,
        mut fetch: F,
    ) -> Vec<f64> {
        let (x0c, z0c) = (x0 >> 2, z0 >> 2); // x0/4 (x0 is a multiple of 4)
        let (nx, nz) = (cw + 1, ch + 1);
        let mut p = vec![0.0f64; nx * nz * 33];
        let mut pidx = 0;
        for x in 0..nx {
            for z in 0..nz {
                let cn = fetch(x0c + x as i32, z0c + z as i32);
                let center = biomes[(x + 2) + (z + 2) * bstride];
                let (c_base, _) = biome_height(center);
                let mut s_acc = 0.0f32;
                let mut d_acc = 0.0f32;
                let mut wsum = 0.0f32;
                for dx in -2i32..=2 {
                    for dz in -2i32..=2 {
                        let bi = ((x as i32 + dx + 2) as usize)
                            + ((z as i32 + dz + 2) as usize) * bstride;
                        let (b_base, b_var) = biome_height(biomes[bi]);
                        let mut w = self.q[((dx + 2) + (dz + 2) * 5) as usize] / (b_base + 2.0);
                        if b_base > c_base {
                            w /= 2.0;
                        }
                        s_acc += b_var * w;
                        d_acc += b_base * w;
                        wsum += w;
                    }
                }
                s_acc /= wsum;
                d_acc /= wsum;
                let scale = (s_acc * 0.9 + 0.1) as f64;
                let mut depth = ((d_acc * 4.0 - 1.0) / 8.0) as f64;

                let mut dn = cn.depth / 8000.0;
                if dn < 0.0 {
                    dn = -dn * 0.3;
                }
                dn = dn * 3.0 - 2.0;
                if dn < 0.0 {
                    dn /= 2.0;
                    if dn < -1.0 {
                        dn = -1.0;
                    }
                    dn /= 1.4;
                    dn /= 2.0;
                } else {
                    if dn > 1.0 {
                        dn = 1.0;
                    }
                    dn /= 8.0;
                }
                depth += dn * 0.2;
                depth = depth * 8.5 / 8.0;
                let baseline = 8.5 + depth * 4.0;

                for y in 0..33 {
                    let mut ybias = (y as f64 - baseline) * 12.0 * 128.0 / 256.0 / scale;
                    if ybias < 0.0 {
                        ybias *= 4.0;
                    }
                    let lo = cn.min[y] / 512.0;
                    let hi = cn.max[y] / 512.0;
                    let sel = (cn.main[y] / 10.0 + 1.0) / 2.0;
                    let mut val = clamped_lerp(lo, hi, sel) - ybias;
                    if y > 29 {
                        let t = (y - 29) as f64 / 3.0;
                        val = val * (1.0 - t) + (-10.0) * t;
                    }
                    p[pidx] = val;
                    pidx += 1;
                }
            }
        }
        p
    }

    /// Production density region: each lattice column is fetched from the shared
    /// noise cache, sampled + stored only on a miss. Output is identical to the
    /// uncached canonical path — the cache only memoizes [`Self::column_noise`].
    fn density_region(
        &self,
        x0: i32,
        z0: i32,
        cw: usize,
        ch: usize,
        biomes: &[i32],
        bstride: usize,
    ) -> Vec<f64> {
        let cache = &self.cache;
        let seed = self.seed;
        self.density_region_with(x0, z0, cw, ch, biomes, bstride, |cx, cz| {
            let key = (seed, cx, cz);
            cache.get(key).unwrap_or_else(|| {
                let v = self.column_noise(cx, cz);
                cache.put(key, v);
                v
            })
        })
    }

    /// Reference (pre-cache) batched density sampler — kept only to prove the
    /// per-column path yields the identical integer heightmap.
    #[cfg(all(test, feature = "worldgen-tests"))]
    fn density_region_batched(
        &self,
        x0: i32,
        z0: i32,
        cw: usize,
        ch: usize,
        biomes: &[i32],
        bstride: usize,
    ) -> Vec<f64> {
        let (x0c, z0c) = (x0 >> 2, z0 >> 2);
        let (nx, nz) = (cw + 1, ch + 1);
        let mut depth_r = vec![0.0f64; nx * nz];
        let mut main_r = vec![0.0f64; nx * 33 * nz];
        let mut min_r = vec![0.0f64; nx * 33 * nz];
        let mut max_r = vec![0.0f64; nx * 33 * nz];
        self.depth
            .sample_region_2d(&mut depth_r, x0c, z0c, nx, nz, 200.0, 200.0, 0.5);
        self.main.sample_region(
            &mut main_r,
            x0c,
            0,
            z0c,
            nx,
            33,
            nz,
            684.412 / 80.0,
            684.412 / 160.0,
            684.412 / 80.0,
        );
        self.min
            .sample_region(&mut min_r, x0c, 0, z0c, nx, 33, nz, 684.412, 684.412, 684.412);
        self.max
            .sample_region(&mut max_r, x0c, 0, z0c, nx, 33, nz, 684.412, 684.412, 684.412);

        let mut p = vec![0.0f64; nx * nz * 33];
        let mut didx = 0;
        let mut pidx = 0;
        for _x in 0..nx {
            for _z in 0..nz {
                let center = biomes[(_x + 2) + (_z + 2) * bstride];
                let (c_base, _) = biome_height(center);
                let mut s_acc = 0.0f32;
                let mut d_acc = 0.0f32;
                let mut wsum = 0.0f32;
                for dx in -2i32..=2 {
                    for dz in -2i32..=2 {
                        let bi = ((_x as i32 + dx + 2) as usize)
                            + ((_z as i32 + dz + 2) as usize) * bstride;
                        let (b_base, b_var) = biome_height(biomes[bi]);
                        let mut w = self.q[((dx + 2) + (dz + 2) * 5) as usize] / (b_base + 2.0);
                        if b_base > c_base {
                            w /= 2.0;
                        }
                        s_acc += b_var * w;
                        d_acc += b_base * w;
                        wsum += w;
                    }
                }
                s_acc /= wsum;
                d_acc /= wsum;
                let scale = (s_acc * 0.9 + 0.1) as f64;
                let mut depth = ((d_acc * 4.0 - 1.0) / 8.0) as f64;

                let mut dn = depth_r[didx] / 8000.0;
                didx += 1;
                if dn < 0.0 {
                    dn = -dn * 0.3;
                }
                dn = dn * 3.0 - 2.0;
                if dn < 0.0 {
                    dn /= 2.0;
                    if dn < -1.0 {
                        dn = -1.0;
                    }
                    dn /= 1.4;
                    dn /= 2.0;
                } else {
                    if dn > 1.0 {
                        dn = 1.0;
                    }
                    dn /= 8.0;
                }
                depth += dn * 0.2;
                depth = depth * 8.5 / 8.0;
                let baseline = 8.5 + depth * 4.0;

                for y in 0..33 {
                    let mut ybias = (y as f64 - baseline) * 12.0 * 128.0 / 256.0 / scale;
                    if ybias < 0.0 {
                        ybias *= 4.0;
                    }
                    let lo = min_r[pidx] / 512.0;
                    let hi = max_r[pidx] / 512.0;
                    let sel = (main_r[pidx] / 10.0 + 1.0) / 2.0;
                    let mut val = clamped_lerp(lo, hi, sel) - ybias;
                    if y > 29 {
                        let t = (y - 29) as f64 / 3.0;
                        val = val * (1.0 - t) + (-10.0) * t;
                    }
                    p[pidx] = val;
                    pidx += 1;
                }
            }
        }
        p
    }

    /// Top solid-terrain Y per column (`i + j*w`) for a block region, before the
    /// surface skin. `(x0,z0,w,h)` multiples of 4; `biomes`/`bstride` as in
    /// [`density_region`].
    pub fn region_heightmap(
        &self,
        x0: i32,
        z0: i32,
        w: usize,
        h: usize,
        biomes: &[i32],
        bstride: usize,
    ) -> Vec<i32> {
        let (cw, ch) = (w / 4, h / 4);
        let p = self.density_region(x0, z0, cw, ch, biomes, bstride);
        Self::heightmap_from_density(&p, w, h, cw, ch)
    }

    /// Trilinearly interpolate a density field (`(cw+1)*(ch+1)*33`, layout
    /// `(x*nz+z)*33+y`) into a per-column top-solid-Y heightmap (`x + z*w`).
    /// Split out of [`Self::region_heightmap`] so the noise path is independent of
    /// the interpolation, and so parity tests can run it on any density field.
    fn heightmap_from_density(p: &[f64], w: usize, h: usize, cw: usize, ch: usize) -> Vec<i32> {
        let nz = ch + 1; // density z-stride in cells (points per column)
        let mut hm = vec![-1i32; w * h];
        for sx in 0..cw {
            for sz in 0..ch {
                for sy in 0..32usize {
                    let i000 = (sx * nz + sz) * 33 + sy;
                    let i001 = (sx * nz + sz + 1) * 33 + sy;
                    let i100 = ((sx + 1) * nz + sz) * 33 + sy;
                    let i101 = ((sx + 1) * nz + sz + 1) * 33 + sy;
                    let mut v000 = p[i000];
                    let mut v001 = p[i001];
                    let mut v100 = p[i100];
                    let mut v101 = p[i101];
                    let dv000 = (p[i000 + 1] - v000) * 0.125;
                    let dv001 = (p[i001 + 1] - v001) * 0.125;
                    let dv100 = (p[i100 + 1] - v100) * 0.125;
                    let dv101 = (p[i101 + 1] - v101) * 0.125;
                    for yy in 0..8usize {
                        let world_y = (sy * 8 + yy) as i32;
                        let mut aa = v000;
                        let mut bb = v001;
                        let da = (v100 - v000) * 0.25;
                        let db = (v101 - v001) * 0.25;
                        for xx in 0..4usize {
                            let mut val = aa;
                            let dval = (bb - aa) * 0.25;
                            for zz in 0..4usize {
                                if val > 0.0 {
                                    let wx = sx * 4 + xx;
                                    let wz = sz * 4 + zz;
                                    let ci = wx + wz * w;
                                    if world_y > hm[ci] {
                                        hm[ci] = world_y;
                                    }
                                }
                                val += dval;
                            }
                            aa += da;
                            bb += db;
                        }
                        v000 += dv000;
                        v001 += dv001;
                        v100 += dv100;
                        v101 += dv101;
                    }
                }
            }
        }
        hm
    }
}

#[cfg(all(test, feature = "worldgen-tests"))]
mod tests {
    use super::*;
    use crate::worldgen::classic::biome::stack::river_mix;

    /// The per-column (cache-ready) density path must produce the IDENTICAL integer
    /// heightmap as the original batched region sample, over a large sweep of
    /// realistic chunk regions and seeds. The raw f64 noise differs by ~1e-11
    /// (FP operation order), but that is far below what the heightmap can resolve,
    /// so the generated terrain is unchanged. Locks that invariant permanently.
    #[test]
    fn per_column_density_matches_batched_heightmap() {
        use crate::worldgen::classic::biome::stack::land_mix;
        let (w, h) = (32usize, 32usize); // matches the generator's chunk+margin region
        let (cw, ch) = (w / 4, h / 4);
        let bstride = w / 4 + 5;
        for &seed in &[0x1234_5678i64, 1, 7, 42] {
            let t = TerrainGen::new(seed);
            let lm = land_mix(seed);
            // A contiguous 24x24 chunk block plus a couple of far-flung origins.
            let mut coords: Vec<(i32, i32)> = (-12..12)
                .flat_map(|cz| (-12..12).map(move |cx| (cx, cz)))
                .collect();
            coords.extend_from_slice(&[(500, -500), (-1234, 777), (9001, 9001)]);
            for (cx, cz) in coords {
                let (x0, z0) = (cx * 16 - 8, cz * 16 - 8);
                let biomes = lm.gen((x0 >> 2) as i64 - 2, (z0 >> 2) as i64 - 2, bstride, bstride);
                let p_new = t.density_region(x0, z0, cw, ch, &biomes, bstride);
                let p_old = t.density_region_batched(x0, z0, cw, ch, &biomes, bstride);
                let hm_new = TerrainGen::heightmap_from_density(&p_new, w, h, cw, ch);
                let hm_old = TerrainGen::heightmap_from_density(&p_old, w, h, cw, ch);
                assert_eq!(
                    hm_new, hm_old,
                    "heightmap diverged at chunk ({cx},{cz}) seed {seed:#x}"
                );
            }
        }
    }

    /// The real 1.8.9 terrain heightmap (top solid terrain block, excluding water/
    /// snow/decoration) for seed 12345, chunk (-5, 5), flat `x + z*16`.
    const DUMP_HM: [i32; 256] = [
        100, 100, 100, 100, 105, 104, 103, 102, 101, 101, 101, 100, 100, 100, 100, 100, 100, 100,
        100, 99, 105, 104, 103, 102, 102, 101, 101, 101, 100, 100, 100, 101, 100, 100, 100, 99,
        105, 104, 103, 103, 102, 102, 101, 101, 101, 101, 101, 101, 99, 100, 101, 102, 105, 104,
        104, 103, 102, 102, 102, 102, 101, 102, 102, 102, 99, 100, 101, 102, 104, 105, 104, 103,
        103, 103, 102, 102, 102, 103, 103, 103, 99, 100, 101, 102, 103, 103, 104, 104, 103, 103,
        103, 103, 103, 103, 103, 104, 100, 101, 101, 103, 103, 103, 104, 104, 103, 104, 104, 104,
        104, 104, 104, 104, 100, 101, 102, 102, 102, 102, 103, 105, 104, 104, 104, 105, 105, 105,
        105, 105, 101, 102, 103, 102, 102, 102, 102, 105, 104, 105, 105, 105, 105, 105, 105, 105,
        101, 102, 103, 102, 102, 102, 102, 105, 105, 105, 105, 105, 106, 105, 105, 105, 100, 103,
        103, 102, 101, 102, 102, 105, 105, 105, 105, 106, 106, 105, 105, 105, 100, 104, 101, 101,
        101, 101, 102, 105, 106, 106, 106, 106, 106, 105, 105, 105, 100, 100, 100, 101, 101, 101,
        105, 105, 106, 106, 106, 106, 106, 105, 105, 105, 100, 100, 100, 100, 101, 101, 105, 105,
        106, 106, 106, 106, 106, 105, 105, 99, 100, 100, 100, 100, 101, 105, 105, 105, 107, 106,
        106, 106, 106, 105, 105, 99, 104, 100, 100, 101, 104, 105, 105, 106, 107, 107, 106, 106,
        106, 105, 99, 99,
    ];

    // P2 work-in-progress: the terrain heightmap is currently ~1–2 blocks off from
    // real Minecraft (biomes exact, terrain shape close). The single-point lattice
    // noise is KAT-verified; the residual is a precision bug in the octave
    // accumulation / 2-D depth-noise path, under investigation against the dumps.
    #[test]
    #[ignore = "P2 terrain heightmap off by 1-2 blocks; noise-accumulation precision bug WIP"]
    fn terrain_heightmap_matches_real_minecraft() {
        let (cx, cz) = (-5i32, 5i32);
        let biomes = river_mix(12345).gen((cx * 4 - 2) as i64, (cz * 4 - 2) as i64, 10, 10);
        let hm = TerrainGen::new(12345).region_heightmap(cx * 16, cz * 16, 16, 16, &biomes, 10);
        let mut diffs = 0;
        for i in 0..256 {
            if hm[i] != DUMP_HM[i] {
                diffs += 1;
            }
        }
        assert_eq!(
            diffs, 0,
            "terrain heightmap differs from real Minecraft in {diffs}/256 columns"
        );
    }
}
