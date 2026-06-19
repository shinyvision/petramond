//! Per-chunk lazy memoization of the per-column noise fields.
//!
//! `surface_height`, `river_strength`, and `climate` are pure functions of world
//! `(x, z)` over the immutable, seed-derived noise samplers. During one chunk's
//! generation they are sampled at the SAME `(wx, wz)` many times across stages:
//!   - `biome_assign` samples the 16×16 interior,
//!   - `smoothed_plan` resamples a 5×5 (`±SMOOTH_R`) stencil per river column —
//!     once in `fill_columns` and AGAIN in `place_features`,
//!   - `place_features` samples the whole `16 + 2*MARGIN` origin window, nearby
//!     tree-spacing candidates, plus each candidate's river stencil.
//! That cross-stage duplication dominated worldgen. This cache computes each
//! distinct column at most once and hands the SAME bits back on every later read.
//!
//! Identity (and therefore byte-parity) is exact: the stored value is the first
//! `field.X(wx, wz)` result — the very call the old code made — so substituting
//! the cache for a direct call cannot change any block or biome byte. The cache
//! is built fresh per chunk (its base `(bx, bz)` is derived from `(cx, cz)`); it
//! is NEVER reused across chunks (that would alias every read) and holds no
//! interior mutability beyond its own `&mut self` accessors.

use crate::biome::Climate;
use crate::chunk::{CHUNK_SX, CHUNK_SZ};

use super::carve::SMOOTH_R;
use super::feature::TREE_SPACING_RADIUS;
use super::noise::HeightField;
use super::proto::MARGIN;

/// Half-width padding around the 16-column chunk that generation + feature
/// placement can reach: the feature origin border (`MARGIN`), the tree-spacing
/// neighbour search, and the river smoothing stencil radius (`SMOOTH_R`) applied
/// at the outermost candidate columns. Derived from the source constants so a
/// future bump keeps the window correct (an undersized window would trip the
/// `index` assert, not silently alias).
pub const PAD: i32 = MARGIN + TREE_SPACING_RADIUS + SMOOTH_R;
/// Cache window edge length, in columns.
pub const W: i32 = 16 + 2 * PAD;
const WN: usize = (W * W) as usize;

const ZERO_CLIMATE: Climate = Climate {
    temperature: 0.0,
    humidity: 0.0,
    continentalness: 0.0,
    erosion: 0.0,
    weirdness: 0.0,
    depth: 0.0,
};

pub struct FieldCache<'a> {
    field: &'a HeightField,
    /// World coords of local index 0 (= chunk origin − PAD).
    bx: i32,
    bz: i32,
    surf: Vec<i32>,
    river: Vec<f32>,
    climate: Vec<Climate>,
    hs: Vec<bool>,
    hr: Vec<bool>,
    hc: Vec<bool>,
}

impl<'a> FieldCache<'a> {
    pub fn new(field: &'a HeightField, cx: i32, cz: i32) -> Self {
        Self {
            field,
            bx: cx * CHUNK_SX as i32 - PAD,
            bz: cz * CHUNK_SZ as i32 - PAD,
            surf: vec![0; WN],
            river: vec![0.0; WN],
            climate: vec![ZERO_CLIMATE; WN],
            hs: vec![false; WN],
            hr: vec![false; WN],
            hc: vec![false; WN],
        }
    }

    /// World → flat local index. HARD assert (survives `--release`, which is how
    /// the genparity gate runs): an out-of-window coord must panic, never alias a
    /// valid-but-wrong cell and silently corrupt terrain.
    #[inline]
    fn index(&self, wx: i32, wz: i32) -> usize {
        let lx = wx - self.bx;
        let lz = wz - self.bz;
        assert!(
            lx >= 0 && lx < W && lz >= 0 && lz < W,
            "FieldCache OOB: ({wx},{wz}) outside window base ({},{}) size {W}",
            self.bx,
            self.bz
        );
        (lz * W + lx) as usize
    }

    #[inline]
    pub fn surf(&mut self, wx: i32, wz: i32) -> i32 {
        let i = self.index(wx, wz);
        if !self.hs[i] {
            self.surf[i] = self.field.surface_height(wx, wz);
            self.hs[i] = true;
        }
        let v = self.surf[i];
        // Bring-up / regression oracle (debug builds only, zero release cost): a
        // hit MUST return the exact value a direct sample would — proving no
        // window/index aliasing for whatever terrain the caller generates,
        // beyond the few chunks the genparity gate covers.
        debug_assert_eq!(
            v,
            self.field.surface_height(wx, wz),
            "FieldCache.surf aliased at ({wx},{wz})"
        );
        v
    }

    #[inline]
    pub fn river(&mut self, wx: i32, wz: i32) -> f32 {
        let i = self.index(wx, wz);
        if !self.hr[i] {
            self.river[i] = self.field.river_strength(wx, wz);
            self.hr[i] = true;
        }
        let v = self.river[i];
        debug_assert_eq!(
            v,
            self.field.river_strength(wx, wz),
            "FieldCache.river aliased at ({wx},{wz})"
        );
        v
    }

    #[inline]
    pub fn climate(&mut self, wx: i32, wz: i32) -> Climate {
        let i = self.index(wx, wz);
        if !self.hc[i] {
            self.climate[i] = self.field.climate(wx, wz);
            self.hc[i] = true;
        }
        let v = self.climate[i];
        debug_assert_eq!(
            v,
            self.field.climate(wx, wz),
            "FieldCache.climate aliased at ({wx},{wz})"
        );
        v
    }
}

#[cfg(test)]
mod tests {
    use crate::worldgen::generate_chunk;

    /// Generate a broad, terrain-diverse grid across several seeds in a DEBUG
    /// build so the per-access `debug_assert_eq!` oracle in every cache accessor
    /// fires if any (wx,wz) ever aliases or the window is undersized. Covers
    /// river-smoothing stencils, mountain bands, and the full feature margin —
    /// terrain the 27-chunk genparity gate may not reach. (No-op in release.)
    #[test]
    fn field_cache_matches_direct_samples_over_diverse_grid() {
        for &seed in &[0x1234_5678u32, 1, 0xDEAD_BEEF, 7] {
            for cz in -3..=3 {
                for cx in -3..=3 {
                    // Generation drives every cache accessor; the debug oracle
                    // panics on any mismatch. A clean run proves cache fidelity.
                    let _ = generate_chunk(seed, cx, cz);
                }
            }
        }
    }
}
