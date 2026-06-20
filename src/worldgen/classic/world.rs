//! Biome-driven world provider: the bridge from the verified layered biome
//! cascade to the game's chunk generator.
//!
//! For a chunk it yields (1) the per-block biome (the cascade's voronoi output,
//! matching the reference biome layout) and (2) a surface height shaped FROM the
//! biomes (each biome's base height + variation blended through the noise terrain),
//! so biomes and terrain correlate and the low river biome becomes a continuous
//! flooded valley. The game's `Biome` enum is the render/feature vocabulary, so MC
//! biome ids are mapped onto it via [`map_biome`].

use crate::biome::Biome;
use crate::chunk::SEA_LEVEL;

use super::biome::layers::Layer;
use super::biome::stack::{river_mix, voronoi};
use super::terrain::TerrainGen;

const CHUNK: usize = 16;
/// Border (blocks) of biome context kept around a chunk for the river-narrowing
/// proximity test.
const BORDER: i32 = 2;

/// One chunk's biome-driven terrain: top-solid surface height and per-block MC
/// biome id, both row-major `x + z*16`.
pub struct ChunkCells {
    pub surf: [i32; 256],
    pub biome_ids: [i32; 256],
}

/// Owns the per-seed cascade layers + terrain noise (built once, reused per chunk).
pub struct CascadeWorld {
    terrain: TerrainGen,
    voronoi: Box<dyn Layer>,
    river_mix: Box<dyn Layer>,
}

impl CascadeWorld {
    pub fn new(seed: u32) -> Self {
        let s = seed as i64;
        Self {
            terrain: TerrainGen::new(s),
            voronoi: voronoi(s),
            river_mix: river_mix(s),
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

    /// Generate a block region's per-block biomes and biome-driven surface height
    /// (row-major `i + j*w`) in a SINGLE cascade pass — one voronoi gen, one
    /// river_mix gen, one terrain region fill — then river narrowing. `(x0,z0,w,h)`
    /// must be multiples of 4. Replaces the old per-chunk loop, so the feature
    /// margin (which spans several chunks) costs one pass instead of one per chunk.
    pub fn region(&self, x0: i32, z0: i32, w: usize, h: usize) -> RegionCells {
        // Voronoi biomes with a BORDER ring for the river-narrowing proximity test.
        let bw = w + 2 * BORDER as usize;
        let bh = h + 2 * BORDER as usize;
        let bordered = self
            .voronoi
            .gen((x0 - BORDER) as i64, (z0 - BORDER) as i64, bw, bh);
        // Scale-4 biome grid for the terrain blend: origin (x0/4 - 2), stride cw+5.
        let (cw, ch) = (w / 4, h / 4);
        let bstride = cw + 5;
        let bheight = ch + 5;
        let rm = self
            .river_mix
            .gen((x0 >> 2) as i64 - 2, (z0 >> 2) as i64 - 2, bstride, bheight);
        let mut surf = self.terrain.region_heightmap(x0, z0, w, h, &rm, bstride);
        let mut biome_ids = vec![0i32; w * h];
        for z in 0..h {
            for x in 0..w {
                let i = z * w + x;
                let id = bordered[(z + BORDER as usize) * bw + (x + BORDER as usize)];
                biome_ids[i] = id;
                // River narrowing: lift a LAND column dragged below sea only by a
                // neighbouring river biome's height blend back to a bank, so rivers
                // stay narrow distinct channels rather than bleeding into wide water
                // where strands run close. Low biomes (oceans, river, swamp, shores)
                // are exempt.
                if surf[i] <= SEA_LEVEL && !keeps_low(id) && near_river(&bordered, bw, x, z) {
                    surf[i] = SEA_LEVEL + 1;
                }
            }
        }
        RegionCells {
            x0,
            z0,
            w,
            surf,
            biome_ids,
        }
    }
}

/// A block region's biome-driven terrain. `surf`/`biome_ids` are row-major
/// `(wx-x0) + (wz-z0)*w`; use [`RegionCells::at`] for world-coordinate lookups.
pub struct RegionCells {
    pub x0: i32,
    pub z0: i32,
    pub w: usize,
    pub surf: Vec<i32>,
    pub biome_ids: Vec<i32>,
}

impl RegionCells {
    #[inline]
    pub fn at(&self, wx: i32, wz: i32) -> (i32, i32) {
        let i = (wz - self.z0) as usize * self.w + (wx - self.x0) as usize;
        (self.surf[i], self.biome_ids[i])
    }
}

/// True if a river biome cell lies within [`BORDER`] of `(x,z)` in the bordered
/// voronoi grid (indices are into the bordered grid, `(x,z)` are inner coords).
fn near_river(bordered: &[i32], bw: usize, x: usize, z: usize) -> bool {
    for dz in 0..=(2 * BORDER as usize) {
        for dx in 0..=(2 * BORDER as usize) {
            let id = bordered[(z + dz) * bw + (x + dx)];
            let base = if id >= 128 { id - 128 } else { id };
            if base == 7 || base == 11 {
                return true;
            }
        }
    }
    false
}

/// Biomes that legitimately sit at/below sea level and must NOT be lifted by river
/// narrowing (oceans, the river itself, swamps, and shores).
fn keeps_low(id: i32) -> bool {
    let base = if id >= 128 { id - 128 } else { id };
    matches!(base, 0 | 10 | 24 | 7 | 11 | 6 | 16 | 26 | 15 | 25)
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
        3 => Mountains,        // extreme hills
        4 => Forest,
        5 => Taiga,
        6 => Swamp,
        7 => River,
        10 => Ocean,           // frozen ocean (no frozen-ocean game biome)
        11 => River,           // frozen river
        12 => SnowyTundra,
        13 => SnowyPeaks,      // snowy mountains
        14 => MushroomFields,
        15 => MushroomFields,  // mushroom shore
        16 => Beach,
        17 => Desert,          // desert hills
        18 => Forest,          // wooded hills
        19 => Taiga,           // taiga hills
        20 => Foothills,       // mountain edge
        21 => Jungle,
        22 => Jungle,          // jungle hills
        23 => Jungle,          // jungle edge
        24 => DeepOcean,
        25 => StonyPeaks,      // stone shore (rocky coast)
        26 => Beach,           // snowy beach
        27 => BirchForest,
        28 => BirchForest,     // birch forest hills
        29 => DarkForest,
        30 => SnowyTaiga,
        31 => SnowyTaiga,      // snowy taiga hills
        32 => OldGrowthTaiga,  // giant tree taiga
        33 => OldGrowthTaiga,  // giant tree taiga hills
        34 => Mountains,       // wooded mountains
        35 => Savanna,
        36 => Savanna,         // savanna plateau
        37 => Badlands,
        38 => Badlands,        // wooded badlands plateau
        39 => Badlands,        // badlands plateau
        _ => Plains,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
            v.sort_by(|a, b| b.1.cmp(&a.1));
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
