//! Underground scatter features — ore veins + dirt / gravel / tuff blobs.
//!
//! This is the `DecoStep::RawGeneration` + `DecoStep::Ores` content: small
//! veins that overwrite Stone below the surface, spanning the FULL cubic world
//! depth (down to `WORLD_MIN_Y`). Two vein shapes:
//!   - [`VeinShape::Blob`]: a roughly-spherical blob of `~size` cells (dirt,
//!     gravel, tuff, and the bulk ores).
//!   - [`VeinShape::Grid3`]: a single-layer 3×3 patch holding 1..=9 ore blocks —
//!     the iron/diamond rule: a vein always fits a 3×3 area and never exceeds 9.
//! A config may carry a [`DepthRamp`]: each rolled vein is then only accepted
//! with a chance that grows quadratically toward the bottom of its Y band —
//! diamonds get more likely the deeper you dig, yet stay rare even at the floor.
//!
//! Seam handling mirrors the tree pass without a margin buffer: every chunk
//! regenerates its 3x3 neighbourhood's veins from a positional RNG keyed on the
//! ORIGIN chunk (`positional(seed, salt, ncx, vein, ncz)`), and writes only the
//! cells that fall inside itself (`FeatureCtx` clips). A vein straddling a chunk
//! border is therefore materialised identically from both sides — no seam, no
//! double-placement — because both chunks derive the exact same vein.

use crate::block::Block;
use crate::chunk::{Chunk, WORLD_MAX_Y, WORLD_MIN_Y};
use crate::mathh::IVec3;
use crate::section::Section;

use super::super::rng::FeatureRng;
use super::sink::SinkTarget;
use super::{ChunkSink, FeatureCtx, SectionSink};

/// How a vein materialises its cells around the rolled origin.
enum VeinShape {
    /// Roughly-spherical blob of `~size` cells with per-vein radius jitter.
    Blob { size: i32 },
    /// One horizontal 3×3 layer centred on the origin holding exactly
    /// `1..=max_ore` ore cells (uniformly chosen among the 9 slots).
    Grid3 { max_ore: i32 },
}

/// Per-vein acceptance chance ramping toward the BOTTOM of the config's Y band:
/// `chance(y) = max_chance · t²` with `t = (y_max − y) / (y_max − y_min)`.
struct DepthRamp {
    max_chance: f32,
}

/// One scatter species: a block that overwrites Stone in up to `count` veins per
/// chunk within a world-Y band.
struct ScatterConfig {
    block: Block,
    salt: u64,
    count: i32,
    shape: VeinShape,
    y_min: i32,
    y_max: i32,
    ramp: Option<DepthRamp>,
}

impl ScatterConfig {
    /// Conservative `(horizontal, vertical)` reach of one vein from its rolled
    /// origin, in cells: every write lands within this Chebyshev box. Blob radius
    /// is `base_r × (0.85 + 0.4·f)` with `f < 1`, so `ceil(base_r × 1.25)` bounds
    /// `ceil(r)` (f32 multiply is monotone; an exact-integer bound still holds
    /// because `r` is strictly below it). Grid3 writes one 3×3 layer.
    fn reach(&self) -> (i32, i32) {
        match self.shape {
            VeinShape::Blob { size } => {
                let r = (blob_base_radius(size) * 1.25).ceil() as i32;
                (r, r)
            }
            VeinShape::Grid3 { .. } => (1, 0),
        }
    }
}

/// The widest horizontal reach across [`CONFIGS`] — the column-level reject bound.
fn max_config_reach() -> i32 {
    static MAX: std::sync::OnceLock<i32> = std::sync::OnceLock::new();
    *MAX.get_or_init(|| CONFIGS.iter().map(|c| c.reach().0).max().unwrap_or(0))
}

/// Radius for a blob of `size` cells: `r = cbrt(3·size / 4π)` — shared by the
/// materialiser and the reach bound so they can never drift apart.
#[inline]
fn blob_base_radius(size: i32) -> f32 {
    ((size as f32) * 3.0 / (4.0 * std::f32::consts::PI)).cbrt()
}

const fn blob(
    block: Block,
    salt: u64,
    count: i32,
    size: i32,
    y_min: i32,
    y_max: i32,
) -> ScatterConfig {
    ScatterConfig {
        block,
        salt,
        count,
        shape: VeinShape::Blob { size },
        y_min,
        y_max,
        ramp: None,
    }
}

const fn grid3(
    block: Block,
    salt: u64,
    count: i32,
    y_min: i32,
    y_max: i32,
    ramp: Option<DepthRamp>,
) -> ScatterConfig {
    ScatterConfig {
        block,
        salt,
        count,
        shape: VeinShape::Grid3 { max_ore: 9 },
        y_min,
        y_max,
        ramp,
    }
}

