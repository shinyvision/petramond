//! Biome-blended vertex tints for the chunk mesher.
//!
//! Grass tops, foliage (leaves), and water are tinted by the biome colour, blended
//! over a 5x5 column window so the colour transitions smoothly across biome borders
//! (the same window Minecraft uses). The blend is precomputed once per chunk in
//! [`biome_window`]; the per-face loop then just looks the column up.

use crate::atlas::Tile;
use crate::biome::Biome;
use crate::chunk::{CHUNK_SX, CHUNK_SZ};

/// Which biome-colour a tile tints with, or `None` for an untinted tile.
#[derive(Copy, Clone)]
pub(super) enum TintKind {
    Grass,
    Foliage,
    Water,
}

/// Classify a tile's biome-tint kind. Grass tops / short grass / fern take the
/// grass colour; all leaves take the foliage colour; water takes the water colour;
/// everything else is untinted.
pub(super) fn tile_tint(tile: Tile) -> Option<TintKind> {
    match tile {
        Tile::GrassTop => Some(TintKind::Grass),
        Tile::ShortGrass => Some(TintKind::Grass),
        Tile::Fern => Some(TintKind::Grass),
        Tile::Water => Some(TintKind::Water),
        Tile::WaterStill => Some(TintKind::Water),
        Tile::WaterFlow => Some(TintKind::Water),
        Tile::OakLeaves => Some(TintKind::Foliage),
        Tile::AcaciaLeaves => Some(TintKind::Foliage),
        Tile::BirchLeaves => Some(TintKind::Foliage),
        Tile::DarkOakLeaves => Some(TintKind::Foliage),
        Tile::JungleLeaves => Some(TintKind::Foliage),
        Tile::MangroveLeaves => Some(TintKind::Foliage),
        Tile::SpruceLeaves => Some(TintKind::Foliage),
        Tile::RedwoodLeaves => Some(TintKind::Foliage),
        _ => None,
    }
}

/// The untinted (white) tint used for tiles with no biome colour.
pub(super) const NO_TINT: [f32; 3] = [1.0, 1.0, 1.0];

/// The grass / foliage / water biome tints for every column of one chunk, each
/// 5x5-window blended. Indexed by the column index `z * CHUNK_SX + x`.
pub(super) struct BiomeTints {
    pub grass: Vec<[f32; 3]>,
    pub foliage: Vec<[f32; 3]>,
    pub water: Vec<[f32; 3]>,
}

impl BiomeTints {
    /// The blended tint for a tile at column `ci`, by its [`TintKind`] (untinted
    /// tiles get [`NO_TINT`]).
    #[inline]
    pub(super) fn tile(&self, kind: Option<TintKind>, ci: usize) -> [f32; 3] {
        match kind {
            Some(TintKind::Grass) => self.grass[ci],
            Some(TintKind::Foliage) => self.foliage[ci],
            Some(TintKind::Water) => self.water[ci],
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
    const R: i32 = 2;
    let n = (2 * R + 1) as f32 * (2 * R + 1) as f32;
    let mut grass = vec![[0f32; 3]; CHUNK_SX * CHUNK_SZ];
    let mut foliage = vec![[0f32; 3]; CHUNK_SX * CHUNK_SZ];
    let mut water = vec![[0f32; 3]; CHUNK_SX * CHUNK_SZ];
    for z in 0..CHUNK_SZ {
        for x in 0..CHUNK_SX {
            let wx = ox + x as i32;
            let wz = oz + z as i32;
            let mut g = [0f32; 3];
            let mut f = [0f32; 3];
            let mut w = [0f32; 3];
            for dz in -R..=R {
                for dx in -R..=R {
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
