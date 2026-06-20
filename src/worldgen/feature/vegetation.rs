//! Ground vegetation — the `DecoStep::VegetationGround` content.
//!
//! Scatters single-block plants (grass tufts, ferns, flowers, mushrooms, dead
//! bushes, the odd cactus) on top of the terrain, keyed to the column's biome and
//! its surface material. Runs AFTER the underground pass but BEFORE trees, so the
//! surface read from the heightmap is bare ground (not a tree canopy).
//!
//! Plants are one block wide, so there is no cross-chunk footprint: each column's
//! plant is placed by its owning chunk from a positional RNG keyed on (seed, wx,
//! wz), making the result deterministic and seamless with no neighbour pass.

use crate::biome::Biome;
use crate::block::Block;
use crate::chunk::{Chunk, CHUNK_SX, CHUNK_SY, CHUNK_SZ};
use crate::mathh::smoothstep;

use super::super::rng::FeatureRng;

const VEG_SALT: u64 = 0x0000_5EED_1EAF_0001;
/// Salt for the flower-patch SPECIES field (which one flower a patch is made of).
const PATCH_TYPE_SALT: u64 = 0x0000_F10E_7376_0001;
/// Salt for the flower-patch PRESENCE field (where flower patches occur at all).
const PATCH_PRESENCE_SALT: u64 = 0x0000_B10C_7376_0001;
/// Flower-patch lattice period in blocks: one species field cell per this many
/// blocks, so a run of a single flower species reads as a small cluster.
const PATCH_PERIOD: f32 = 13.0;

/// Place ground vegetation across the chunk. Pure function of `(seed, cx, cz)`.
pub fn place_vegetation(chunk: &mut Chunk, seed: u32) {
    let (ox, oz) = chunk.chunk_origin_world();
    for z in 0..CHUNK_SZ {
        for x in 0..CHUNK_SX {
            let top = chunk.surface_y(x, z);
            // Need a ground voxel and an empty cell above it within the world.
            if top < 1 || top + 1 >= CHUNK_SY as i32 {
                continue;
            }
            let above = (top + 1) as usize;
            if chunk.block_raw(x, above, z) != Block::Air.id() {
                continue; // already occupied (e.g. tree, or water surface)
            }
            let surf = Block::from_id(chunk.block_raw(x, top as usize, z));
            let biome = Biome::from_id(chunk.biome_at(x, z));
            let wx = ox + x as i32;
            let wz = oz + z as i32;
            let mut rng = FeatureRng::positional(seed, VEG_SALT, wx, 0, wz);
            if let Some(p) = pick_plant(biome, surf, seed, wx, wz, &mut rng) {
                chunk.set_block_raw(x, above, z, p.id());
            }
        }
    }
}

/// Choose a plant for a column. Non-grass surfaces keep their material-specific
/// scatter; grass surfaces split into two INDEPENDENT decisions:
///   1. a common grass-tuft / fern scatter (keyed to the column RNG), and
///   2. a flower PATCH: a low-frequency presence field decides whether this column
///      is inside a flower patch, a low-frequency species field picks the ONE
///      flower that patch is made of, and the column RNG decides whether a flower
///      actually stands here. So flowers appear as single-species clusters with a
///      natural within-patch scatter — not a per-column free-for-all of mixed
///      species. Both draws happen in a fixed order so the stream is deterministic.
fn pick_plant(
    biome: Biome,
    surf: Block,
    seed: u32,
    wx: i32,
    wz: i32,
    rng: &mut FeatureRng,
) -> Option<Block> {
    use Biome::*;
    use Block::*;

    // Dry sand surfaces: sparse desert / badlands dead bushes + the occasional
    // cactus. Dead bushes are deliberately rare so deserts read as open sand.
    if matches!(surf, Sand | RedSand) {
        if !matches!(biome, Desert | Badlands | Beach) || !rng.chance(0.007) {
            return None;
        }
        return Some(if rng.next_i32(0, 99) < 45 { DeadBush } else { Cactus });
    }
    // Mushroom-island mycelium: dense mushrooms.
    if surf == Mycelium {
        if !rng.chance(0.10) {
            return None;
        }
        return Some(if rng.next_i32(0, 99) < 55 { RedMushroom } else { BrownMushroom });
    }
    // Podzol (old-growth taiga / grove): ferns + mushrooms.
    if surf == Podzol {
        if !rng.chance(0.10) {
            return None;
        }
        let r = rng.next_i32(0, 99);
        return Some(if r < 58 {
            Fern
        } else if r < 80 {
            ShortGrass
        } else if r < 90 {
            RedMushroom
        } else {
            BrownMushroom
        });
    }
    // Everything else only vegetates on grass.
    if surf != Grass {
        return None;
    }

    // (1) Flower patch — checked first, then grass, so the draw order is fixed.
    let palette = flower_palette(biome);
    if !palette.is_empty() {
        let (coverage, in_patch_density) = flower_params(biome);
        // Inside a flower patch when the presence field is in the top `coverage`.
        let presence = patch_field(seed, PATCH_PRESENCE_SALT, wx, wz);
        if presence > 1.0 - coverage && rng.chance(in_patch_density) {
            let kind = patch_field(seed, PATCH_TYPE_SALT, wx, wz);
            let idx = ((kind * palette.len() as f32) as usize).min(palette.len() - 1);
            return Some(palette[idx]);
        }
    }

    // (2) Grass-tuft / fern scatter — the common ground cover everywhere.
    let (tuft, density) = grass_cover(biome);
    if rng.chance(density) {
        return Some(tuft);
    }
    None
}