/// Vein table, spanning the full cubic depth. Order is fixed (deterministic
/// placement); every vein only overwrites Stone, so overlaps just leave the
/// earlier block in place.
///
/// Y bands (world Y, floor −64, sea level 63):
///   - dirt/gravel pockets ride the whole underground;
///   - tuff is the deep stratum flavour below y 0;
///   - coal stays shallow-to-mid, copper mid, gold deep;
///   - iron is a Grid3 vein (≤9 ore in a 3×3 layer) across the WHOLE depth, at
///     the same veins-per-volume rate as before — as common, less plentiful;
///   - diamond is a Grid3 vein on a depth ramp: absent above y 14, increasingly
///     likely toward the floor, yet rare even there.
static CONFIGS: &[ScatterConfig] = &[
    // Underground dirt / gravel pockets, all the way down.
    blob(Block::Dirt, 0xA1_0005, 9, 33, WORLD_MIN_Y, 130),
    blob(Block::Gravel, 0xA1_0006, 10, 33, WORLD_MIN_Y, 130),
    // Tuff: the deep-stratum stone flavour.
    blob(Block::Tuff, 0xA1_0004, 6, 40, WORLD_MIN_Y, 0),
    // Ores.
    blob(Block::CoalOre, 0xA1_0010, 14, 17, 16, 150),
    blob(Block::CopperOre, 0xA1_0012, 11, 10, -16, 96),
    grid3(Block::IronOre, 0xA1_0011, 34, WORLD_MIN_Y, 136, None),
    blob(Block::GoldOre, 0xA1_0013, 3, 9, WORLD_MIN_Y, 30),
    grid3(
        Block::DiamondOre,
        0xA1_0016,
        7,
        WORLD_MIN_Y,
        16,
        Some(DepthRamp { max_chance: 1.0 }),
    ),
];

/// Place all underground veins for `chunk`. Pure function of `(seed, cx, cz)`.
pub fn place_underground(chunk: &mut Chunk, seed: u32) {
    let (ccx, ccz) = (chunk.cx, chunk.cz);
    let clip = clip_box_of(chunk.world_box());
    let mut sink = ChunkSink::new(chunk);
    let mut ctx = FeatureCtx::new(&mut sink);
    place_underground_into(&mut ctx, clip, ccx, ccz, seed);
}

/// Cubic per-section scatter: run the SAME 3×3-column vein loop into one 16³
/// [`Section`] through a [`SectionSink`]. Veins are keyed on the ORIGIN column
/// (`positional(seed, salt, ncx, vein, ncz)`) exactly as the chunk path, and only
/// overwrite Stone, so a vein straddling a section seam (horizontal OR vertical) is
/// materialised identically from every section it touches — byte-parity with the
/// whole-column pass for this section's slab.
pub fn place_underground_section(section: &mut Section, seed: u32) {
    let (ccx, ccz) = (section.cx, section.cz);
    let clip = clip_box_of(section.world_box());
    let mut sink = SectionSink::new(section);
    let mut ctx = FeatureCtx::new(&mut sink);
    place_underground_into(&mut ctx, clip, ccx, ccz, seed);
}

/// Inclusive world-coordinate bounds of a sink target's writable footprint.
fn clip_box_of((origin, size): (IVec3, IVec3)) -> (IVec3, IVec3) {
    (origin, origin + size - IVec3::splat(1))
}

