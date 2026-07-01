use std::collections::HashMap;

use crate::chunk::{ChunkPos, SectionPos, SECTION_SIZE, SKY_FULL};
use crate::column::Column;

use super::{NBHD, NBHD_AREA};

/// How a section's skylight resolves, decided cheaply from the 3x3 column heightmaps.
pub(super) enum SkyPlan {
    /// Every cell sits above all surrounding cover: full daylight, no flood.
    Full,
    /// Every cell sits deeper than skylight can seep below the lowest cover: dark, no flood.
    Dark,
    /// The section straddles the surface band and needs a neighbourhood flood.
    Flood { surface: Box<[i32]> },
}

/// How far skylight reaches from an open-sky cell before it is fully dark.
const SKY_SEEP_REACH: i32 = (SKY_FULL / 2) as i32;

/// Heightmap stand-in for an unloaded neighbour column: fully covered, so it seeds
/// no phantom skylight into the centre.
const COVERED: i32 = i32::MAX;

/// Decide how `pos`'s skylight resolves from the 3x3 column heightmaps alone.
pub(super) fn plan(pos: SectionPos, columns: &HashMap<ChunkPos, Column>) -> SkyPlan {
    let (hmin, hmax) = cover_range(pos, columns);
    let oy = pos.origin_world().1;
    let top = oy + SECTION_SIZE as i32 - 1;
    if oy > hmax {
        SkyPlan::Full
    } else if top < hmin + 1 - SKY_SEEP_REACH {
        SkyPlan::Dark
    } else {
        SkyPlan::Flood {
            surface: gather_surface(pos, columns),
        }
    }
}

fn cover_range(pos: SectionPos, columns: &HashMap<ChunkPos, Column>) -> (i32, i32) {
    let (mut hmin, mut hmax) = (i32::MAX, i32::MIN);
    for dcz in -1..=1 {
        for dcx in -1..=1 {
            let cp = ChunkPos::new(pos.cx + dcx, pos.cz + dcz);
            if let Some(col) = columns.get(&cp) {
                for &h in col.heightmap_slice() {
                    hmin = hmin.min(h);
                    hmax = hmax.max(h);
                }
            }
        }
    }
    if hmin == i32::MAX {
        (crate::column::NO_SURFACE, crate::column::NO_SURFACE)
    } else {
        (hmin, hmax)
    }
}

fn gather_surface(pos: SectionPos, columns: &HashMap<ChunkPos, Column>) -> Box<[i32]> {
    let mut surface = vec![COVERED; NBHD_AREA].into_boxed_slice();
    for dcz in -1..=1 {
        for dcx in -1..=1 {
            let cp = ChunkPos::new(pos.cx + dcx, pos.cz + dcz);
            let Some(col) = columns.get(&cp) else {
                continue;
            };
            let hm = col.heightmap_slice();
            let bx = ((dcx + 1) as usize) * SECTION_SIZE;
            let bz = ((dcz + 1) as usize) * SECTION_SIZE;
            for lz in 0..SECTION_SIZE {
                for lx in 0..SECTION_SIZE {
                    surface[(bz + lz) * NBHD + (bx + lx)] = hm[lz * SECTION_SIZE + lx];
                }
            }
        }
    }
    surface
}
