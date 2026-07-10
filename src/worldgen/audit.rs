//! Worldgen-correctness oracles — the debris / relief / island / jaggedness
//! audits that measure whether the generator's output obeys its invariants.
//!
//! These deliberately live in the library (not the `genmap` previewer binary) so
//! they are testable under `cargo test`, reusable, and cannot drift from the
//! generator they measure. Each entry point GENERATES read-only chunks (or the
//! generator's post-lift regions) and returns a plain data struct of the metrics
//! it computed; the `genmap` binary prints those structs so its CLI output is
//! unchanged. The thresholds these struct fields are checked against are the
//! `mc-worldgen-jaggedness` family of invariants (≈0 floating debris, bounded
//! relief stdev, 0 mid-channel islands, …).
//!
//! Terrain-solidity is defined once via [`Block::is_terrain_solid`] (the
//! `Stone|Dirt|Grass|Sand|Snow` bare-ground set) so logs/leaves never swamp the
//! real terrain-overhang signal, and the flood scan shares the generic
//! [`flood_reachable`] helper.

use crate::biome::{Biome, BIOME_COUNT};
use crate::block::Block;
use crate::chunk::{Chunk, CHUNK_SX, CHUNK_SY, CHUNK_SZ, SEA_LEVEL};
use crate::worldgen::driver::ChunkGenerator;
use crate::worldgen::generate_chunk;

/// Highest non-air block in a column + its Y (mirrors the previewer's column
/// scan). Returns `(0, 0)` for an all-air column.
fn top_block(c: &Chunk, x: usize, z: usize) -> (u8, i32) {
    for y in (0..CHUNK_SY).rev() {
        let b = c.block_raw(x, y, z);
        if b != 0 {
            return (b, y as i32);
        }
    }
    (0, 0)
}

/// Highest terrain-solid block in a column + its Y. Tree logs/leaves, plants,
/// and built blocks are skipped so the roughness audit reads the natural ground
/// surface rather than being skewed by huge tree canopies (e.g. redwoods).
/// Returns `(0, 0)` for an all-terrain-air column.
fn terrain_top_block(c: &Chunk, x: usize, z: usize) -> (u8, i32) {
    for y in (0..CHUNK_SY).rev() {
        let b = c.block_raw(x, y, z);
        if is_terrain(b) {
            return (b, y as i32);
        }
    }
    (0, 0)
}

/// Is this raw block id terrain-solid (the bare-ground set, excluding tree
/// logs/leaves and built blocks)? The single terrain-solid predicate used by
/// every audit.
#[inline]
fn is_terrain(b: u8) -> bool {
    Block::from_id(b).is_terrain_solid()
}

#[inline]
fn intended_wet_biome(biome: Biome) -> bool {
    matches!(
        biome,
        Biome::Ocean | Biome::DeepOcean | Biome::Beach | Biome::River
    )
}

// ---------------------------------------------------------------------------
// Shared graph helpers
// ---------------------------------------------------------------------------

/// 6-connected flood through a `w × h × depth` occupancy grid (index
/// `(y*w + z)*w + x`), seeded from every occupied cell in the bottom (`y == 0`)
/// layer. Returns a `reach` mask the same length as `occ`: `true` where an
/// occupied cell is reachable from the floor. Iterative (explicit stack).
pub fn flood_reachable(occ: &[bool], w: usize, depth: usize) -> Vec<bool> {
    debug_assert_eq!(occ.len(), w * w * depth);
    let idx = |x: usize, y: usize, z: usize| (y * w + z) * w + x;
    let mut reach = vec![false; occ.len()];
    let mut stack: Vec<(usize, usize, usize)> = Vec::new();
    for z in 0..w {
        for x in 0..w {
            if occ[idx(x, 0, z)] {
                reach[idx(x, 0, z)] = true;
                stack.push((x, 0, z));
            }
        }
    }
    while let Some((x, y, z)) = stack.pop() {
        let mut push = |x: usize, y: usize, z: usize, st: &mut Vec<(usize, usize, usize)>| {
            let i = idx(x, y, z);
            if occ[i] && !reach[i] {
                reach[i] = true;
                st.push((x, y, z));
            }
        };
        if x + 1 < w {
            push(x + 1, y, z, &mut stack);
        }
        if x > 0 {
            push(x - 1, y, z, &mut stack);
        }
        if z + 1 < w {
            push(x, y, z + 1, &mut stack);
        }
        if z > 0 {
            push(x, y, z - 1, &mut stack);
        }
        if y + 1 < depth {
            push(x, y + 1, z, &mut stack);
        }
        if y > 0 {
            push(x, y - 1, z, &mut stack);
        }
    }
    reach
}

