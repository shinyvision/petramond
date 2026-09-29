use petramond_world::block::Block;
use petramond_world::chunk::{WORLD_MAX_Y, WORLD_MIN_Y};
use petramond_world::mathh::IVec3;
use petramond_world::section::Section;

use super::super::rng::FeatureRng;
use super::sink::SinkTarget;
use super::{FeatureCtx, SectionSink};
use std::sync::Arc;

use crate::cache::GenCaches;
use crate::data::ores::{blob_base_radius, OreTable, VeinShape};

/// Places the underground veins reaching one 16³ [`Section`] through a [`SectionSink`] - each
/// section regenerates the veins of its whole 3×3 column neighbourhood.
/// Veins key off their ORIGIN column (`positional(seed, salt, ncx, vein, ncz)`) and only overwrite
/// their hosts, so a vein straddling a section seam (horizontal or vertical) comes out the same
/// from every section it touches.
pub fn place_underground_section(section: &mut Section, seed: u32, caches: &GenCaches) {
    place_table_section(crate::data::ores::table(), section, seed, caches);
}

fn place_table_section(table: &OreTable, section: &mut Section, seed: u32, caches: &GenCaches) {
    let (ccx, ccz) = (section.cx, section.cz);
    let clip = clip_box_of(section.world_box());
    let mut sink = SectionSink::new(section);
    let mut ctx = FeatureCtx::new(&mut sink);
    place_underground_into(table, &mut ctx, clip, ccx, ccz, seed, caches);
}

pub fn y_span() -> (i32, i32) {
    crate::data::ores::table().y_span
}

fn clip_box_of((origin, size): (IVec3, IVec3)) -> (IVec3, IVec3) {
    (origin, origin + size - IVec3::splat(1))
}

/// A vein whose origin (and depth-ramp roll) is decided, with the stream its
/// shape continues from. A column's veins are rolled once and reused by every
/// section of the nine columns they can reach.
#[derive(Clone, Copy)]
struct RolledVein {
    origin: IVec3,
    rng: FeatureRng,
}

/// A column's rolled veins per table row, each row's sorted by origin height:
/// veins of one row write the same block under the same host rule, so their
/// order among themselves never shows, while rows keep table order.
pub(crate) struct ColumnVeins {
    veins: Box<[RolledVein]>,
    rows: Box<[u32]>,
}

impl ColumnVeins {
    pub(crate) fn heap_bytes(&self) -> usize {
        std::mem::size_of_val(&*self.veins) + std::mem::size_of_val(&*self.rows)
    }

    fn row(&self, row: usize) -> &[RolledVein] {
        let start = if row == 0 { 0 } else { self.rows[row - 1] };
        &self.veins[start as usize..self.rows[row] as usize]
    }
}

/// Which table (by its leaked rows' address), seed and origin column.
pub(crate) type VeinKey = (u32, usize, [i32; 2]);

fn column_veins(table: &OreTable, seed: u32, ncx: i32, ncz: i32) -> ColumnVeins {
    let mut out = Vec::new();
    let mut rows = Vec::with_capacity(table.veins.len());
    for cfg in table.veins {
        let start = out.len();
        for i in 0..cfg.count {
            let mut rng = FeatureRng::positional(seed, cfg.salt, ncx, i, ncz);
            let ox = ncx * 16 + rng.next_i32(0, 15);
            let oz = ncz * 16 + rng.next_i32(0, 15);
            let oy = rng.next_i32(cfg.y_min, cfg.y_max);
            if let Some(max_chance) = cfg.depth_ramp {
                let t = (cfg.y_max - oy) as f32 / (cfg.y_max - cfg.y_min) as f32;
                if !rng.chance(max_chance * t * t) {
                    continue;
                }
            }
            out.push(RolledVein {
                origin: IVec3::new(ox, oy, oz),
                rng,
            });
        }
        out[start..].sort_by_key(|v| v.origin.y);
        rows.push(out.len() as u32);
    }
    ColumnVeins {
        veins: out.into(),
        rows: rows.into(),
    }
}

fn place_underground_into(
    table: &OreTable,
    ctx: &mut FeatureCtx,
    clip: (IVec3, IVec3),
    ccx: i32,
    ccz: i32,
    seed: u32,
    caches: &GenCaches,
) {
    let (clip_min, clip_max) = clip;
    let max_r = table.max_reach;
    for dcz in -1..=1 {
        for dcx in -1..=1 {
            let ncx = ccx + dcx;
            let ncz = ccz + dcz;
            if (ncx * 16 + 15 + max_r) < clip_min.x
                || (ncx * 16 - max_r) > clip_max.x
                || (ncz * 16 + 15 + max_r) < clip_min.z
                || (ncz * 16 - max_r) > clip_max.z
            {
                continue;
            }
            let key = (seed, table.veins.as_ptr() as usize, [ncx, ncz]);
            let veins = caches
                .terrain
                .ore_veins
                .get_or_compute_unlocked(key, || Arc::new(column_veins(table, seed, ncx, ncz)));
            for (row, cfg) in table.veins.iter().enumerate() {
                let (rxz, ry) = cfg.shape.reach();
                if cfg.y_max + ry < clip_min.y || cfg.y_min - ry > clip_max.y {
                    continue;
                }
                let row = veins.row(row);
                let first = row.partition_point(|v| v.origin.y + ry < clip_min.y);
                for rolled in &row[first..] {
                    let IVec3 {
                        x: ox,
                        y: oy,
                        z: oz,
                    } = rolled.origin;
                    if oy - ry > clip_max.y {
                        break;
                    }
                    if ox + rxz < clip_min.x
                        || ox - rxz > clip_max.x
                        || oz + rxz < clip_min.z
                        || oz - rxz > clip_max.z
                    {
                        continue;
                    }
                    let vein = Vein {
                        origin: rolled.origin,
                        block: cfg.block,
                        hosts: cfg.hosts,
                    };
                    let mut rng = rolled.rng;
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

#[derive(Copy, Clone)]
struct Vein {
    origin: IVec3,
    block: Block,
    hosts: &'static [Block],
}

#[inline]
fn vein_y_in_world(y: i32) -> bool {
    y > WORLD_MIN_Y && y < WORLD_MAX_Y
}

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

fn place_grid3_vein(ctx: &mut FeatureCtx, vein: Vein, max_ore: i32, rng: &mut FeatureRng) {
    let (ox, oy, oz) = (vein.origin.x, vein.origin.y, vein.origin.z);
    if !vein_y_in_world(oy) {
        return;
    }
    let mut remaining = rng.next_i32(1, max_ore);
    let mut slots_left = 9;
    for dz in -1..=1 {
        for dx in -1..=1 {
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