/// Smooth low-frequency value field in `[0,1)` at world `(wx,wz)`: hashed lattice
/// corners with a smoothstep bilinear blend, so flower patches are organic blobs
/// rather than a hard grid. Pure function of `(seed, salt, wx, wz)` — seamless
/// across chunk borders.
fn patch_field(seed: u32, salt: u64, wx: i32, wz: i32) -> f32 {
    let fx = wx as f32 / PATCH_PERIOD;
    let fz = wz as f32 / PATCH_PERIOD;
    let x0 = fx.floor() as i32;
    let z0 = fz.floor() as i32;
    let tx = smoothstep(0.0, 1.0, fx - x0 as f32);
    let tz = smoothstep(0.0, 1.0, fz - z0 as f32);
    let corner =
        |ix: i32, iz: i32| FeatureRng::positional(seed, salt, ix, 0, iz).next_f32();
    let c00 = corner(x0, z0);
    let c10 = corner(x0 + 1, z0);
    let c01 = corner(x0, z0 + 1);
    let c11 = corner(x0 + 1, z0 + 1);
    let a = c00 + (c10 - c00) * tx;
    let b = c01 + (c11 - c01) * tx;
    a + (b - a) * tz
}

/// A biome's flower species palette. A patch is made of exactly ONE entry, chosen
/// by the species field. Empty = no flowers (grass/fern only).
fn flower_palette(biome: Biome) -> &'static [Block] {
    use Biome::*;
    use Block::*;
    match biome {
        Meadow => &[Dandelion, Poppy, OxeyeDaisy, Cornflower, Allium, AzureBluet],
        Plains => &[Dandelion, Poppy, OxeyeDaisy, Cornflower, AzureBluet],
        CherryGrove => &[Allium, OxeyeDaisy, Poppy],
        Forest => &[Poppy, Dandelion, OxeyeDaisy],
        BirchForest => &[OxeyeDaisy, Cornflower, Dandelion],
        _ => &[],
    }
}

/// `(patch coverage, within-patch density)` per biome. Coverage is the fraction of
/// the biome inside flower patches; density is how thickly flowers stand within a
/// patch. Tuned so flower biomes are lush and everything else only has occasional
/// patches.
fn flower_params(biome: Biome) -> (f32, f32) {
    use Biome::*;
    match biome {
        Meadow => (0.60, 0.45),
        CherryGrove => (0.45, 0.40),
        Plains => (0.28, 0.30),
        Forest => (0.16, 0.22),
        BirchForest => (0.16, 0.22),
        _ => (0.0, 0.0),
    }
}

/// The common ground-cover tuft for a biome and how often it stands. Conifer/cold
/// biomes get ferns; everywhere else short grass.
fn grass_cover(biome: Biome) -> (Block, f32) {
    use Biome::*;
    use Block::*;
    match biome {
        Taiga | SnowyTaiga | Grove | OldGrowthTaiga => (Fern, 0.12),
        Jungle => (Fern, 0.20),
        Meadow => (ShortGrass, 0.16),
        Plains | Savanna => (ShortGrass, 0.14),
        Forest | BirchForest | DarkForest | CherryGrove => (ShortGrass, 0.11),
        Swamp | Wetland => (ShortGrass, 0.10),
        WindsweptHills | Foothills => (ShortGrass, 0.06),
        _ => (ShortGrass, 0.05),
    }
}