// ---------------------------------------------------------------------------
// Audit: overhangs + per-column floating debris + biome census
// ---------------------------------------------------------------------------

/// One biome's share of a sampled window: percent of columns + its name.
#[derive(Clone, Debug, PartialEq)]
pub struct BiomeShare {
    pub name: &'static str,
    pub percent: f64,
}

/// Result of [`audit`]: overhang ceilings, per-column floating debris, ocean
/// depth, the tallest column + its skin stack, the overhangiest column, and a
/// biome census — all over a 24×24-chunk window centred on the origin.
#[derive(Clone, Debug, PartialEq)]
pub struct DebrisAudit {
    pub seed: u32,
    /// Solid voxels with air directly below them (terrain overhang ceilings).
    pub overhang_ceilings: u64,
    /// Overhang voxels with NO solid anywhere below in their column (per-column
    /// detached-debris proxy — should be 0).
    pub floating_debris: u64,
    /// Highest solid floor under a water column, or -1 if no ocean column found.
    pub deepest_ocean_floor: i32,
    /// Tallest column's top-solid Y and its world (x, z).
    pub tallest_y: i32,
    pub tallest_xz: (i32, i32),
    /// `"y125:Stone y124:Stone …"` skin of the tallest column (top 7 blocks).
    pub tallest_skin: String,
    /// Most overhang ceilings in a single column + its world (x, z).
    pub overhangiest: u32,
    pub overhangiest_xz: (i32, i32),
    /// Biome census over the window, descending by share (only >0%).
    pub biomes: Vec<BiomeShare>,
}

