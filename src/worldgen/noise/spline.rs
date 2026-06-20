//! Piecewise-linear splines + the peaks-and-valleys fold used by the height field.
//!
//! Modern voxel terrain shaping routes continentalness, erosion,
//! and peaks-and-valleys (PV) through SPLINES — `offset`, `factor`, `jaggedness` —
//! rather than summing fixed-amplitude noise layers. We stay a 2-D per-column
//! height field (a deliberate choice; see `height.rs`) but borrow that idea:
//!
//!   - **erosion is the master amplitude knob.** Low erosion keeps full relief;
//!     high erosion collapses terrain to near-flat. This is what makes flat plains
//!     abut jagged country instead of every region having the same mid-amplitude.
//!   - **continentalness sets the base lift ceiling** (how high inland may rise).
//!   - **the PV fold** turns a smooth noise into ridge-and-valley structure so the
//!     relief is directional (ridgelines, valleys) instead of round blobs.
//!
//! Splines are plain monotone-x tables sampled by `lerp_table`, so they are pure,
//! `const`, and trivially deterministic (no f32 cast points — kept all-f64).

/// Sample a piecewise-linear table at `x`, clamped to the endpoints. `pts` must be
/// sorted ascending by the first (x) component and be non-empty.
#[inline]
pub fn lerp_table(pts: &[(f64, f64)], x: f64) -> f64 {
    if x <= pts[0].0 {
        return pts[0].1;
    }
    let last = pts[pts.len() - 1];
    if x >= last.0 {
        return last.1;
    }
    let mut i = 0;
    while i < pts.len() - 1 && x > pts[i + 1].0 {
        i += 1;
    }
    let (x0, y0) = pts[i];
    let (x1, y1) = pts[i + 1];
    let t = if x1 > x0 { (x - x0) / (x1 - x0) } else { 0.0 };
    y0 + (y1 - y0) * t
}

/// Surface relief amplitude as a function of erosion01 (0 = rugged, 1 = smooth).
/// The reference `offset` spline collapses terrain ~15× from rugged to smooth; we
/// keep full amplitude at low erosion and drop to near-flat at high erosion.
#[inline]
pub fn erosion_amp(er01: f64) -> f64 {
    const PTS: [(f64, f64); 6] = [
        (0.00, 1.00),
        (0.20, 0.85),
        (0.45, 0.45),
        (0.62, 0.18),
        (0.80, 0.06),
        (1.00, 0.03),
    ];
    lerp_table(&PTS, er01)
}

/// The `factor` analogue: low erosion grows taller, fatter relief; high erosion
/// flattens it. Multiplies the continentalness uplift so mountains only emerge on
/// low-erosion high-continentalness ground.
#[inline]
pub fn erosion_relief_gain(er01: f64) -> f64 {
    const PTS: [(f64, f64); 4] = [(0.00, 1.0), (0.35, 0.7), (0.55, 0.35), (1.00, 0.15)];
    lerp_table(&PTS, er01)
}

/// Continentalness01 -> maximum inland uplift ceiling, in blocks above the base
/// floor. Zero in/near the ocean band, rising steeply far inland so big massifs
/// are reachable (the old additive stack capped out ~30 blocks of relief).
#[inline]
pub fn lift_ceiling(cont01: f64) -> f64 {
    const PTS: [(f64, f64); 4] = [(0.46, 0.0), (0.64, 18.0), (0.82, 40.0), (1.00, 78.0)];
    lerp_table(&PTS, cont01)
}

/// Peaks-and-valleys fold (`1 - |3|w| - 2|`): maps a signed noise in
/// [-1, 1] to ridge/valley structure in [-1, 1]. Input magnitude near 0 -> valley
/// floor (-1); near 2/3 -> ridge crest (+1); near 1 -> mid (0). Folding a smooth
/// noise this way yields long connected ridgelines and valleys rather than domes.
#[inline]
pub fn pv_fold(signed: f64) -> f64 {
    let a = signed.abs();
    1.0 - (3.0 * a - 2.0).abs()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lerp_table_hits_knots_and_interpolates() {
        const PTS: [(f64, f64); 3] = [(0.0, 0.0), (1.0, 10.0), (2.0, 12.0)];
        assert_eq!(lerp_table(&PTS, -5.0), 0.0); // clamp low
        assert_eq!(lerp_table(&PTS, 0.0), 0.0);
        assert_eq!(lerp_table(&PTS, 0.5), 5.0); // midpoint of first segment
        assert_eq!(lerp_table(&PTS, 1.0), 10.0);
        assert_eq!(lerp_table(&PTS, 1.5), 11.0);
        assert_eq!(lerp_table(&PTS, 9.0), 12.0); // clamp high
    }

    #[test]
    fn erosion_amp_is_monotone_decreasing() {
        let mut prev = f64::INFINITY;
        let mut e = 0.0;
        while e <= 1.0 {
            let a = erosion_amp(e);
            assert!(a <= prev + 1e-9, "erosion_amp not decreasing at {e}");
            prev = a;
            e += 0.05;
        }
        assert!(erosion_amp(0.0) > erosion_amp(1.0) * 10.0, "not enough flat/steep contrast");
    }

    #[test]
    fn pv_fold_ridges_and_valleys() {
        assert!((pv_fold(0.0) - (-1.0)).abs() < 1e-9); // valley floor
        assert!((pv_fold(2.0 / 3.0) - 1.0).abs() < 1e-9); // ridge crest
        assert!((pv_fold(1.0) - 0.0).abs() < 1e-9);
        assert_eq!(pv_fold(-0.5), pv_fold(0.5)); // symmetric in sign
    }
}
