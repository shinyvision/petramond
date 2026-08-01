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

/// Per-column chance of a pebble field on bare ground. A WORLD CONSTANT rather
/// than a biome field: loose stone lies on every kind of ground, and the first
/// pickaxe is knapped from it, so a fresh player must find some wherever they
/// spawn. Deliberately sparse — you should have to look.
const PEBBLE_DENSITY: f32 = 0.008;
/// Hemp grows in small STANDS: an anchor column plus a short random walk out
/// of it, the farming pack's wild-crop patch shape (`WildCropSpec`) lifted into
/// core. Salt frozen — worldgen determinism depends on the literal.
///
/// THIS IS NOT A `patch_field` BLOB, and the reason is the whole point. A
/// smoothed field couples a patch's SIZE to its SPACING: both scale with the
/// period, so "small stands, far apart" is not a setting it has. Measured, with
/// `littercensus` clump sizes: period 8 gave a mean of 4 but merged into runs
/// of 13+ wherever grassland was continuous, and period 32 gave two stands per
/// 400 chunks of 12 and 23 stalks. An anchor plus a bounded walk separates the
/// two knobs — [`HEMP_STAND_STEPS`] is the size, the biome's anchor chance is
/// the frequency — and the size can never run away, because the walk stops.
const HEMP_ANCHOR_SALT: u64 = 0x0000_4E4D_7038_0001;
/// Stalks per stand, inclusive: the walk's step count. Some steps revisit a
/// cell and some land on ground that refuses, so the stand is at most this.
const HEMP_STAND_STEPS: (i32, i32) = (2, 6);
/// How far a walk may wander from its anchor — the largest step count minus
/// the anchor itself, so the clamp never bends a walk back into a blob.
const HEMP_STAND_REACH: i32 = 5;
/// Only columns on this lattice may ANCHOR a stand.
///
/// This is a COST decision, and a load-bearing one: membership is answered by
/// re-rolling every candidate anchor in reach, per column, so an anchor on
/// every column means `(2·reach+1)²` = 121 positional rolls for each grass
/// column in a hemp biome. Measured with `genmap 42 /dev/null top`, that was
/// **+30% on total generation time** (1.73s → 2.29s over 576 chunks) — far too
/// much for one plant. On a 4-block lattice the same scan is at most 9 rolls.
///
/// What it costs in return is that stands can only START on the lattice. That
/// is invisible: the walk out of the anchor is what gives a stand its shape,
/// and at roughly one stand per 16 chunks nothing lines up to be seen.
const HEMP_ANCHOR_GRID: i32 = 4;
// HEMP IS DENSER NEAR WATER, AND THAT IS A BIOME STATEMENT, NOT A HEIGHT ONE.
// The obvious cheap proxy — "how far is this column above sea level", which is
// exact here because the generator has no water table, so every surface water
// body sits at exactly `SEA_LEVEL` — was built, measured, and thrown away:
// `littercensus` puts 65% of all DRY land within two blocks of sea level, so
// boosting that band boosts most of the map and says nothing about water. The
// signal that does work is the one the vegetation pass already holds: swamp and
// wetland ARE the temperate water margins, and they carry several times the
// hemp of plains and forest (see each biome's `with_hemp`).
//
// What this deliberately does NOT catch is a river bank cutting through plains:
// that column is plain grass at plain height, and telling it apart needs a
// world-anchored surface scan at neighbouring offsets (the shape of the beach
// pass's `near_ocean_memo`) rather than anything a single column knows.

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

    if let Some(litter) = pick_litter(biome, surf, seed, wx, wz, rng) {
        return Some(litter);
    }

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
/// SNOW DOES NOT GATE THIS PASS, and that reversal (2026-07-31) is load-bearing.
/// Skipping snow-covered columns whole protected the look of a snowfield — a
/// litter cell takes the column's snow layer, so its grass goes green-topped —
/// and it cost the player the game: measured with `littercensus --biome
/// snowy_taiga`, 102k columns of snowy taiga and tundra carried ZERO pebbles,
/// zero branches and zero hemp, which is every ingredient of the first tool. A
/// snowy biome is not decoration, it is somewhere a fresh player can spawn. The
/// look survives anyway: those biomes already spend 12% of their columns on
/// ferns, which take the snow layer the same way, so litter at ~0.5% is
/// invisible against speckling that is twenty times denser.
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
        // Three sizes, evenly drawn, so a scattered field does not read as one
        // sprite stamped across the map.
        return Some(match rng.next_i32(0, 2) {
            0 => Block::PebblesSmall,
            1 => Block::PebblesMedium,
            _ => Block::PebblesLarge,
        });
    }

    // Hemp grows in small STANDS. The biome gate is checked FIRST and is what
    // keeps the anchor scan off the hot path: only grass in a hemp biome ever
    // pays for it, and the scan itself is one positional roll per candidate
    // anchor that almost always fails on that roll.
    let anchor_chance = spec.vegetation.hemp_anchor_chance;
    if anchor_chance > 0.0 && surf == Block::Grass && in_hemp_stand(seed, anchor_chance, wx, wz) {
        return Some(Block::Hemp);
    }
    None
}