/// Audit overhangs + floating debris across a region. An "overhang ceiling" is a
/// solid voxel with air directly below it. A "floating" voxel is solid with NO
/// solid anywhere below it in its column (true detached debris — should be ~0).
/// Also reports the deepest ocean column and the tallest column's skin stack.
pub fn audit(seed: u32) -> DebrisAudit {
    use crate::chunk::{SectionPos, SECTION_MIN_CY, SECTION_SIZE, WORLD_MIN_Y};

    let r: i32 = 12;
    let n = (r * 2) as usize;
    let generator = ChunkGenerator::new(seed);
    let mut overhang = 0u64;
    let mut floating = 0u64;
    let mut deepest_floor = i32::MAX;
    let (mut tall, mut tall_chunk, mut tall_xz) = (0i32, (0, 0), (0usize, 0usize));
    let (mut best_oh, mut oh_loc) = (0u32, (0i32, 0i32));
    let mut biome_counts = [0u32; BIOME_COUNT + 1];
    let mut total_cols = 0u32;
    for cz in 0..n {
        for cx in 0..n {
            let chunk = generate_chunk(seed, cx as i32 - r, cz as i32 - r);
            // The deep sections below the chunk window: the cubic world's floor.
            // Scanning them keeps the per-column floating metric honest — a deep
            // cavern crossing y = 0 is a roofed cave over solid rock, not
            // "floating" terrain (every column must ground out on the guaranteed
            // solid bottom band; a floater here means the floor guarantee broke).
            let col = generator.generate_column_gen(cx as i32 - r, cz as i32 - r);
            let deep: Vec<_> = (SECTION_MIN_CY..0)
                .map(|cy| {
                    generator
                        .generate_section(SectionPos::new(cx as i32 - r, cy, cz as i32 - r), &col)
                })
                .collect();
            let block_at = |x: usize, wy: i32, z: usize| -> u8 {
                if wy < 0 {
                    let si = ((wy - WORLD_MIN_Y) / SECTION_SIZE as i32) as usize;
                    deep[si].block_raw(x, wy.rem_euclid(SECTION_SIZE as i32) as usize, z)
                } else {
                    chunk.block_raw(x, wy as usize, z)
                }
            };
            for z in 0..CHUNK_SZ {
                for x in 0..CHUNK_SX {
                    let bid = chunk.biome_at(x, z) as usize;
                    if bid <= BIOME_COUNT {
                        biome_counts[bid] += 1;
                    }
                    total_cols += 1;
                    // ocean floor depth (highest solid where water sits above)
                    let (tb, ty) = top_block(&chunk, x, z);
                    if Block::from_id(tb) == Block::Water {
                        let mut fy = 0;
                        for y in (0..CHUNK_SY).rev() {
                            if is_terrain(chunk.block_raw(x, y, z)) {
                                fy = y as i32;
                                break;
                            }
                        }
                        deepest_floor = deepest_floor.min(fy);
                    } else if ty > tall {
                        tall = ty;
                        tall_chunk = (cx as i32 - r, cz as i32 - r);
                        tall_xz = (x, z);
                    }
                    // overhang + floating scan, over the FULL cubic depth
                    let mut solid_below = false;
                    let mut prev_solid = false;
                    let mut col_oh = 0u32;
                    for wy in WORLD_MIN_Y..CHUNK_SY as i32 {
                        let s = is_terrain(block_at(x, wy, z));
                        if s && wy > WORLD_MIN_Y && !prev_solid {
                            overhang += 1;
                            col_oh += 1;
                            if !solid_below {
                                floating += 1;
                            }
                        }
                        prev_solid = s;
                        if s {
                            solid_below = true;
                        }
                    }
                    if col_oh > best_oh {
                        best_oh = col_oh;
                        oh_loc = (
                            (cx as i32 - r) * CHUNK_SX as i32 + x as i32,
                            (cz as i32 - r) * CHUNK_SZ as i32 + z as i32,
                        );
                    }
                }
            }
        }
    }
    // tallest column skin stack
    let tc = generate_chunk(seed, tall_chunk.0, tall_chunk.1);
    let (tx, tz) = tall_xz;
    let mut stack = String::new();
    for y in (tall - 6..=tall).rev() {
        if y < 0 {
            break;
        }
        let b = Block::from_id(tc.block_raw(tx, y as usize, tz));
        stack.push_str(&format!("y{y}:{b:?} "));
    }
    let twx = tall_chunk.0 * CHUNK_SX as i32 + tx as i32;
    let twz = tall_chunk.1 * CHUNK_SZ as i32 + tz as i32;

    DebrisAudit {
        seed,
        overhang_ceilings: overhang,
        floating_debris: floating,
        deepest_ocean_floor: if deepest_floor == i32::MAX {
            -1
        } else {
            deepest_floor
        },
        tallest_y: tall,
        tallest_xz: (twx, twz),
        tallest_skin: stack,
        overhangiest: best_oh,
        overhangiest_xz: oh_loc,
        biomes: biome_census(&biome_counts, total_cols as f64),
    }
}

/// Build a descending biome census (only entries with >0% share) from a count
/// table indexed by biome id over `total` columns.
fn biome_census(counts: &[u32; BIOME_COUNT + 1], total: f64) -> Vec<BiomeShare> {
    let mut census: Vec<BiomeShare> = (1..=BIOME_COUNT as u8)
        .map(|id| BiomeShare {
            name: Biome::from_id(id).name(),
            percent: 100.0 * counts[id as usize] as f64 / total,
        })
        .filter(|s| s.percent > 0.0)
        .collect();
    census.sort_by(|a, b| b.percent.partial_cmp(&a.percent).unwrap());
    census
}

// ---------------------------------------------------------------------------
// Flood audit: true 3-D detached-debris census
// ---------------------------------------------------------------------------

