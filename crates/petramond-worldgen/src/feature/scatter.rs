//! Underground scatter features — ore veins + dirt / gravel / tuff blobs.
//!
//! This is the `DecoStep::RawGeneration` + `DecoStep::Ores` content: small
//! veins that overwrite their host blocks (stone, unless a row says
//! otherwise) below the surface, spanning the FULL cubic world depth (down to
//! `WORLD_MIN_Y`). The vein table is data — `assets/ores.json`, loaded by
//! [`crate::data::ores`] — and this pass interprets it. Two vein shapes:
//!   - `VeinShape::Blob`: a roughly-spherical blob of `~size` cells (dirt,
//!     gravel, tuff, and the bulk ores).
//!   - `VeinShape::Grid3`: a single-layer 3×3 patch holding 1..=9 ore
//!     blocks — the iron/diamond rule: a vein always fits a 3×3 area and never
//!     exceeds 9.
//!
//! A row may carry a depth ramp: each rolled vein is then only accepted
//! with a chance that grows quadratically toward the bottom of its Y band —
//! diamonds get more likely the deeper you dig, yet stay rare even at the floor.
//!
//! Seam handling mirrors the tree pass without a margin buffer: every section
//! regenerates its 3x3 column neighbourhood's veins from a positional RNG keyed
//! on the ORIGIN column (`positional(seed, salt, ncx, vein, ncz)`), and writes
//! only the cells that fall inside itself (`FeatureCtx` clips). A vein
//! straddling a border is therefore materialised identically from both sides —
//! no seam, no double-placement — because both derive the exact same vein.

use petramond_world::block::Block;
use petramond_world::chunk::{WORLD_MAX_Y, WORLD_MIN_Y};
use petramond_world::mathh::IVec3;
use petramond_world::section::Section;

use super::super::rng::FeatureRng;
use super::sink::SinkTarget;
use super::{FeatureCtx, SectionSink};
use crate::data::ores::{blob_base_radius, OreTable, VeinShape};

/// Place the underground veins reaching one 16³ [`Section`], through a
/// [`SectionSink`]: every section regenerates its 3×3 column neighbourhood's veins.
/// Veins are keyed on the ORIGIN column (`positional(seed, salt, ncx, vein, ncz)`)
/// and only overwrite their hosts, so a vein straddling a section seam
/// (horizontal OR vertical) is materialised identically from every section it
/// touches.
pub fn place_underground_section(section: &mut Section, seed: u32) {
    place_table_section(crate::data::ores::table(), section, seed);
}

/// [`place_underground_section`] over an explicit vein table.
fn place_table_section(table: &OreTable, section: &mut Section, seed: u32) {
    let (ccx, ccz) = (section.cx, section.cz);
    let clip = clip_box_of(section.world_box());
    let mut sink = SectionSink::new(section);
    let mut ctx = FeatureCtx::new(&mut sink);
    place_underground_into(table, &mut ctx, clip, ccx, ccz, seed);
}

/// World-Y span the scatter veins can possibly touch (the union of every
/// row's band widened by its vertical reach, clamped to the world), so the
/// cubic generator can skip the deep / high sections a vein can never reach.
pub fn y_span() -> (i32, i32) {
    crate::data::ores::table().y_span
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
    table: &OreTable,
    ctx: &mut FeatureCtx,
    clip: (IVec3, IVec3),
    ccx: i32,
    ccz: i32,
    seed: u32,
) {
    let (clip_min, clip_max) = clip;
    let max_r = table.max_reach;
    // 3x3 neighbourhood so border-straddling veins appear from both sides.
    for dcz in -1..=1 {
        for dcx in -1..=1 {
            let ncx = ccx + dcx;
            let ncz = ccz + dcz;
            // Column-level reject: no origin in this 16×16 column can reach the
            // clip box horizontally (origins span the column; reach ≤ the widest
            // vein's radius).
            if (ncx * 16 + 15 + max_r) < clip_min.x
                || (ncx * 16 - max_r) > clip_max.x
                || (ncz * 16 + 15 + max_r) < clip_min.z
                || (ncz * 16 - max_r) > clip_max.z
            {
                continue;
            }
            for cfg in table.veins {
                let (rxz, ry) = cfg.shape.reach();
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
                    if let Some(max_chance) = cfg.depth_ramp {
                        // Deeper = likelier: quadratic ease toward the band floor.
                        let t = (cfg.y_max - oy) as f32 / (cfg.y_max - cfg.y_min) as f32;
                        if !rng.chance(max_chance * t * t) {
                            continue;
                        }
                    }
                    let vein = Vein {
                        origin: IVec3::new(ox, oy, oz),
                        block: cfg.block,
                        hosts: cfg.hosts,
                    };
                    match cfg.shape {
                        VeinShape::Blob { size } => place_blob_vein(ctx, vein, size, &mut rng),
                        VeinShape::Grid3 { max_ore } => {
                            place_grid3_vein(ctx, vein, max_ore, &mut rng)
                        }
                    }
                }
            }
        }
    }
}

/// One rolled vein: where it sits, what it places, what it may overwrite.
#[derive(Copy, Clone)]
struct Vein {
    origin: IVec3,
    block: Block,
    hosts: &'static [Block],
}

/// Keep the world-floor layer solid stone and never write above the world top.
#[inline]
fn vein_y_in_world(y: i32) -> bool {
    y > WORLD_MIN_Y && y < WORLD_MAX_Y
}

/// A roughly-spherical blob of `~size` host cells turned into the vein's block,
/// with a small per-vein radius jitter so veins read irregular rather than as
/// clean spheres. Writes are host-only and chunk-clipped.
fn place_blob_vein(ctx: &mut FeatureCtx, vein: Vein, size: i32, rng: &mut FeatureRng) {
    let (ox, oy, oz) = (vein.origin.x, vein.origin.y, vein.origin.z);
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
                    ctx.replace_block(IVec3::new(ox + dx, y, oz + dz), vein.hosts, vein.block);
                }
            }
        }
    }
}

/// The iron/diamond vein shape: exactly `1..=max_ore` cells of the vein's block
/// chosen uniformly among the 3×3 slots of one horizontal layer centred on the
/// origin — a vein always fits a 3×3 area and never holds more than 9 ore
/// blocks. Writes are host-only and chunk-clipped (a slot occupied by cave air,
/// dirt, or an earlier vein simply stays as it is).
fn place_grid3_vein(ctx: &mut FeatureCtx, vein: Vein, max_ore: i32, rng: &mut FeatureRng) {
    let (ox, oy, oz) = (vein.origin.x, vein.origin.y, vein.origin.z);
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
                ctx.replace_block(IVec3::new(ox + dx, oy, oz + dz), vein.hosts, vein.block);
                remaining -= 1;
            }
            slots_left -= 1;
        }
    }
}

#[cfg(test)]
mod tests;
