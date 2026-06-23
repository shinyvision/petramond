//! Biome-driven world provider: the bridge from the verified layered biome
//! cascade to the game's chunk generator.
//!
//! For a chunk it yields (1) the per-block land biome and (2) a base surface height
//! shaped FROM those biomes. Active rivers are carved later by `worldgen::river`
//! as explicit path objects instead of being smeared through the classic river
//! biome overlay.

use crate::biome::Biome;
use crate::worldgen::river::RiverColumn;

use std::sync::Arc;

use super::biome::layers::Layer;
use super::biome::stack::{land_mix, land_voronoi};
use super::terrain::{NoiseCache, TerrainGen};

const CHUNK: usize = 16;

/// Biomes whose water is intended (ocean, deep ocean, river, frozen ocean/river,
/// mushroom shore, beach, snowy beach) and so whose sub-sea surface is genuine
/// water, not land that happened to fall below the waterline. Mutated `+128`
/// variants fold to their base. Used by tooling (`genmap relief`) to separate
/// land-biome relief from intended-wet columns.
#[inline]
pub fn keep_wet(biome_id: i32) -> bool {
    let base = if biome_id >= 128 {
        biome_id - 128
    } else {
        biome_id
    };
    matches!(base, 0 | 6 | 7 | 10 | 11 | 15 | 16 | 24 | 26)
}

/// One chunk's biome-driven terrain: top-solid surface height and per-block MC
/// biome id, both row-major `x + z*16`.
pub struct ChunkCells {
    pub surf: [i32; 256],
    pub biome_ids: [i32; 256],
}

/// Owns the per-seed cascade layers + terrain noise (built once, reused per chunk).
pub struct CascadeWorld {
    terrain: TerrainGen,
    land_voronoi: Box<dyn Layer>,
    land_mix: Box<dyn Layer>,
}

impl CascadeWorld {
    pub fn new(seed: u32) -> Self {
        Self::with_cache(seed, Arc::new(NoiseCache::new()))
    }

    /// As [`Self::new`] but the terrain generator shares an external noise cache,
    /// so several generators (one per worker thread) pool their column samples.
    pub fn with_cache(seed: u32, cache: Arc<NoiseCache>) -> Self {
        let s = seed as i64;
        Self {
            terrain: TerrainGen::with_cache(s, cache),
            land_voronoi: land_voronoi(s),
            land_mix: land_mix(s),
        }
    }

    /// One chunk's biomes + biome-driven height (see [`Self::region`]).
    pub fn chunk(&self, cx: i32, cz: i32) -> ChunkCells {
        let r = self.region(cx * 16, cz * 16, CHUNK, CHUNK);
        let mut surf = [0i32; 256];
        let mut biome_ids = [0i32; 256];
        surf.copy_from_slice(&r.surf);
        biome_ids.copy_from_slice(&r.biome_ids);
        ChunkCells { surf, biome_ids }
    }

    /// Cheap land-biome lookup for non-authoritative fallbacks such as fog before
    /// a chunk column is loaded. Loaded chunk biome bytes remain authoritative.
    pub fn biome_at(&self, wx: i32, wz: i32) -> Biome {
        map_biome(self.land_voronoi.gen(wx as i64, wz as i64, 1, 1)[0])
    }

    /// Generate a block region's per-block land biomes and base surface height
    /// (row-major `i + j*w`) in a SINGLE cascade pass — one land-voronoi gen, one
    /// land-mix gen, one terrain region fill. Rivers are applied after this base
    /// region is built. `(x0,z0,w,h)` must be multiples of 4.
    pub fn region(&self, x0: i32, z0: i32, w: usize, h: usize) -> RegionCells {
        let land = self.land_voronoi.gen(x0 as i64, z0 as i64, w, h);
        // Scale-4 biome grid for the terrain blend: origin (x0/4 - 2), stride cw+5.
        let (cw, ch) = (w / 4, h / 4);
        let bstride = cw + 5;
        let rm = self
            .land_mix
            .gen((x0 >> 2) as i64 - 2, (z0 >> 2) as i64 - 2, bstride, ch + 5);
        let surf = self.terrain.region_heightmap(x0, z0, w, h, &rm, bstride);
        let mut biome_ids = vec![0i32; w * h];
        for z in 0..h {
            for x in 0..w {
                let i = z * w + x;
                biome_ids[i] = land[i];
            }
        }
        RegionCells {
            x0,
            z0,
            w,
            h,
            surf,
            biome_ids,
            rivers: vec![RiverColumn::default(); w * h],
        }
    }
}

