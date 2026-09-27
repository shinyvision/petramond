use crate::chunk::{ChunkPos, SectionPos};

pub const RENDER_DIST: i32 = 32;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct LoadAnchor {
    pub cx: i32,
    pub cy: i32,
    pub cz: i32,
    pub radius: i32,
}

/// Sections up/down around the player's section, times a horizontal disc of columns.
/// Big enough for the surface band on normal terrain. Far deep/sky sections wait till you're close.
pub const VERTICAL_LOAD_RADIUS: i32 = 5;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct LoadTarget {
    pub center: ChunkPos,
    pub center_cy: i32,
    pub render_dist: i32,
}

impl LoadTarget {
    pub fn new(cx: i32, cy: i32, cz: i32, render_dist: i32) -> Self {
        Self {
            center: ChunkPos::new(cx, cz),
            center_cy: cy,
            render_dist,
        }
    }

    pub fn column_priority_key(self, pos: ChunkPos) -> i64 {
        let dx = pos.cx - self.center.cx;
        let dz = pos.cz - self.center.cz;
        (dx as i64 * dx as i64) + (dz as i64 * dz as i64)
    }

    pub fn section_priority_key(self, pos: SectionPos) -> i64 {
        let dx = pos.cx - self.center.cx;
        let dy = pos.cy - self.center_cy;
        let dz = pos.cz - self.center.cz;
        (dx as i64 * dx as i64) + (dy as i64 * dy as i64) + (dz as i64 * dz as i64)
    }

    pub fn surface_biased_section_key(
        self,
        pos: SectionPos,
        band_lo: i32,
        anchor_underground: bool,
    ) -> i64 {
        let key = self.section_priority_key(pos);
        if anchor_underground || pos.cy >= band_lo {
            return key;
        }
        let h = i64::from((self.render_dist / 2).max(8));
        key + h * h
    }

    pub fn deferred_section_key(
        self,
        pos: SectionPos,
        band_lo: i32,
        anchor_underground: bool,
    ) -> i64 {
        let h = i64::from((self.render_dist / 2).max(8));
        self.surface_biased_section_key(pos, band_lo, anchor_underground) + h * h
    }
}