/// Result of [`flood_audit`]: a true 3-D detached-debris census.
#[derive(Clone, Debug, PartialEq)]
pub struct FloodAudit {
    pub seed: u32,
    /// Total terrain-solid voxels in the region occupancy grid.
    pub solids: u64,
    /// Terrain-solid voxels NOT reachable by a 6-connected flood from the
    /// bedrock layer — genuine detached debris (should be ~0 / tiny ppm).
    pub detached_debris: u64,
    /// Region dimensions `(w, w, depth)` the census ran over.
    pub region: (usize, usize, usize),
}

impl FloodAudit {
    /// Detached debris as parts-per-million of solid terrain.
    pub fn ppm(&self) -> f64 {
        if self.solids == 0 {
            0.0
        } else {
            self.detached_debris as f64 / self.solids as f64 * 1_000_000.0
        }
    }
}

/// True 3-D floating-debris census: build a region occupancy grid, flood-fill
/// upward from the bedrock layer (6-connected, across chunk boundaries), and
/// count solid terrain voxels NOT reachable from the bottom — genuine detached
/// debris.
pub fn flood_audit(seed: u32) -> FloodAudit {
    let r: i32 = 8;
    let n = (r * 2) as usize;
    let w = n * CHUNK_SX;
    let hgt: usize = 190;
    let idx = |x: usize, y: usize, z: usize| (y * w + z) * w + x;
    let mut occ = vec![false; w * w * hgt];
    let mut solids: u64 = 0;
    for cz in 0..n {
        for cx in 0..n {
            let chunk = generate_chunk(seed, cx as i32 - r, cz as i32 - r);
            for z in 0..CHUNK_SZ {
                for x in 0..CHUNK_SX {
                    let gx = cx * CHUNK_SX + x;
                    let gz = cz * CHUNK_SZ + z;
                    for y in 0..hgt {
                        if is_terrain(chunk.block_raw(x, y, z)) {
                            occ[idx(gx, y, gz)] = true;
                            solids += 1;
                        }
                    }
                }
            }
        }
    }
    let reach = flood_reachable(&occ, w, hgt);
    let mut floaters: u64 = 0;
    for i in 0..occ.len() {
        if occ[i] && !reach[i] {
            floaters += 1;
        }
    }
    FloodAudit {
        seed,
        solids,
        detached_debris: floaters,
        region: (w, w, hgt),
    }
}

// ---------------------------------------------------------------------------
// Relief audit: lowland-relief diagnostic
// ---------------------------------------------------------------------------

/// Distribution summary of a set of surface heights.
#[derive(Clone, Debug, PartialEq)]
pub struct HeightStats {
    pub count: usize,
    pub min: i32,
    pub p10: i32,
    pub p50: i32,
    pub p90: i32,
    pub max: i32,
    pub mean: f64,
    pub stdev: f64,
}

impl HeightStats {
    fn from(values: &[i32]) -> Self {
        if values.is_empty() {
            return HeightStats {
                count: 0,
                min: 0,
                p10: 0,
                p50: 0,
                p90: 0,
                max: 0,
                mean: 0.0,
                stdev: 0.0,
            };
        }
        let n = values.len() as f64;
        let mean = values.iter().map(|&y| y as f64).sum::<f64>() / n;
        let var = values
            .iter()
            .map(|&y| (y as f64 - mean).powi(2))
            .sum::<f64>()
            / n;
        let mut sorted = values.to_vec();
        sorted.sort_unstable();
        let pct = |p: f64| sorted[((sorted.len() - 1) as f64 * p) as usize];
        HeightStats {
            count: values.len(),
            min: sorted[0],
            p10: pct(0.10),
            p50: pct(0.50),
            p90: pct(0.90),
            max: sorted[sorted.len() - 1],
            mean,
            stdev: var.sqrt(),
        }
    }
}

/// Result of [`relief_audit`]: land-biome surface relief over a window.
#[derive(Clone, Debug, PartialEq)]
pub struct ReliefStats {
    pub seed: u32,
    /// Window width/height in blocks (square).
    pub window_blocks: i32,
    /// Land-column surface-height distribution.
    pub land: HeightStats,
    /// Land columns whose top solid sits exactly at the waterline (`SEA_LEVEL`):
    /// dry land at sea level (coastal flats), the natural depth-zero clustering.
    pub at_waterline: u64,
    pub at_waterline_pct: f64,
    /// Land columns below the waterline (water laid on top — the pond-maze metric).
    pub flooded: u64,
    pub flooded_pct: f64,
    /// Coarse 2-block-bucket histogram from 62..=80 (shares, 10 buckets).
    pub hist_pct: [f64; 10],
}

