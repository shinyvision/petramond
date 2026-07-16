//! Streaming load-target policy: the streaming radii, the per-connection
//! anchor, and the priority keys that order column/section generation and
//! terrain sends (including the surface-first bias).

use crate::chunk::{ChunkPos, SectionPos};

pub const RENDER_DIST: i32 = 32;

/// One streaming anchor for [`World::update_load_multi`]: a player's section
/// coordinates plus that connection's streaming radius (its requested view
/// distance, already clamped by the server's own maximum), one per connected
/// player.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct LoadAnchor {
    pub cx: i32,
    pub cy: i32,
    pub cz: i32,
    /// Horizontal streaming radius in chunks for this anchor's connection.
    pub radius: i32,
}

/// Vertical load radius (in 16³ sections) around the player's section: the world
/// streams a flattened cylinder — a Euclidean horizontal disc of columns × this many
/// sections above and below the player. Sized so the visible surface band is fully
/// loaded when standing on typical terrain, while the deep underground / high sky a
/// far column doesn't need is left ungenerated until the player approaches it (the
/// per-section "generate closest to the player" payoff that makes room for caves).
pub const VERTICAL_LOAD_RADIUS: i32 = 5;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) struct LoadTarget {
    pub center: ChunkPos,
    /// Player's section `cy` — the centre of the vertical load window.
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

    pub(super) fn column_priority_key(self, pos: ChunkPos) -> i64 {
        let dx = pos.cx - self.center.cx;
        let dz = pos.cz - self.center.cz;
        (dx as i64 * dx as i64) + (dz as i64 * dz as i64)
    }

    pub(super) fn section_priority_key(self, pos: SectionPos) -> i64 {
        let dx = pos.cx - self.center.cx;
        let dy = pos.cy - self.center_cy;
        let dz = pos.cz - self.center.cz;
        (dx as i64 * dx as i64) + (dy as i64 * dy as i64) + (dz as i64 * dz as i64)
    }

    /// [`section_priority_key`](Self::section_priority_key) with a surface-first
    /// bias: while the anchor itself is above ground, a section wholly below its
    /// own column's surface band (`pos.cy < band_lo` — the same test deep
    /// classification uses) is scheduled as if it were `render_dist / 2` sections
    /// farther away. The player can only see such sections through cave openings,
    /// so the visible surface shell streams, lights, and ships first; nearby cave
    /// interiors still beat far surface rather than starving. An underground
    /// anchor (a caving player) keeps the pure 3D nearest-first order — the deep
    /// sections around them ARE the visible world.
    pub(super) fn surface_biased_section_key(
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
}
