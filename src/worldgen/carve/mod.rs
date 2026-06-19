//! Carvers — terrain subtraction after the solid fill.
//!
//! Strata P2: only the river carver exists, and it contributes *parameters*
//! (`CarvePlan`) that the driver's cascade consumes inline, preserving the god
//! file's exact carve branch. The `Carver` trait + `CarverSet` set up the fixed
//! carver ordering that P4 will run as true post-fill passes.

pub mod river;

use crate::chunk::SEA_LEVEL;
use river::RiverCarver;
use super::field_cache::FieldCache;

#[derive(Copy, Clone)]
pub struct CarvePlan {
    pub carve: bool,
    /// Target surface Y after carving: solid above this is cut away, the column
    /// floods to sea level, and the floor voxel becomes the riverbed.
    pub river_floor: i32,
}

pub trait Carver: Send + Sync {
    fn plan(&self, river: f32, surf: i32) -> CarvePlan;
}

pub struct CarverSet {
    river: RiverCarver,
}

impl Default for CarverSet {
    fn default() -> Self {
        Self { river: RiverCarver }
    }
}

/// Half-width (blocks) of the river-edge smoothing kernel. A 2-block radius
/// (5×5 mean) is enough to ramp the stair-step banks and dissolve the small
/// (≲4-wide) mid-channel sand bars / peninsulas the per-column carve leaves
/// behind, without over-blurring the channel itself.
pub const SMOOTH_R: i32 = 2;

impl CarverSet {
    /// Combined carve plan for a column. P2 has a single carver.
    #[inline]
    pub fn plan(&self, river: f32, surf: i32) -> CarvePlan {
        self.river.plan(river, surf)
    }

    /// River-edge smoothing pass. Blurs the raw river floor over a small
    /// neighbourhood so jagged stair-step banks become gentle ramps and small
    /// mid-channel islands / peninsulas dissolve into the water.
    ///
    /// Runs as part of the carve plan, so it is applied to the terrain fill AND
    /// (because feature placement anchors to the same plan) BEFORE trees are placed
    /// — nothing grows on an island the smoothing has removed. It is a pure function
    /// of world position (neighbours are resampled from the height field, not the
    /// chunk), so the result is identical and seamless across chunk borders.
    ///
    /// Carve-only: a column is never raised above its natural surface. Water columns
    /// are never un-flooded (kept at least as deep as the raw carve); land/bank
    /// columns are only ever cut DOWN toward the water, so banks ramp and small
    /// islands surrounded by water sink to water level.
    ///
    /// `river`/`surf` are the already-computed values for the centre column; only
    /// columns the raw carver actually touches pay for the neighbourhood resample.
    pub fn smoothed_plan(
        &self,
        cache: &mut FieldCache,
        wx: i32,
        wz: i32,
        river: f32,
        surf: i32,
    ) -> CarvePlan {
        let raw = self.plan(river, surf);
        // Off-river / mountain columns pass straight through (raw.carve already
        // gates river>0.05 AND surf<=sea+RIVER_MAX_LAND).
        if !raw.carve {
            return raw;
        }
        let mut sum = 0i32;
        let mut cnt = 0i32;
        // Track whether a WATER neighbour exists on each side, to spot a land column
        // that is enclosed by water (a mid-channel bar / peninsula tip).
        let (mut w_px, mut w_nx, mut w_pz, mut w_nz) = (false, false, false, false);
        for dz in -SMOOTH_R..=SMOOTH_R {
            for dx in -SMOOTH_R..=SMOOTH_R {
                let s = cache.surf(wx + dx, wz + dz);
                let rv = cache.river(wx + dx, wz + dz);
                // A neighbour's contribution is its own carved floor (its natural
                // surface where it isn't a river column), so the blur ramps from the
                // sub-sea bed up to the surrounding terrain across the bank.
                let nf = self.plan(rv, s).river_floor;
                sum += nf;
                cnt += 1;
                // Opposite-side water is tested COLLINEARLY through the centre (the
                // ±x water must share the centre row, ±z the centre column), so the
                // enclosure fires on a true bar flanked by water — not on a straight
                // land neck that merely has water diagonally on either side.
                if nf < SEA_LEVEL {
                    if dz == 0 && dx > 0 { w_px = true; }
                    if dz == 0 && dx < 0 { w_nx = true; }
                    if dx == 0 && dz > 0 { w_pz = true; }
                    if dx == 0 && dz < 0 { w_nz = true; }
                }
            }
        }
        let blurred = (sum as f32 / cnt as f32).round() as i32;
        let floor = if raw.river_floor < SEA_LEVEL {
            // Water: smooth but never un-flood (keep at least the raw depth).
            blurred.min(raw.river_floor)
        } else if (w_px && w_nx) || (w_pz && w_nz) {
            // Land enclosed by water on opposite sides = a bar/island the per-column
            // carve left standing. Flood it (carve below sea), at roughly the
            // surrounding water depth, so it dissolves into the channel.
            blurred.min(SEA_LEVEL - 1).min(surf)
        } else {
            // Plain bank: only ever carve down toward the water (ramp the step).
            blurred.min(surf)
        };
        CarvePlan {
            carve: floor < surf,
            river_floor: floor,
        }
    }
}
