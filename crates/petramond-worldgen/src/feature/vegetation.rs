//! Ground vegetation — the `DecoStep::VegetationGround` content.
//!
//! Scatters single-block plants (grass tufts, ferns, flowers, mushrooms, dead
//! bushes, the odd cactus) on top of the terrain, keyed to the column's biome and
//! its surface material. Runs AFTER the underground pass but BEFORE trees, so the
//! surface read from the heightmap is bare ground (not a tree canopy).
//!
//! Plants are one block wide, so there is no cross-column footprint: each column's
//! plant is placed by the section holding it from a positional RNG keyed on
//! (seed, wx, wz), making the result deterministic and seamless with no
//! neighbour pass.

use crate::biome::{spec, CoverCluster};
use crate::surface::rule::SurfaceCtx;
use crate::surface::SurfaceSystem;
use petramond_world::biome::Biome;
use petramond_world::block::Block;
use petramond_world::chunk::{SEA_LEVEL, SECTION_SIZE, WORLD_MAX_Y, WORLD_MIN_Y};
use petramond_world::section::Section;

use super::super::rng::{patch_field, FeatureRng};
use crate::salts;

/// Flower-patch lattice period in blocks: one species field cell per this many
/// blocks, so a run of a single flower species reads as a small cluster.
const PATCH_PERIOD: f32 = 13.0;

/// Per-column chance of a pebble field on bare ground. A WORLD CONSTANT rather
/// than a biome field: loose stone lies on every kind of ground, and the first
/// pickaxe is knapped from it, so a fresh player must find some wherever they
/// spawn. Deliberately sparse — you should have to look.
const PEBBLE_DENSITY: f32 = 0.008;
/// Hemp grows in small STANDS: an anchor column plus a short random walk out
/// of it (the [`salts::HEMP_ANCHOR`] stream), the farming pack's wild-crop
/// patch shape (`WildCropSpec`) lifted into core.
///
/// THIS IS NOT A `patch_field` BLOB, and the reason is the whole point. A
/// smoothed field couples a patch's size to its spacing: both scale with the
/// period. An anchor plus a bounded walk separates the
/// two knobs — this is the size, the biome's anchor chance is the frequency —
/// and the size can never run away, because the walk stops.
///
/// Stalks per stand, inclusive: the walk's step count. Some steps revisit a
/// cell and some land on ground that refuses, so the stand is at most this.
const HEMP_STAND_STEPS: (i32, i32) = (2, 6);
const HEMP_STAND_REACH: i32 = 5;
const HEMP_ANCHOR_GRID: i32 = 4;

/// Per-section ground vegetation. Places each column's single plant into the ONE
/// section that contains the cell just above its post-cave bare-ground top.
/// `biomes`/`surf`/`top` are the column's 16×16 grids (biome id, original density
/// surface, and post-cave top), indexed `z*16 + x`.
///
/// Submerged columns are skipped outright because their top material is water (so
/// a frozen pond's sea ice never carries a snow layer). The surface material is
/// recomputed analytically at the post-cave anchor depth because the anchor cell
/// may live in the section below this one. Must run AFTER terrain + scatter and
/// BEFORE features.
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
            if column_surf < SEA_LEVEL {
                continue;
            }
            let anchor = top[i];
            if anchor <= WORLD_MIN_Y || anchor + 1 >= WORLD_MAX_Y {
                continue;
            }
            let plant_y = anchor + 1;
            let ly = plant_y - oy;
            if ly < 0 || ly >= SECTION_SIZE as i32 {
                continue;
            }
            let (lx, ly, lz) = (x, ly as usize, z);
            if section.block_raw(lx, ly, lz) != Block::Air.id() {
                continue;
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
            let mut rng = FeatureRng::positional(seed, salts::VEGETATION, wx, 0, wz);
            if let Some(p) = pick_plant(biome, surf_block, seed, wx, wz, &mut rng) {
                section.set_block_raw(lx, ly, lz, p.id());
            } else if spec(biome).snow_cover.covers(anchor) && surf_block.is_solid() {
                section.set_block_raw(lx, ly, lz, Block::SnowLayer.id());
            }
        }
    }
}

fn pick_plant(
    biome: Biome,
    surf: Block,
    seed: u32,
    wx: i32,
    wz: i32,
    rng: &mut FeatureRng,
) -> Option<Block> {
    let vegetation = spec(biome).vegetation;

    if let Some(litter) = pick_litter(biome, surf, seed, wx, wz, rng) {
        return Some(litter);
    }

    if let Some(cover) = vegetation.covers.iter().find(|c| c.on.contains(&surf)) {
        if cover.clustered && !cover_cluster_allows(vegetation.cover_cluster, seed, wx, wz) {
            return None;
        }
        return cover.roll.pick(rng);
    }

    if matches!(surf, Block::Sand | Block::RedSand) {
        return vegetation.sand_cover.and_then(|roll| roll.pick(rng));
    }

    if surf == Block::Podzol {
        if !cover_cluster_allows(vegetation.cover_cluster, seed, wx, wz) {
            return None;
        }
        return vegetation.podzol_cover.and_then(|roll| roll.pick(rng));
    }

    if surf != Block::Grass {
        return None;
    }

    let palette = vegetation.flower_palette;
    if !palette.is_empty() {
        let presence = patch_field(seed, salts::FLOWER_PATCH_PRESENCE, wx, wz, PATCH_PERIOD);
        if presence > 1.0 - vegetation.flower_coverage && rng.chance(vegetation.flower_density) {
            let kind = patch_field(seed, salts::FLOWER_PATCH_TYPE, wx, wz, PATCH_PERIOD);
            let idx = ((kind * palette.len() as f32) as usize).min(palette.len() - 1);
            return Some(palette[idx]);
        }
    }

    if let Some(roll) = vegetation.grass_cover {
        if !cover_cluster_allows(vegetation.cover_cluster, seed, wx, wz) {
            return None;
        }
        return roll.pick(rng);
    }
    if rng.chance(vegetation.grass_density) {
        return Some(vegetation.grass_tuft);
    }
    None
}

