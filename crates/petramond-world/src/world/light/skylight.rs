use rustc_hash::FxHashMap;

use crate::chunk::{ChunkPos, SectionPos, SECTION_SIZE, SKY_FULL};
use crate::column::Column;

use super::NBHD_AREA;

pub enum SkyPlan {
    Full,
    Dark,
    Flood { surface: Box<[i32]> },
}

const SKY_SEEP_REACH: i32 = (SKY_FULL / 2) as i32;

const COVERED: i32 = i32::MAX;

pub fn cover_change_affects_section(pos: SectionPos, min_cover: i32, max_cover: i32) -> bool {
    let affected_min = min_cover.saturating_add(1).saturating_sub(SKY_SEEP_REACH);
    let oy = pos.origin_world().1;
    let top = oy + SECTION_SIZE as i32 - 1;
    top >= affected_min && oy <= max_cover
}

#[derive(Copy, Clone, PartialEq, Eq)]
pub enum SkyClass {
    Full,
    Dark,
    Flood,
}

pub fn classify(
    pos: SectionPos,
    columns: &FxHashMap<ChunkPos, std::sync::Arc<Column>>,
) -> SkyClass {
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

pub fn plan(pos: SectionPos, columns: &FxHashMap<ChunkPos, std::sync::Arc<Column>>) -> SkyPlan {
    match classify(pos, columns) {
        SkyClass::Full => SkyPlan::Full,
        SkyClass::Dark => SkyPlan::Dark,
        SkyClass::Flood => SkyPlan::Flood {
            surface: gather_surface(pos, columns),
        },
    }
}

fn cover_range(
    pos: SectionPos,
    columns: &FxHashMap<ChunkPos, std::sync::Arc<Column>>,
) -> (i32, i32) {
    let (mut hmin, mut hmax) = (i32::MAX, i32::MIN);
    for dcz in -1..=1 {
        for dcx in -1..=1 {
            let cp = ChunkPos::new(pos.cx + dcx, pos.cz + dcz);
            if let Some(col) = columns.get(&cp) {
                let (lo, hi) = col.sky_cover_range();
                hmin = hmin.min(lo);
                hmax = hmax.max(hi);
            }
        }
    }
    if hmin == i32::MAX {
        (crate::column::NO_SURFACE, crate::column::NO_SURFACE)
    } else {
        (hmin, hmax)
    }
}

fn gather_surface(
    pos: SectionPos,
    columns: &FxHashMap<ChunkPos, std::sync::Arc<Column>>,
) -> Box<[i32]> {
    let surface = gather_surface_span(ChunkPos::new(pos.cx - 1, pos.cz - 1), 3, columns);
    debug_assert_eq!(surface.len(), NBHD_AREA);
    surface
}

pub fn gather_surface_span(
    base: ChunkPos,
    span: usize,
    columns: &FxHashMap<ChunkPos, std::sync::Arc<Column>>,
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
                let dst = (bz + lz) * dim + bx;
                let src = lz * SECTION_SIZE;
                surface[dst..dst + SECTION_SIZE].copy_from_slice(&hm[src..src + SECTION_SIZE]);
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
                columns.insert(ChunkPos::new(cx, cz), std::sync::Arc::new(column));
            }
        }

        std::sync::Arc::make_mut(columns.get_mut(&ChunkPos::new(0, 0)).unwrap())
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
