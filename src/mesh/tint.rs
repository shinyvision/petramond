//! Biome-blended vertex tints for the chunk mesher.
//!
//! Grass tops, foliage (leaves), and water are tinted by the biome colour, blended
//! over a 5x5 column window so the colour transitions smoothly across biome borders
//! The blend is precomputed once per section in
//! [`biome_window`]; the per-face loop then just looks the column up.

use crate::atlas::TileTint;
use crate::biome::Biome;
use crate::chunk::{CHUNK_SX, CHUNK_SZ};

const COLUMNS: usize = CHUNK_SX * CHUNK_SZ;
const BLEND_RADIUS: i32 = 2;
const BLEND_DIAMETER: usize = (BLEND_RADIUS as usize * 2) + 1;
const BIOME_PAD_X: usize = CHUNK_SX + (BLEND_RADIUS as usize * 2);
const BIOME_PAD_Z: usize = CHUNK_SZ + (BLEND_RADIUS as usize * 2);
const BIOME_PAD_AREA: usize = BIOME_PAD_X * BIOME_PAD_Z;
const SUM_X: usize = BIOME_PAD_X + 1;
const SUM_Z: usize = BIOME_PAD_Z + 1;
const SUM_CHANNELS: usize = 9;
const INV_BLEND_AREA: f32 = 1.0 / (BLEND_DIAMETER * BLEND_DIAMETER) as f32;

/// The untinted (white) tint used for tiles with no biome colour.
pub(super) const NO_TINT: [f32; 3] = [1.0, 1.0, 1.0];

/// The grass / foliage / water biome tints for every column of one chunk, each
/// 5x5-window blended. Indexed by the column index `z * CHUNK_SX + x`.
pub(super) struct BiomeTints {
    pub grass: [[f32; 3]; COLUMNS],
    pub foliage: [[f32; 3]; COLUMNS],
    pub water: [[f32; 3]; COLUMNS],
}

impl BiomeTints {
    #[inline]
    fn filled(biome: Biome) -> Self {
        Self {
            grass: [biome.grass_color(); COLUMNS],
            foliage: [biome.foliage_color(); COLUMNS],
            water: [biome.water_color(); COLUMNS],
        }
    }

    /// The blended tint for a tile at column `ci`, by its
    /// [`world_tint`](crate::atlas::Tile::world_tint) class (untinted tiles get
    /// [`NO_TINT`]). The classification is atlas-manifest data, so a modded
    /// texture joins a tint class by declaring it — never a code edit here.
    #[inline]
    pub(super) fn tile(&self, kind: Option<TileTint>, ci: usize) -> [f32; 3] {
        match kind {
            Some(TileTint::Grass) => self.grass[ci],
            Some(TileTint::Foliage) => self.foliage[ci],
            Some(TileTint::Water) => self.water[ci],
            None => NO_TINT,
        }
    }
}