/// Bucket labels for [`ReliefStats::hist_pct`].
pub const RELIEF_HIST_LABELS: [&str; 10] = [
    "62-63", "64-65", "66-67", "68-69", "70-71", "72-73", "74-75", "76-77", "78-79", "80+",
];

/// Land-biome surface-relief diagnostic over a 24×24-chunk window. Reports the
/// surface-height distribution, the at-waterline and flooded shares (both
/// measured against `SEA_LEVEL`), and a coarse height histogram.
pub fn relief_audit(seed: u32) -> ReliefStats {
    let gen = ChunkGenerator::new(seed);
    let r: i32 = 12; // 24x24 chunks = 384x384 blocks

    let mut land: Vec<i32> = Vec::new();
    let mut at_waterline = 0u64;
    let mut flooded = 0u64;
    for cz in -r..r {
        for cx in -r..r {
            let region = gen.region(cx, cz);
            let (ox, oz) = (cx * 16, cz * 16);
            for lz in 0..16i32 {
                for lx in 0..16i32 {
                    let wx = ox + lx;
                    let wz = oz + lz;
                    let i = (wz - region.z0) as usize * region.w + (wx - region.x0) as usize;
                    if intended_wet_biome(region.biomes[i]) {
                        continue;
                    }
                    let surf = region.surf[i];
                    land.push(surf);
                    // Top solid exactly at SEA_LEVEL is dry land at the waterline;
                    // below it, water is laid on top (genuinely flooded).
                    if surf == SEA_LEVEL {
                        at_waterline += 1;
                    } else if surf < SEA_LEVEL {
                        flooded += 1;
                    }
                }
            }
        }
    }

    // Coarse histogram, 2-block buckets from 62..=80.
    let n = land.len().max(1) as f64;
    let mut hist = [0u64; 10]; // buckets [62,64),[64,66),...,[80,82)
    for &y in &land {
        let b = ((y - 62).clamp(0, 19) / 2) as usize;
        hist[b.min(9)] += 1;
    }
    let hist_pct = {
        let mut out = [0.0f64; 10];
        for (o, &c) in out.iter_mut().zip(hist.iter()) {
            *o = c as f64 / n * 100.0;
        }
        out
    };

    ReliefStats {
        seed,
        window_blocks: r * 32,
        land: HeightStats::from(&land),
        at_waterline,
        at_waterline_pct: at_waterline as f64 / n * 100.0,
        flooded,
        flooded_pct: flooded as f64 / n * 100.0,
        hist_pct,
    }
}

// ---------------------------------------------------------------------------
// Roughness: walkability / spikiness metric
// ---------------------------------------------------------------------------

/// Result of [`roughness`]: surface walkability/spikiness over mountain-like
/// biome columns of a 24×24-chunk window.
#[derive(Clone, Debug, PartialEq)]
pub struct RoughnessStats {
    pub seed: u32,
    /// Number of mountain-like biome columns.
    pub mountain_cols: u64,
    /// Mean of each column's steepest neighbour step.
    pub mean_max_step: f64,
    /// Columns that stick ≥4 above ALL four neighbours (isolated spikes).
    pub pillar_pct: f64,
    /// Columns whose steepest neighbour step is ≤2 (you can stand/walk).
    pub walkable_pct: f64,
    /// Max-step histogram shares for buckets `0,1,2,3,4,5+`.
    pub max_step_hist_pct: [f64; 6],
}

