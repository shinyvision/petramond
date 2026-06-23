//! River source gating and spacing.
//!
//! The cheap per-cell gate (`source_gate`): an fbm score short-circuit, then a
//! non-ocean + elevation-band check, returning a jittered source point. Plus the
//! deterministic suppress-weaker-of-two spacing (`source_suppressed`) so exactly
//! one of any too-close pair survives. Both are pure functions of `(seed, cell)`.

use noise::NoiseFn;

use super::*;

/// A source that passed the cheap gate: jittered source point + its score.
#[derive(Copy, Clone)]
pub(super) struct SourceGate {
    pub(super) x: f32,
    pub(super) z: f32,
    pub(super) score: f32,
}

impl RiverSystem {
    /// Cheap source gate (decisions §4): fbm score short-circuit first, then
    /// non-ocean + elevation band. Returns the jittered source point + score, or
    /// `None`. Pure function of (seed, cell). Neighbours re-run only THIS (cheap),
    /// never the full trace.
    pub(super) fn source_gate(&self, cx: i32, cz: i32) -> Option<SourceGate> {
        let ox = cx * CELL_BLOCKS;
        let oz = cz * CELL_BLOCKS;
        let center_x = ox + CELL_BLOCKS / 2;
        let center_z = oz + CELL_BLOCKS / 2;
        let mut rng = FeatureRng::positional(self.seed, SOURCE_SALT, cx, 0, cz);
        // Seed-uniform density: a per-cell roll gated by a probability, with the
        // low-freq `source` field only MODULATING density for regional clustering.
        let roll = rng.next_f32();
        let jx = rng.next_f32();
        let jz = rng.next_f32();
        let cluster = self.source.get([center_x as f64, center_z as f64]) as f32;
        let prob = (SOURCE_PROB + SOURCE_CLUSTER * cluster).clamp(0.05, 0.95);
        if roll > prob {
            return None;
        }

        let x = ox as f32 + CELL_BLOCKS as f32 * (0.20 + 0.60 * jx);
        let z = oz as f32 + CELL_BLOCKS as f32 * (0.20 + 0.60 * jz);
        let sx = x.round() as i32;
        let sz = z.round() as i32;
        if self.is_ocean_at(sx, sz) {
            return None;
        }
        let elev = self.coarse_elevation(sx, sz);
        if !(SOURCE_MIN_ELEV..=SOURCE_MAX_ELEV).contains(&elev) {
            return None;
        }
        // Score (regional wetness) ranks sources for suppress-weaker-of-two.
        Some(SourceGate { x, z, score: cluster })
    }

    /// Suppress-weaker-of-two (decisions §4): drop this cell's source if a
    /// neighbour within `MIN_SOURCE_SPACING` has a strictly higher score (tiebreak
    /// on `(cx,cz)`). Deterministic + symmetric, so exactly one of a close pair
    /// survives. Pure function of (seed, cell).
    pub(super) fn source_suppressed(&self, cx: i32, cz: i32, src: SourceGate) -> bool {
        for dz in -1..=1 {
            for dx in -1..=1 {
                if dx == 0 && dz == 0 {
                    continue;
                }
                let (nx, nz) = (cx + dx, cz + dz);
                let Some(other) = self.source_gate(nx, nz) else {
                    continue;
                };
                let ddx = other.x - src.x;
                let ddz = other.z - src.z;
                if ddx * ddx + ddz * ddz >= MIN_SOURCE_SPACING * MIN_SOURCE_SPACING {
                    continue;
                }
                let wins =
                    other.score > src.score || (other.score == src.score && (nz, nx) < (cz, cx));
                if wins {
                    return true;
                }
            }
        }
        false
    }
}
