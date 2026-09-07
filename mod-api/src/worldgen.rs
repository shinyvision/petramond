//! Worldgen feature admission: what a feature declares at registration so
//! the host can skip the sections it provably cannot touch.

use serde::{Deserialize, Serialize};

/// Conservative WRITE bounds of a worldgen feature, declared with
/// [`HostCall::RegisterWorldgenFeature`] and checked by the host per section
/// BEFORE it snapshots blocks and encodes the guest call — a section the
/// feature cannot write into costs the mod no dispatch at all.
///
/// The bounds must cover every cell the feature may write, including
/// cross-section reach (a vein that pokes one block past its band declares
/// the band one block wider). Every predicate composes by AND; the
/// [`ANY`](Self::ANY) filter admits every section.
///
/// [`HostCall::RegisterWorldgenFeature`]: crate::HostCall::RegisterWorldgenFeature
#[derive(Serialize, Deserialize, Copy, Clone, Debug, PartialEq, Eq)]
pub struct GenFeatureFilter {
    /// Lowest world Y the feature writes (inclusive).
    pub min_y: i32,
    /// Highest world Y the feature writes (inclusive).
    pub max_y: i32,
    /// Inclusive `[below, above]` offsets from a column's surface height
    /// within which the feature writes — a section is admitted when ANY of
    /// its columns' surfaces put that band inside it. `None` = not anchored
    /// to the surface.
    pub surface_offsets: Option<[i32; 2]>,
    /// Whether the guest call should carry the 4096-cell block snapshot. A
    /// purely positional feature (absolute-Y blobs) can decline it and save
    /// the copy; its `GenCtx::block` then reads `None` everywhere.
    pub needs_blocks: bool,
}

impl Default for GenFeatureFilter {
    fn default() -> Self {
        Self::ANY
    }
}

impl GenFeatureFilter {
    /// Admits every section and carries the block snapshot.
    pub const ANY: GenFeatureFilter = GenFeatureFilter {
        min_y: i32::MIN,
        max_y: i32::MAX,
        surface_offsets: None,
        needs_blocks: true,
    };

    /// Writes only inside the inclusive absolute band `min_y..=max_y`.
    pub const fn y_band(min_y: i32, max_y: i32) -> GenFeatureFilter {
        GenFeatureFilter {
            min_y,
            max_y,
            ..Self::ANY
        }
    }

    /// Writes only inside `surface + below ..= surface + above` of some
    /// column of the section (offsets inclusive; `below` is usually ≤ 0).
    pub const fn surface_band(below: i32, above: i32) -> GenFeatureFilter {
        GenFeatureFilter {
            surface_offsets: Some([below, above]),
            ..Self::ANY
        }
    }

    /// The same bounds, declining the block snapshot.
    pub const fn without_blocks(self) -> GenFeatureFilter {
        GenFeatureFilter {
            needs_blocks: false,
            ..self
        }
    }

    /// Whether both bands are ordered (`min <= max`); the host rejects the
    /// registration otherwise.
    pub fn is_valid(self) -> bool {
        self.min_y <= self.max_y && self.surface_offsets.is_none_or(|[lo, hi]| lo <= hi)
    }

    /// Whether the section at vertical index `cy` (world Y `cy*16 ..= cy*16+15`)
    /// may receive writes, given the surface heights of its columns. Evaluated
    /// in `i64` so the open bounds never overflow.
    pub fn intersects(self, cy: i32, surfaces: &[i32]) -> bool {
        let lo = i64::from(cy) * 16;
        let hi = lo + 15;
        if lo > i64::from(self.max_y) || hi < i64::from(self.min_y) {
            return false;
        }
        self.surface_offsets.is_none_or(|[below, above]| {
            surfaces.iter().any(|&y| {
                lo <= i64::from(y) + i64::from(above) && hi >= i64::from(y) + i64::from(below)
            })
        })
    }
}

#[cfg(test)]
mod tests;