/// Whether `(wx, wz)` lies in some hemp stand: is it an anchor, or on the walk
/// out of one within reach?
///
/// Anchor rolls and walk shapes are positional-RNG-pure per `(seed, anchor)`,
/// so every caller — either batch granularity, any chunk, any order — computes
/// the same answer for the same column, with no neighbour reads and therefore
/// no seam. That purity is also why the walk may be recomputed per column
/// instead of being memoised: it is cheaper than the bookkeeping would be, and
/// the anchor roll rejects almost every candidate before the walk starts.
fn in_hemp_stand(seed: u32, anchor_chance: f32, wx: i32, wz: i32) -> bool {
    /// The lattice points within `HEMP_STAND_REACH` of `v`.
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
            let mut rng = FeatureRng::positional(seed, HEMP_ANCHOR_SALT, ax, 0, az);
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
                // Clamped to the reach box, so a walk can never leave the
                // window every neighbouring column scans — which is what makes
                // the membership answer identical from either side.
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

/// The ground litter may lie on. Deliberately a SHORT list of bare, settled
/// surfaces (per Rachel) rather than "anything solid": litter on gravel, moss,
/// mycelium or a mountain's stone cap reads as debris rather than as something
/// a person would stop and pick up. Shared by the pebbles here and by the
/// branch scatter in `tree_select`, so the two cannot drift.
#[inline]
pub(crate) fn litter_ground(surf: Block) -> bool {
    matches!(
        surf,
        Block::Grass | Block::Dirt | Block::Sand | Block::Podzol
    )
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A snowfield must still hand a fresh player the makings of the first tool.
    ///
    /// This pass once skipped snow-covered columns whole, to protect the look of
    /// an unbroken snowfield, and the price was that every `SnowCover::Always`
    /// biome generated no pebbles and no hemp at all — measured at zero over
    /// 102k columns. A player can spawn in one of those biomes, and with logs
    /// axe-gated the gathering layer is the only way out, so the look is not
    /// worth the game. Re-gate this pass on snow and this fails.
    ///
    /// Hemp is the scarce half and the reason the sweep is this wide: cold hemp
    /// is deliberately an order below the temperate rate. (Fallen branches are
    /// the third ingredient and are placed by the TREE placer, not here.)
    #[test]
    fn a_snowfield_still_grows_the_ingredients_of_the_first_tool() {
        const SEED: u32 = 0x5EA5_04E5;
        // Wide enough that the hemp count has room to be retuned DOWN without
        // this reading as a flake: the sweep expects tens of stalks, not one.
        const SIDE: i32 = 600;

        let mut pebbles = 0;
        let mut hemp = 0;
        for wz in 0..SIDE {
            for wx in 0..SIDE {
                let mut rng = FeatureRng::positional(SEED, VEG_SALT, wx, 0, wz);
                match pick_litter(Biome::SnowyTaiga, Block::Grass, SEED, wx, wz, &mut rng) {
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
