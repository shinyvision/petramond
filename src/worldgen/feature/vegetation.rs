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
use crate::chunk::{Chunk, CHUNK_SX, CHUNK_SY, CHUNK_SZ, SEA_LEVEL, SECTION_SIZE, WORLD_MAX_Y};
use crate::mathh::smoothstep;
use crate::section::Section;
use crate::worldgen::biome::{spec, CoverCluster};
use crate::worldgen::surface::rule::SurfaceCtx;
use crate::worldgen::surface::SurfaceSystem;

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
            } else if spec(biome).snow_cover.covers(top) && surf.is_solid() && !surf.is_slippery() {
                // Snow-covered columns blanket the bare ground with a snow
                // layer; a column that rolled a plant keeps it (ferns poke
                // through the snow). The solid-surface guard skips water tops
                // (a submerged column's heightmap ends at the waterline), and
                // the slippery guard skips SEA ICE tops — snow never rides the
                // ice. That last exclusion is also what keeps this path
                // byte-identical to `place_vegetation_section`, which skips
                // every submerged column outright (`column_surf < SEA_LEVEL`):
                // a frozen pond inside a snowy biome is exactly a submerged
                // column whose heightmap ends at solid waterline ice.
                chunk.set_block_raw(x, above, z, Block::SnowLayer.id());
            }
        }
    }
}

/// Cubic per-section ground vegetation. Places each column's single plant into the
/// ONE section that contains the cell just above its post-cave bare-ground top, so
/// the result is byte-identical to the whole-column [`place_vegetation`] for this
/// section's slab. `biomes`/`surf`/`top` are the column's 16×16 grids (biome id,
/// original density surface, and post-cave top), indexed `z*16 + x`.
///
/// Submerged columns are skipped outright because their top material is water. The
/// surface material is recomputed analytically at the post-cave anchor depth
/// because the anchor cell may live in the section below this one. Must run AFTER
/// terrain + scatter and BEFORE features, matching the chunk stage order.
pub fn place_vegetation_section(
    section: &mut Section,
    biomes: &[u8],
    surf: &[i32],
    top: &[i32],
    seed: u32,
) {
    let (ox, oy, oz) = section.origin_world();
    for z in 0..SECTION_SIZE {
        for x in 0..SECTION_SIZE {
            let i = z * SECTION_SIZE + x;
            let column_surf = surf[i];
            // Submerged (or floorless) columns top out at the waterline: their surface
            // material is water, which carries no ground plant. Skip — matches the chunk
            // path, where `pick_plant(.., Water)` returns None.
            if column_surf < SEA_LEVEL {
                continue;
            }
            let anchor = top[i];
            if anchor < 1 || anchor + 1 >= WORLD_MAX_Y {
                continue;
            }
            let plant_y = anchor + 1;
            let ly = plant_y - oy;
            if ly < 0 || ly >= SECTION_SIZE as i32 {
                continue; // the plant cell belongs to a different section.
            }
            let (lx, ly, lz) = (x, ly as usize, z);
            if section.block_raw(lx, ly, lz) != Block::Air.id() {
                continue; // already occupied (terrain/scatter) — matches the chunk guard.
            }
            let biome = Biome::from_id(biomes[i]);
            let wx = ox + x as i32;
            let wz = oz + z as i32;
            let depth_from_top = (column_surf - anchor).max(0) as u32;
            let surf_block = SurfaceSystem.skin_block(
                &SurfaceCtx {
                    seed,
                    wx,
                    wz,
                    y: anchor,
                    surf_y: column_surf,
                    depth_from_top,
                },
                spec(biome).surface,
            );
            let mut rng = FeatureRng::positional(seed, VEG_SALT, wx, 0, wz);
            if let Some(p) = pick_plant(biome, surf_block, seed, wx, wz, &mut rng) {
                section.set_block_raw(lx, ly, lz, p.id());
            } else if spec(biome).snow_cover.covers(anchor) && surf_block.is_solid() {
                // Mirrors the chunk path's snow-layer branch — the analytic
                // skin block stands in for the chunk read, exactly like the
                // plant pick above, so the two paths stay byte-identical.
                section.set_block_raw(lx, ly, lz, Block::SnowLayer.id());
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
    let vegetation = spec(biome).vegetation;

    if matches!(surf, Block::Sand | Block::RedSand) {
        return vegetation.sand_cover.and_then(|picker| picker(rng));
    }

    if surf == Block::Mycelium {
        if !rng.chance(0.10) {
            return None;
        }
        return Some(if rng.next_i32(0, 99) < 55 {
            Block::RedMushroom
        } else {
            Block::BrownMushroom
        });
    }

    if surf == Block::Podzol {
        if !cover_cluster_allows(vegetation.cover_cluster, seed, wx, wz) {
            return None;
        }
        return vegetation.podzol_cover.and_then(|picker| picker(rng));
    }

    if surf != Block::Grass {
        return None;
    }

    let palette = vegetation.flower_palette;
    if !palette.is_empty() {
        let presence = patch_field(seed, PATCH_PRESENCE_SALT, wx, wz, PATCH_PERIOD);
        if presence > 1.0 - vegetation.flower_coverage && rng.chance(vegetation.flower_density) {
            let kind = patch_field(seed, PATCH_TYPE_SALT, wx, wz, PATCH_PERIOD);
            let idx = ((kind * palette.len() as f32) as usize).min(palette.len() - 1);
            return Some(palette[idx]);
        }
    }

    if let Some(picker) = vegetation.grass_cover {
        if !cover_cluster_allows(vegetation.cover_cluster, seed, wx, wz) {
            return None;
        }
        return picker(rng);
    }
    if rng.chance(vegetation.grass_density) {
        return Some(vegetation.grass_tuft);
    }
    None
}

/// Whether ground cover is allowed at this column under the biome's optional
/// cluster mask: with no mask, always; otherwise only inside a low-frequency
/// patch (so ferns/tufts form clumps with bare ground between).
fn cover_cluster_allows(cluster: Option<CoverCluster>, seed: u32, wx: i32, wz: i32) -> bool {
    match cluster {
        None => true,
        Some(c) => patch_field(seed, c.salt, wx, wz, c.period) < c.coverage,
    }
}

/// Smooth low-frequency value field in `[0,1)` at world `(wx,wz)`: hashed lattice
/// corners with a smoothstep bilinear blend, so flower patches are organic blobs
/// rather than a hard grid. Pure function of `(seed, salt, wx, wz)` — seamless
/// across chunk borders.
pub(crate) fn patch_field(seed: u32, salt: u64, wx: i32, wz: i32, period: f32) -> f32 {
    let fx = wx as f32 / period;
    let fz = wz as f32 / period;
    let x0 = fx.floor() as i32;
    let z0 = fz.floor() as i32;
    let tx = smoothstep(0.0, 1.0, fx - x0 as f32);
    let tz = smoothstep(0.0, 1.0, fz - z0 as f32);
    let corner = |ix: i32, iz: i32| FeatureRng::positional(seed, salt, ix, 0, iz).next_f32();
    let c00 = corner(x0, z0);
    let c10 = corner(x0 + 1, z0);
    let c01 = corner(x0, z0 + 1);
    let c11 = corner(x0 + 1, z0 + 1);
    let a = c00 + (c10 - c00) * tx;
    let b = c01 + (c11 - c01) * tx;
    a + (b - a) * tz
}