/// A block region's biome-driven terrain. `surf`/`biome_ids` are row-major
/// `(wx-x0) + (wz-z0)*w`; use [`RegionCells::at`] for world-coordinate lookups.
pub struct RegionCells {
    pub x0: i32,
    pub z0: i32,
    pub w: usize,
    pub h: usize,
    pub surf: Vec<i32>,
    pub biome_ids: Vec<i32>,
    pub rivers: Vec<RiverColumn>,
}

impl RegionCells {
    #[inline]
    pub fn at(&self, wx: i32, wz: i32) -> (i32, i32) {
        let i = (wz - self.z0) as usize * self.w + (wx - self.x0) as usize;
        (self.surf[i], self.biome_ids[i])
    }

    #[inline]
    pub fn river_at(&self, wx: i32, wz: i32) -> RiverColumn {
        let i = (wz - self.z0) as usize * self.w + (wx - self.x0) as usize;
        self.rivers[i]
    }
}

/// Map a Minecraft 1.8 biome id (base `0..39` or a `+128` mutated variant) onto the
/// game's [`Biome`] vocabulary used for surface skin, features and colour. Variants
/// fold to their base biome; biomes with no close game equivalent map to the
/// nearest match.
pub fn map_biome(id: i32) -> Biome {
    use Biome::*;
    let base = if id >= 128 { id - 128 } else { id };
    match base {
        0 => Ocean,
        1 => Plains,
        2 => Desert,
        3 => Mountains, // extreme hills
        4 => Forest,
        5 => Taiga,
        6 => Swamp,
        7 => River,
        10 => Ocean, // frozen ocean (no frozen-ocean game biome)
        11 => River, // frozen river
        12 => SnowyTundra,
        13 => SnowyPeaks, // snowy mountains
        14 => MushroomFields,
        15 => MushroomFields, // mushroom shore
        16 => Beach,
        17 => Desert,    // desert hills
        18 => Forest,    // wooded hills
        19 => Taiga,     // taiga hills
        20 => Foothills, // mountain edge
        21 => Jungle,
        22 => Jungle, // jungle hills
        23 => Jungle, // jungle edge
        24 => DeepOcean,
        25 => StonyPeaks, // stone shore (rocky coast)
        26 => Beach,      // snowy beach
        27 => BirchForest,
        28 => BirchForest, // birch forest hills
        29 => DarkForest,
        30 => SnowyTaiga,
        31 => SnowyTaiga,     // snowy taiga hills
        32 => OldGrowthTaiga, // giant tree taiga
        33 => OldGrowthTaiga, // giant tree taiga hills
        34 => Mountains,      // wooded mountains
        35 => Savanna,
        36 => Savanna, // savanna plateau
        37 => Badlands,
        38 => Badlands, // wooded badlands plateau
        39 => Badlands, // badlands plateau
        _ => Plains,
    }
}

#[cfg(all(test, feature = "worldgen-tests"))]
mod tests {
    use super::*;
    use crate::worldgen::classic::biome::stack::voronoi;

    /// Biome-province invariants on the SHIPPING terrain path. Samples the live
    /// `CascadeWorld` region (the same provider the chunk generator reads) on a
    /// stride-8 grid and asserts the world stays a varied mix rather than one
    /// dominant biome: many distinct biomes coexist with no single one swallowing
    /// the window, a large connected desert exists AND reaches high ground (hills /
    /// plateaus), and at least one swamp forms well inland from any ocean.
    ///
    /// Relocated from the excised legacy `HeightField` path, which sampled the same
    /// invariants through a now-deleted generator. The desert thresholds (`>= 220`
    /// connected cells, surface `>= y82`) and inland-swamp test are carried over
    /// verbatim; they pass on the shipping path with wide margin. The legacy
    /// per-biome upper caps (forest/savanna `<= 2200`, etc.) were artifacts of the
    /// old climate-noise generator and do NOT hold for the cascade biome system —
    /// which deliberately grows large contiguous provinces — so they are replaced
    /// by the equivalent "no single biome dominates" invariant. Window origin
    /// `(4096, -4096)` at seed 42 is a continental patch with a desert/savanna belt
    /// abutting forest, plains, swamp, mountains and ocean.
    #[test]
    fn biome_field_keeps_regions_varied_with_large_deserts_and_inland_swamps() {
        const SEED: u32 = 42;
        const STEP: i32 = 8;
        const R: i32 = 640;
        const OX: i32 = 4096;
        const OZ: i32 = -4096;
        let n = (R * 2 / STEP + 1) as usize;
        let world = CascadeWorld::new(SEED);
        // One contiguous cascade region covering every sampled column (origin and
        // size are multiples of 4 as `region` requires).
        let span = ((n - 1) as i32 * STEP) as usize; // 1280
        let region = world.region(OX, OZ, span + 4, span + 4);

        let mut grid = vec![Biome::Ocean; n * n];
        let mut max_desert_y = i32::MIN;
        let mut counts = [0usize; 32];
        for gz in 0..n {
            let wz = OZ + gz as i32 * STEP;
            for gx in 0..n {
                let wx = OX + gx as i32 * STEP;
                let (surf, biome_id) = region.at(wx, wz);
                let biome = map_biome(biome_id);
                if biome == Biome::Desert {
                    max_desert_y = max_desert_y.max(surf);
                }
                counts[biome.id() as usize] += 1;
                grid[gz * n + gx] = biome;
            }
        }

        // Varied world: many biomes present, none dominating. (Replaces the legacy
        // per-biome upper caps, which the cascade biome system does not satisfy.)
        let distinct = counts.iter().filter(|&&c| c > 0).count();
        let max_any = counts.iter().copied().max().unwrap_or(0);
        let total = n * n;
        assert!(
            distinct >= 8,
            "expected a varied biome mix, only {distinct} distinct biomes sampled"
        );
        assert!(
            max_any * 5 < total * 2,
            "expected no single biome to dominate, but one covered {max_any}/{total} cells"
        );

        // A large connected desert exists and reaches hill/plateau elevation.
        let largest_desert = largest_component(&grid, n, Biome::Desert);
        assert!(
            largest_desert >= 220,
            "largest sampled desert component was {largest_desert} cells"
        );
        assert!(
            max_desert_y >= 82,
            "expected desert hills/plateaus, max desert surface was y{max_desert_y}"
        );

        // At least one inland swamp (>= 64 blocks from any sampled ocean).
        assert!(
            has_inland_swamp(&grid, n, 8),
            "expected at least one swamp sample >=64 blocks from sampled ocean"
        );
    }

