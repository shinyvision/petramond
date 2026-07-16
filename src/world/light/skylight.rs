use rustc_hash::FxHashMap;

use crate::chunk::{ChunkPos, SectionPos, SECTION_SIZE, SKY_FULL};
use crate::column::Column;

use super::NBHD_AREA;

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

/// [`SkyPlan`] without the flood's gathered surface payload — how a section's
/// skylight resolves, classified from the 3x3 column sky-cover maps alone.
#[derive(Copy, Clone, PartialEq, Eq)]
pub(super) enum SkyClass {
    Full,
    Dark,
    Flood,
}

/// Decide how `pos`'s skylight resolves from the 3x3 column sky-cover maps alone.
pub(super) fn classify(pos: SectionPos, columns: &FxHashMap<ChunkPos, Column>) -> SkyClass {
    let (hmin, hmax) = cover_range(pos, columns);
    let oy = pos.origin_world().1;
    let top = oy + SECTION_SIZE as i32 - 1;
    if oy > hmax {
        SkyClass::Full
    } else if top < hmin + 1 - SKY_SEEP_REACH {
        SkyClass::Dark
    } else {
        SkyClass::Flood
    }
}

pub(super) fn plan(pos: SectionPos, columns: &FxHashMap<ChunkPos, Column>) -> SkyPlan {
    match classify(pos, columns) {
        SkyClass::Full => SkyPlan::Full,
        SkyClass::Dark => SkyPlan::Dark,
        SkyClass::Flood => SkyPlan::Flood {
            surface: gather_surface(pos, columns),
        },
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
    let surface = gather_surface_span(ChunkPos::new(pos.cx - 1, pos.cz - 1), 3, columns);
    debug_assert_eq!(surface.len(), NBHD_AREA);
    surface
}

/// Gather a `span`×`span` column window of sky cover, `base` at the low corner,
/// into a `(span*16)`² map. Absent columns read as fully covered so they seed no
/// phantom skylight.
pub(super) fn gather_surface_span(
    base: ChunkPos,
    span: usize,
    columns: &FxHashMap<ChunkPos, Column>,
) -> Box<[i32]> {
    let dim = span * SECTION_SIZE;
    let mut surface = vec![COVERED; dim * dim].into_boxed_slice();
    for dcz in 0..span {
        for dcx in 0..span {
            let cp = ChunkPos::new(base.cx + dcx as i32, base.cz + dcz as i32);
            let Some(col) = columns.get(&cp) else {
                continue;
            };
            let hm = col.sky_cover_slice();
            let bx = dcx * SECTION_SIZE;
            let bz = dcz * SECTION_SIZE;
            for lz in 0..SECTION_SIZE {
                for lx in 0..SECTION_SIZE {
                    surface[(bz + lz) * dim + (bx + lx)] = hm[lz * SECTION_SIZE + lx];
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