/// The shared vein loop: regenerate every vein of the 3×3 column neighbourhood around
/// `(ccx,ccz)` into `ctx`, whose sink clips to the caller's target (chunk or section).
///
/// `clip` is that target's inclusive writable box: a vein whose conservative reach
/// box around its rolled origin cannot intersect it is skipped WITHOUT
/// materialising its cells. Byte-identical: every vein derives from its own
/// positional RNG (`(seed, salt, ncx, i, ncz)`), so skipping one vein's remaining
/// draws can never shift another vein's, and a skipped vein's whole write set was
/// outside the sink's clip anyway. This is what makes the 16-tall section path
/// cheap — most of the 3×3 neighbourhood's full-depth veins miss one section's slab.
fn place_underground_into(
    ctx: &mut FeatureCtx,
    clip: (IVec3, IVec3),
    ccx: i32,
    ccz: i32,
    seed: u32,
) {
    let (clip_min, clip_max) = clip;
    // 3x3 neighbourhood so border-straddling veins appear from both sides.
    for dcz in -1..=1 {
        for dcx in -1..=1 {
            let ncx = ccx + dcx;
            let ncz = ccz + dcz;
            // Column-level reject: no origin in this 16×16 column can reach the
            // clip box horizontally (origins span the column; reach ≤ the widest
            // vein's radius).
            let max_r = max_config_reach();
            if (ncx * 16 + 15 + max_r) < clip_min.x
                || (ncx * 16 - max_r) > clip_max.x
                || (ncz * 16 + 15 + max_r) < clip_min.z
                || (ncz * 16 - max_r) > clip_max.z
            {
                continue;
            }
            for cfg in CONFIGS {
                let (rxz, ry) = cfg.reach();
                // Band-level reject: the whole config's Y band is out of reach.
                if cfg.y_max + ry < clip_min.y || cfg.y_min - ry > clip_max.y {
                    continue;
                }
                for i in 0..cfg.count {
                    let mut rng = FeatureRng::positional(seed, cfg.salt, ncx, i, ncz);
                    let ox = ncx * 16 + rng.next_i32(0, 15);
                    let oz = ncz * 16 + rng.next_i32(0, 15);
                    let oy = rng.next_i32(cfg.y_min, cfg.y_max);
                    if oy + ry < clip_min.y
                        || oy - ry > clip_max.y
                        || ox + rxz < clip_min.x
                        || ox - rxz > clip_max.x
                        || oz + rxz < clip_min.z
                        || oz - rxz > clip_max.z
                    {
                        continue;
                    }
                    if let Some(ramp) = &cfg.ramp {
                        // Deeper = likelier: quadratic ease toward the band floor.
                        let t = (cfg.y_max - oy) as f32 / (cfg.y_max - cfg.y_min) as f32;
                        if !rng.chance(ramp.max_chance * t * t) {
                            continue;
                        }
                    }
                    match cfg.shape {
                        VeinShape::Blob { size } => {
                            place_blob_vein(ctx, ox, oy, oz, size, cfg.block, &mut rng)
                        }
                        VeinShape::Grid3 { max_ore } => {
                            place_grid3_vein(ctx, ox, oy, oz, max_ore, cfg.block, &mut rng)
                        }
                    }
                }
            }
        }
    }
}

/// World-Y span the scatter veins can possibly touch (the union of every config's
/// `[y_min,y_max]` widened by the largest vein radius), so the cubic generator can
/// skip the deep / high sections a vein can never reach. The widest config is `size`
/// 40 → radius ≈ 3, so a ±4 pad is safe; the low end clamps at the world floor.
pub const SCATTER_MIN_Y: i32 = WORLD_MIN_Y;
pub const SCATTER_MAX_Y: i32 = 154;

/// Keep the world-floor layer solid stone and never write above the world top.
#[inline]
fn vein_y_in_world(y: i32) -> bool {
    y > WORLD_MIN_Y && y < WORLD_MAX_Y
}

/// A roughly-spherical blob of `~size` Stone cells turned into `block`, with a
/// small per-vein radius jitter so veins read irregular rather than as clean
/// spheres. Writes are Stone-only and chunk-clipped.
fn place_blob_vein(
    ctx: &mut FeatureCtx,
    ox: i32,
    oy: i32,
    oz: i32,
    size: i32,
    block: Block,
    rng: &mut FeatureRng,
) {
    let r = (blob_base_radius(size) * (0.85 + 0.4 * rng.next_f32())).max(0.7);
    let ri = r.ceil() as i32;
    let r2 = r * r;
    for dy in -ri..=ri {
        let y = oy + dy;
        if !vein_y_in_world(y) {
            continue;
        }
        for dz in -ri..=ri {
            for dx in -ri..=ri {
                let d2 = (dx * dx + dy * dy + dz * dz) as f32;
                if d2 <= r2 {
                    ctx.replace_block(IVec3::new(ox + dx, y, oz + dz), Block::Stone, block);
                }
            }
        }
    }
}

/// The iron/diamond vein shape: exactly `1..=max_ore` cells of `block` chosen
/// uniformly among the 3×3 slots of one horizontal layer centred on the origin —
/// a vein always fits a 3×3 area and never holds more than 9 ore blocks. Writes
/// are Stone-only and chunk-clipped (a slot occupied by cave air, dirt, or an
/// earlier vein simply stays as it is).
fn place_grid3_vein(
    ctx: &mut FeatureCtx,
    ox: i32,
    oy: i32,
    oz: i32,
    max_ore: i32,
    block: Block,
    rng: &mut FeatureRng,
) {
    if !vein_y_in_world(oy) {
        return;
    }
    let mut remaining = rng.next_i32(1, max_ore);
    let mut slots_left = 9;
    for dz in -1..=1 {
        for dx in -1..=1 {
            // Reservoir pick: exactly `remaining` of the `slots_left` slots get
            // ore, uniformly, in one fixed deterministic pass.
            if rng.next_i32(0, slots_left - 1) < remaining {
                ctx.replace_block(IVec3::new(ox + dx, oy, oz + dz), Block::Stone, block);
                remaining -= 1;
            }
            slots_left -= 1;
        }
    }
}