/// Walkability / spikiness metric. For mountain-like biome columns, reports how
/// steep the surface is between neighbours — the thing
/// cross-sections and hillshades hide but that turns a mountain into a field of
/// 1-wide pillars. `pillar%` = columns ≥4 above all four neighbours;
/// `walkable%` = columns whose steepest neighbour step is ≤2. Returns `None`
/// when the window holds no mountain columns.
pub fn roughness(seed: u32) -> Option<RoughnessStats> {
    let r: i32 = 12;
    // Peak biomes are rare at climate scale and seldom fall in the origin window,
    // so centre the analysis window on a mountainous region located by a cheap
    // biome scan. `None` only if the world has no mountains within the search.
    let (ccx, ccz) = find_mountain_chunk(seed)?;
    let n = (r * 2) as usize;
    let w = n * CHUNK_SX;
    let mut surf = vec![0i32; w * w];
    let mut mountain_like = vec![false; w * w];
    for cz in 0..n {
        for cx in 0..n {
            let chunk = generate_chunk(seed, ccx - r + cx as i32, ccz - r + cz as i32);
            for z in 0..CHUNK_SZ {
                for x in 0..CHUNK_SX {
                    let i = (cz * CHUNK_SZ + z) * w + (cx * CHUNK_SX + x);
                    let (_, y) = terrain_top_block(&chunk, x, z);
                    surf[i] = y;
                    mountain_like[i] = is_mountain_like(Biome::from_id(chunk.biome_at(x, z)));
                }
            }
        }
    }
    let at = |x: i32, z: i32| {
        surf[(z.clamp(0, w as i32 - 1) as usize) * w + x.clamp(0, w as i32 - 1) as usize]
    };
    let (mut mtn, mut pillars, mut walkable) = (0u64, 0u64, 0u64);
    let mut step_sum = 0i64;
    let mut steps_hist = [0u64; 6]; // 0,1,2,3,4,5+ block max-step buckets
    for z in 1..w as i32 - 1 {
        for x in 1..w as i32 - 1 {
            let h = at(x, z);
            if !mountain_like[z as usize * w + x as usize] {
                continue;
            }
            mtn += 1;
            let nb = [at(x + 1, z), at(x - 1, z), at(x, z + 1), at(x, z - 1)];
            let max_step = nb.iter().map(|&v| (h - v).abs()).max().unwrap();
            let above_all = nb.iter().all(|&v| h - v >= 4);
            step_sum += max_step as i64;
            steps_hist[(max_step.min(5)) as usize] += 1;
            if above_all {
                pillars += 1;
            }
            if max_step <= 2 {
                walkable += 1;
            }
        }
    }
    if mtn == 0 {
        return None;
    }
    let pct = |v: u64| 100.0 * v as f64 / mtn as f64;
    let mut hist_pct = [0.0f64; 6];
    for (o, &c) in hist_pct.iter_mut().zip(steps_hist.iter()) {
        *o = pct(c);
    }
    Some(RoughnessStats {
        seed,
        mountain_cols: mtn,
        mean_max_step: step_sum as f64 / mtn as f64,
        pillar_pct: pct(pillars),
        walkable_pct: pct(walkable),
        max_step_hist_pct: hist_pct,
    })
}

/// Find a chunk near a mountainous region by scanning biomes coarsely outward.
/// Returns the chunk coordinates of the first mountain-like column, or `None` if
/// no mountains exist within the search radius.
fn find_mountain_chunk(seed: u32) -> Option<(i32, i32)> {
    let gen = ChunkGenerator::new(seed);
    for wz in (-6000..6000).step_by(64) {
        for wx in (-6000..6000).step_by(64) {
            if is_mountain_like(gen.biome_at(wx, wz)) {
                return Some((wx.div_euclid(16), wz.div_euclid(16)));
            }
        }
    }
    None
}

#[inline]
fn is_mountain_like(biome: Biome) -> bool {
    matches!(
        biome,
        Biome::Mountains
            | Biome::SnowyPeaks
            | Biome::Foothills
            | Biome::Grove
            | Biome::SnowySlopes
            | Biome::WindsweptHills
            | Biome::StonyPeaks
            | Biome::WoodedHills
            | Biome::MountainEdge
    )
}