/// Precompute the biome-blended grass / foliage / water tint of every column in
/// the chunk at origin `(ox, oz)`, averaging each biome colour over a 5x5 window
/// of columns around it (`neighbour_biome(wx, wz)` reads the biome id at a world
/// column, crossing chunk borders).
pub(super) fn biome_window(
    ox: i32,
    oz: i32,
    neighbour_biome: impl Fn(i32, i32) -> u8,
) -> BiomeTints {
    let mut ids = [0u8; BIOME_PAD_AREA];
    let mut first = 0u8;
    let mut uniform = true;
    for z in 0..BIOME_PAD_Z {
        for x in 0..BIOME_PAD_X {
            let id = neighbour_biome(ox + x as i32 - BLEND_RADIUS, oz + z as i32 - BLEND_RADIUS);
            if x == 0 && z == 0 {
                first = id;
            } else if id != first {
                uniform = false;
            }
            ids[z * BIOME_PAD_X + x] = id;
        }
    }
    if uniform {
        return BiomeTints::filled(Biome::from_id(first));
    }

    let mut sums = [[[0f32; SUM_CHANNELS]; SUM_X]; SUM_Z];
    for z in 0..BIOME_PAD_Z {
        for x in 0..BIOME_PAD_X {
            let b = Biome::from_id(ids[z * BIOME_PAD_X + x]);
            let bg = b.grass_color();
            let bf = b.foliage_color();
            let bw = b.water_color();
            let v = [
                bg[0], bg[1], bg[2], bf[0], bf[1], bf[2], bw[0], bw[1], bw[2],
            ];
            for c in 0..SUM_CHANNELS {
                sums[z + 1][x + 1][c] =
                    v[c] + sums[z][x + 1][c] + sums[z + 1][x][c] - sums[z][x][c];
            }
        }
    }

    let mut grass = [[0f32; 3]; COLUMNS];
    let mut foliage = [[0f32; 3]; COLUMNS];
    let mut water = [[0f32; 3]; COLUMNS];
    for z in 0..CHUNK_SZ {
        for x in 0..CHUNK_SX {
            let i = z * CHUNK_SX + x;
            let x0 = x;
            let z0 = z;
            let x1 = x + BLEND_DIAMETER;
            let z1 = z + BLEND_DIAMETER;
            let sum = |c: usize| -> f32 {
                sums[z1][x1][c] + sums[z0][x0][c] - sums[z0][x1][c] - sums[z1][x0][c]
            };
            grass[i] = [
                sum(0) * INV_BLEND_AREA,
                sum(1) * INV_BLEND_AREA,
                sum(2) * INV_BLEND_AREA,
            ];
            foliage[i] = [
                sum(3) * INV_BLEND_AREA,
                sum(4) * INV_BLEND_AREA,
                sum(5) * INV_BLEND_AREA,
            ];
            water[i] = [
                sum(6) * INV_BLEND_AREA,
                sum(7) * INV_BLEND_AREA,
                sum(8) * INV_BLEND_AREA,
            ];
        }
    }
    BiomeTints {
        grass,
        foliage,
        water,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reference_window(ox: i32, oz: i32, neighbour_biome: impl Fn(i32, i32) -> u8) -> BiomeTints {
        let n = (BLEND_DIAMETER * BLEND_DIAMETER) as f32;
        let mut grass = [[0f32; 3]; COLUMNS];
        let mut foliage = [[0f32; 3]; COLUMNS];
        let mut water = [[0f32; 3]; COLUMNS];
        for z in 0..CHUNK_SZ {
            for x in 0..CHUNK_SX {
                let wx = ox + x as i32;
                let wz = oz + z as i32;
                let mut g = [0f32; 3];
                let mut f = [0f32; 3];
                let mut w = [0f32; 3];
                for dz in -BLEND_RADIUS..=BLEND_RADIUS {
                    for dx in -BLEND_RADIUS..=BLEND_RADIUS {
                        let b = Biome::from_id(neighbour_biome(wx + dx, wz + dz));
                        let bg = b.grass_color();
                        let bf = b.foliage_color();
                        let bw = b.water_color();
                        g[0] += bg[0];
                        g[1] += bg[1];
                        g[2] += bg[2];
                        f[0] += bf[0];
                        f[1] += bf[1];
                        f[2] += bf[2];
                        w[0] += bw[0];
                        w[1] += bw[1];
                        w[2] += bw[2];
                    }
                }
                let i = z * CHUNK_SX + x;
                grass[i] = [g[0] / n, g[1] / n, g[2] / n];
                foliage[i] = [f[0] / n, f[1] / n, f[2] / n];
                water[i] = [w[0] / n, w[1] / n, w[2] / n];
            }
        }
        BiomeTints {
            grass,
            foliage,
            water,
        }
    }

    fn assert_close(a: [f32; 3], b: [f32; 3]) {
        // Tolerance is f32 summed-area drift, not exactness: the SAT subtracts
        // large running sums, so the last bits depend on the palette values
        // themselves. Real algorithmic breakage differs at percent level.
        for i in 0..3 {
            assert!((a[i] - b[i]).abs() < 0.0001, "{a:?} != {b:?}");
        }
    }

    #[test]
    fn summed_area_tint_matches_reference_window() {
        let biome = |wx: i32, wz: i32| -> u8 {
            match (wx.div_euclid(3) + wz.div_euclid(5)).rem_euclid(5) {
                0 => Biome::Plains.id(),
                1 => Biome::Forest.id(),
                2 => Biome::Swamp.id(),
                3 => Biome::Desert.id(),
                _ => Biome::Taiga.id(),
            }
        };

        let fast = biome_window(-16, 32, biome);
        let reference = reference_window(-16, 32, biome);
        for i in 0..COLUMNS {
            assert_close(fast.grass[i], reference.grass[i]);
            assert_close(fast.foliage[i], reference.foliage[i]);
            assert_close(fast.water[i], reference.water[i]);
        }
    }
}
