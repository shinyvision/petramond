//! Underground scatter features — ore veins + stone / dirt / gravel blobs.
//!
//! This is the `DecoStep::RawGeneration` + `DecoStep::Ores` content: small
//! ellipsoidal veins that overwrite Stone below the surface, turning the old
//! solid-grey monolith into varied rock with ores. It is intentionally simpler
//! than the tree placement path (no per-column spacing / biome roll): a fixed set
//! of `ScatterConfig` rows, each producing `count` veins per chunk.
//!
//! Seam handling mirrors the tree pass without a margin buffer: every chunk
//! regenerates its 3x3 neighbourhood's veins from a positional RNG keyed on the
//! ORIGIN chunk (`positional(seed, salt, ncx, vein, ncz)`), and writes only the
//! cells that fall inside itself (`FeatureCtx` clips). A vein straddling a chunk
//! border is therefore materialised identically from both sides — no seam, no
//! double-placement — because both chunks derive the exact same vein.

use crate::block::Block;
use crate::chunk::{Chunk, CHUNK_SY};
use crate::mathh::IVec3;
use crate::section::Section;

use super::super::rng::FeatureRng;
use super::{ChunkSink, FeatureCtx, SectionSink};

/// One scatter species: a block that overwrites Stone in `count` veins per chunk,
/// each ~`size` blocks, within a world-Y band.
struct ScatterConfig {
    block: Block,
    salt: u64,
    count: i32,
    size: i32,
    y_min: i32,
    y_max: i32,
}

const fn c(
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
        size,
        y_min,
        y_max,
    }
}

/// Vein table. Stone variants are broad and shallow-to-mid; ores follow rough
/// typical rarity/altitude (rescaled to our y in [0,256], sea level 64). Order is
/// fixed (deterministic placement); rarer/deeper ores sit later but every vein
/// only overwrites Stone, so overlaps just leave the earlier block in place.
static CONFIGS: &[ScatterConfig] = &[
    // Stone variants — the bulk of the "not solid grey" win.
    c(Block::Granite, 0xA1_0001, 7, 48, 4, 110),
    c(Block::Diorite, 0xA1_0002, 7, 48, 4, 110),
    c(Block::Andesite, 0xA1_0003, 7, 48, 4, 110),
    c(Block::Tuff, 0xA1_0004, 3, 40, 4, 36),
    // Underground dirt / gravel pockets.
    c(Block::Dirt, 0xA1_0005, 6, 33, 4, 130),
    c(Block::Gravel, 0xA1_0006, 7, 33, 4, 130),
    // Ores.
    c(Block::CoalOre, 0xA1_0010, 16, 17, 8, 150),
    c(Block::IronOre, 0xA1_0011, 18, 9, 4, 120),
    c(Block::CopperOre, 0xA1_0012, 12, 10, 30, 96),
    c(Block::GoldOre, 0xA1_0013, 3, 9, 4, 36),
    c(Block::RedstoneOre, 0xA1_0014, 6, 8, 4, 22),
    c(Block::LapisOre, 0xA1_0015, 2, 7, 4, 40),
    c(Block::DiamondOre, 0xA1_0016, 2, 8, 4, 16),
    c(Block::EmeraldOre, 0xA1_0017, 5, 4, 92, 180),
];

/// Place all underground veins for `chunk`. Pure function of `(seed, cx, cz)`.
pub fn place_underground(chunk: &mut Chunk, seed: u32) {
    let (ccx, ccz) = (chunk.cx, chunk.cz);
    let mut sink = ChunkSink::new(chunk);
    let mut ctx = FeatureCtx::new(&mut sink);
    place_underground_into(&mut ctx, ccx, ccz, seed);
}

/// Cubic per-section scatter: run the SAME 3×3-column vein loop into one 16³
/// [`Section`] through a [`SectionSink`]. Veins are keyed on the ORIGIN column
/// (`positional(seed, salt, ncx, vein, ncz)`) exactly as the chunk path, and only
/// overwrite Stone, so a vein straddling a section seam (horizontal OR vertical) is
/// materialised identically from every section it touches — byte-parity with the
/// whole-column pass for this section's slab.
pub fn place_underground_section(section: &mut Section, seed: u32) {
    let (ccx, ccz) = (section.cx, section.cz);
    let mut sink = SectionSink::new(section);
    let mut ctx = FeatureCtx::new(&mut sink);
    place_underground_into(&mut ctx, ccx, ccz, seed);
}

/// The shared vein loop: regenerate every vein of the 3×3 column neighbourhood around
/// `(ccx,ccz)` into `ctx`, whose sink clips to the caller's target (chunk or section).
fn place_underground_into(ctx: &mut FeatureCtx, ccx: i32, ccz: i32, seed: u32) {
    // 3x3 neighbourhood so border-straddling veins appear from both sides.
    for dcz in -1..=1 {
        for dcx in -1..=1 {
            let ncx = ccx + dcx;
            let ncz = ccz + dcz;
            for cfg in CONFIGS {
                for i in 0..cfg.count {
                    let mut rng = FeatureRng::positional(seed, cfg.salt, ncx, i, ncz);
                    let ox = ncx * 16 + rng.next_i32(0, 15);
                    let oz = ncz * 16 + rng.next_i32(0, 15);
                    let oy = rng.next_i32(cfg.y_min, cfg.y_max);
                    place_vein(ctx, ox, oy, oz, cfg.size, cfg.block, &mut rng);
                }
            }
        }
    }
}

/// World-Y span the scatter veins can possibly touch (the union of every config's
/// `[y_min,y_max]` widened by the largest vein radius), so the cubic generator can
/// skip the deep / high sections a vein can never reach. The widest config is `size`
/// 48 → radius ≈ 3, so a ±4 pad is safe.
pub const SCATTER_MIN_Y: i32 = 0;
pub const SCATTER_MAX_Y: i32 = 184;

/// A roughly-spherical blob of `~size` Stone cells turned into `block`, with a
/// small per-vein radius jitter so veins read irregular rather than as clean
/// spheres. Writes are Stone-only and chunk-clipped.
fn place_vein(
    ctx: &mut FeatureCtx,
    ox: i32,
    oy: i32,
    oz: i32,
    size: i32,
    block: Block,
    rng: &mut FeatureRng,
) {
    // radius for a sphere of `size` cells: r = cbrt(3*size / 4pi).
    let base_r = ((size as f32) * 3.0 / (4.0 * std::f32::consts::PI)).cbrt();
    let r = (base_r * (0.85 + 0.4 * rng.next_f32())).max(0.7);
    let ri = r.ceil() as i32;
    let r2 = r * r;
    for dy in -ri..=ri {
        let y = oy + dy;
        if y < 1 || y >= CHUNK_SY as i32 - 1 {
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