    fn largest_component(grid: &[Biome], n: usize, target: Biome) -> usize {
        let mut seen = vec![false; grid.len()];
        let mut best = 0usize;
        let mut stack = Vec::new();
        for i in 0..grid.len() {
            if seen[i] || grid[i] != target {
                continue;
            }
            seen[i] = true;
            stack.push(i);
            let mut size = 0usize;
            while let Some(cur) = stack.pop() {
                size += 1;
                let x = cur % n;
                let z = cur / n;
                let push = |nx: usize, nz: usize, seen: &mut [bool], stack: &mut Vec<usize>| {
                    let ni = nz * n + nx;
                    if !seen[ni] && grid[ni] == target {
                        seen[ni] = true;
                        stack.push(ni);
                    }
                };
                if x > 0 {
                    push(x - 1, z, &mut seen, &mut stack);
                }
                if x + 1 < n {
                    push(x + 1, z, &mut seen, &mut stack);
                }
                if z > 0 {
                    push(x, z - 1, &mut seen, &mut stack);
                }
                if z + 1 < n {
                    push(x, z + 1, &mut seen, &mut stack);
                }
            }
            best = best.max(size);
        }
        best
    }

    fn has_inland_swamp(grid: &[Biome], n: usize, radius_cells: usize) -> bool {
        for z in 0..n {
            for x in 0..n {
                if grid[z * n + x] != Biome::Swamp {
                    continue;
                }
                let z0 = z.saturating_sub(radius_cells);
                let z1 = (z + radius_cells).min(n - 1);
                let x0 = x.saturating_sub(radius_cells);
                let x1 = (x + radius_cells).min(n - 1);
                let mut near_ocean = false;
                'scan: for nz in z0..=z1 {
                    for nx in x0..=x1 {
                        if matches!(grid[nz * n + nx], Biome::Ocean | Biome::DeepOcean) {
                            near_ocean = true;
                            break 'scan;
                        }
                    }
                }
                if !near_ocean {
                    return true;
                }
            }
        }
        false
    }

    #[test]
    #[ignore = "diagnostic: prints the cascade biome histogram near origin"]
    fn biome_histogram() {
        for seed in [42u32, 7, 1] {
            let ids = voronoi(seed as i64).gen(-256, -256, 512, 512);
            let mut counts = std::collections::BTreeMap::<i32, usize>::new();
            for &id in &ids {
                *counts.entry(id).or_default() += 1;
            }
            let total = ids.len() as f64;
            let mut v: Vec<_> = counts.into_iter().collect();
            v.sort_by_key(|&(_, n)| std::cmp::Reverse(n));
            eprintln!("--- seed {seed} (512x512 around origin) ---");
            for (id, n) in v.iter().take(14) {
                eprintln!(
                    "  id {:3} ({:?}): {:.1}%",
                    id,
                    map_biome(*id),
                    *n as f64 / total * 100.0
                );
            }
        }
    }
}
