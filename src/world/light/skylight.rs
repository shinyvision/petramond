use rustc_hash::FxHashMap;

use crate::chunk::{ChunkPos, SectionPos, SECTION_SIZE, SKY_FULL};
use crate::column::Column;

use super::{NBHD, NBHD_AREA};

/// How a section's skylight resolves, decided cheaply from the 3x3 column
/// direct-sky cover maps.
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

/// Whether changing cover between these two vertical endpoints can alter this
/// section's skylight. Above the higher endpoint both states are direct sky;
/// below the lower endpoint's seep reach both are dark.
pub(in crate::world) fn cover_change_affects_section(
    pos: SectionPos,
    min_cover: i32,
    max_cover: i32,
) -> bool {
    let affected_min = min_cover.saturating_add(1).saturating_sub(SKY_SEEP_REACH);
    let oy = pos.origin_world().1;
    let top = oy + SECTION_SIZE as i32 - 1;
    top >= affected_min && oy <= max_cover
}

/// Decide how `pos`'s skylight resolves from the 3x3 column sky-cover maps alone.
pub(super) fn plan(pos: SectionPos, columns: &FxHashMap<ChunkPos, Column>) -> SkyPlan {
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

fn cover_range(pos: SectionPos, columns: &FxHashMap<ChunkPos, Column>) -> (i32, i32) {
    let (mut hmin, mut hmax) = (i32::MAX, i32::MIN);
    for dcz in -1..=1 {
        for dcx in -1..=1 {
            let cp = ChunkPos::new(pos.cx + dcx, pos.cz + dcz);
            if let Some(col) = columns.get(&cp) {
                for &h in col.sky_cover_slice() {
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

fn gather_surface(pos: SectionPos, columns: &FxHashMap<ChunkPos, Column>) -> Box<[i32]> {
    let mut surface = vec![COVERED; NBHD_AREA].into_boxed_slice();
    for dcz in -1..=1 {
        for dcx in -1..=1 {
            let cp = ChunkPos::new(pos.cx + dcx, pos.cz + dcz);
            let Some(col) = columns.get(&cp) else {
                continue;
            };
            let hm = col.sky_cover_slice();
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glass_roof_keeps_the_lower_section_out_of_the_dark_shortcut() {
        let mut columns = FxHashMap::default();
        for cz in -1..=1 {
            for cx in -1..=1 {
                let mut column = Column::new();
                for z in 0..SECTION_SIZE {
                    for x in 0..SECTION_SIZE {
                        column.set_surface_y(x, z, 64);
                        column.set_sky_cover_y(x, z, 64);
                    }
                }
                columns.insert(ChunkPos::new(cx, cz), column);
            }
        }

        // A glass block at y=64 remains the visible top, but the solid cover in
        // this one-cell shaft is down at y=0. Section cy=2 sits far enough below
        // the surrounding terrain that the visible surface map used to classify
        // the entire section as dark at its y=48 boundary.
        columns
            .get_mut(&ChunkPos::new(0, 0))
            .unwrap()
            .set_sky_cover_y(8, 8, 0);

        assert!(
            matches!(plan(SectionPos::new(0, 2, 0), &columns), SkyPlan::Flood { .. }),
            "a clear roof over a shaft must flood the lower section instead of short-circuiting it dark"
        );
        assert!(cover_change_affects_section(
            SectionPos::new(0, 2, 0),
            0,
            64
        ));
        assert!(!cover_change_affects_section(
            SectionPos::new(0, -2, 0),
            0,
            64
        ));
    }
}