// ---------------------------------------------------------------------------
// Tests — the audits now run under `cargo test`. Values captured from the
// current generator for the default seed 0x1234_5678 (see commit baseline).
// These pin the `mc-worldgen-jaggedness` family of invariants.
// ---------------------------------------------------------------------------
#[cfg(all(test, feature = "worldgen-tests"))]
mod tests {
    use super::*;

    const SEED: u32 = 0x1234_5678;

    #[test]
    fn flood_reachable_marks_only_floor_connected_solids() {
        // 2x2 footprint, depth 3. A grounded column at (0,0) full height, and a
        // single detached voxel at (1,1,2) with nothing under it.
        let w = 2usize;
        let depth = 3usize;
        let idx = |x: usize, y: usize, z: usize| (y * w + z) * w + x;
        let mut occ = vec![false; w * w * depth];
        for y in 0..depth {
            occ[idx(0, y, 0)] = true; // grounded pillar
        }
        occ[idx(1, 2, 1)] = true; // floating voxel (no floor under it)
        let reach = flood_reachable(&occ, w, depth);
        assert!(
            reach[idx(0, 0, 0)] && reach[idx(0, 2, 0)],
            "pillar reachable"
        );
        assert!(!reach[idx(1, 2, 1)], "detached voxel must be unreachable");
        let floaters = (0..occ.len()).filter(|&i| occ[i] && !reach[i]).count();
        assert_eq!(floaters, 1);
    }

    /// Floating-debris invariant: the per-column overhang scan finds ZERO truly
    /// floating voxels (a solid with no solid anywhere below it in the column).
    /// The genmap `audit` mode pins this at 0.
    #[test]
    fn audit_has_zero_per_column_floating_debris() {
        let a = audit(SEED);
        assert_eq!(
            a.floating_debris, 0,
            "per-column floating debris must be 0, got {}",
            a.floating_debris
        );
        assert!(a.overhang_ceilings > 0);
        assert!(a.tallest_y > 0);
        assert!(a.overhangiest > 0);
    }

    /// True 3-D detached-debris census stays within the documented tiny bound.
    /// The invariant is "near-zero debris"; assert it stays well under 100 ppm.
    #[test]
    fn flood_audit_detached_debris_within_bound() {
        let f = flood_audit(SEED);
        assert!(f.solids > 0);
        assert_eq!(f.region, (256, 256, 190));
        assert!(
            f.ppm() < 100.0,
            "detached-debris must stay a tiny ppm of solid terrain, got {:.1} ppm",
            f.ppm()
        );
    }

    /// Relief sanity invariant: the land-biome surface keeps real rolling relief
    /// (stdev well above 0 — NOT a collapsed flat plane), rises well above the
    /// waterline, and the bulk of land biomes are dry. Near-coast lowland that
    /// dips below sea level is expected (the depth field naturally crosses the
    /// waterline), so this guards only against a catastrophic regression where
    /// terrain sinks or flattens wholesale — not a tight flooded-share pin.
    #[test]
    fn relief_audit_has_real_relief_and_mostly_dry_land() {
        let r = relief_audit(SEED);
        assert_eq!(r.window_blocks, 384);
        assert!(r.land.count > 0, "window must contain land-biome columns");
        assert!(
            r.land.stdev > 1.0,
            "relief collapsed to flat: stdev {:.3}",
            r.land.stdev
        );
        assert!(
            r.land.max >= SEA_LEVEL + 8,
            "no terrain rises above sea level: max {}",
            r.land.max
        );
        assert!(
            r.flooded_pct < 50.0,
            "majority of land sank below sea level: {:.1}%",
            r.flooded_pct
        );
    }

    /// Jaggedness invariant: mountains are walkable ranges, not a field of
    /// 1-wide pillars.
    #[test]
    fn roughness_mountains_are_walkable_not_pillars() {
        let s = roughness(SEED).expect("window has mountain columns");
        assert!(s.mountain_cols > 0);
        assert!(
            s.pillar_pct < 1.0,
            "mountain-like biomes should not become isolated spike fields"
        );
        assert!(
            s.walkable_pct > 45.0,
            "mountains must be mostly walkable, got {:.1}%",
            s.walkable_pct
        );
    }
}