/// The gathering layer this pass owns: pebbles and hemp — two of the three
/// things a bare hand may take, and so most of what a fresh player has before
/// the first stone axe. FALLEN BRANCHES ARE NOT HERE: a branch has to be near
/// the tree it fell from, and this pass runs BEFORE the trees are placed, so
/// they are scattered by the tree placer itself (`feature::tree_select`).
///
/// Decided BEFORE the flowers and tufts, and it wins the column outright: the
/// densities are low enough that the ground cover barely notices, and running
/// it second would make litter scarcest exactly where the grass is thickest.
/// Draws happen in a fixed order, so the stream is a pure function of the
/// column, like every other decision on this path.
///
/// Snow does not gate this pass: players can spawn in snowy biomes and must
/// still find the materials for their first tool. A litter cell replaces its
/// column's snow layer, but its low density preserves the snowfield's appearance.
fn pick_litter(
    biome: Biome,
    surf: Block,
    seed: u32,
    wx: i32,
    wz: i32,
    rng: &mut FeatureRng,
) -> Option<Block> {
    if !litter_ground(surf) {
        return None;
    }
    let spec = spec(biome);

    if rng.chance(PEBBLE_DENSITY) {
        return Some(match rng.next_i32(0, 2) {
            0 => Block::PebblesSmall,
            1 => Block::PebblesMedium,
            _ => Block::PebblesLarge,
        });
    }

    let anchor_chance = spec.vegetation.hemp_anchor_chance;
    if anchor_chance > 0.0 && surf == Block::Grass && in_hemp_stand(seed, anchor_chance, wx, wz) {
        return Some(Block::Hemp);
    }
    None
}

fn in_hemp_stand(seed: u32, anchor_chance: f32, wx: i32, wz: i32) -> bool {
    fn candidates(v: i32) -> impl Iterator<Item = i32> {
        let lo = v - HEMP_STAND_REACH;
        let first = lo + (HEMP_ANCHOR_GRID - lo.rem_euclid(HEMP_ANCHOR_GRID)) % HEMP_ANCHOR_GRID;
        (0..).map_while(move |i| {
            let a = first + i * HEMP_ANCHOR_GRID;
            (a <= v + HEMP_STAND_REACH).then_some(a)
        })
    }
    for az in candidates(wz) {
        for ax in candidates(wx) {
            let mut rng = FeatureRng::positional(seed, salts::HEMP_ANCHOR, ax, 0, az);
            if !rng.chance(anchor_chance) {
                continue;
            }
            if (ax, az) == (wx, wz) {
                return true;
            }
            let steps = rng.next_i32(HEMP_STAND_STEPS.0, HEMP_STAND_STEPS.1);
            let (mut cx, mut cz) = (ax, az);
            for _ in 1..steps {
                let (dx, dz) = match rng.next_u64() % 4 {
                    0 => (1, 0),
                    1 => (-1, 0),
                    2 => (0, 1),
                    _ => (0, -1),
                };
                let (nx, nz) = (cx + dx, cz + dz);
                if (nx - ax).abs() > HEMP_STAND_REACH || (nz - az).abs() > HEMP_STAND_REACH {
                    continue;
                }
                (cx, cz) = (nx, nz);
                if (cx, cz) == (wx, wz) {
                    return true;
                }
            }
        }
    }
    false
}

#[inline]
pub fn litter_ground(surf: Block) -> bool {
    matches!(
        surf,
        Block::Grass | Block::Dirt | Block::Sand | Block::Podzol
    )
}

fn cover_cluster_allows(cluster: Option<CoverCluster>, seed: u32, wx: i32, wz: i32) -> bool {
    match cluster {
        None => true,
        Some(c) => patch_field(seed, c.salt, wx, wz, c.period) < c.coverage,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_snowfield_still_grows_the_ingredients_of_the_first_tool() {
        const SEED: u32 = 0x5EA5_04E5;
        const SIDE: i32 = 600;

        let mut pebbles = 0;
        let mut hemp = 0;
        for wz in 0..SIDE {
            for wx in 0..SIDE {
                let mut rng = FeatureRng::positional(SEED, salts::VEGETATION, wx, 0, wz);
                match pick_litter(Biome::SNOWY_TAIGA, Block::Grass, SEED, wx, wz, &mut rng) {
                    Some(Block::Hemp) => hemp += 1,
                    Some(_) => pebbles += 1,
                    None => {}
                }
            }
        }

        let columns = SIDE * SIDE;
        assert!(
            pebbles > 400,
            "snowfield grew {pebbles} pebbles over {columns} columns"
        );
        assert!(
            hemp > 0,
            "snowfield grew {hemp} hemp over {columns} columns"
        );
    }
}
