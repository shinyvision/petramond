//! The weather field: a pure, closed-form function of (seed, position, clock).
//!
//! Every consumer — the weather mod's deterministic server tick, its
//! presentation-side client instance, and the cloud shader (which re-implements
//! the same lattice math in WGSL) — evaluates this function locally from a
//! handful of replicated parameters. There is no stored or synced field.
//!
//! The field is PERIODIC with period [`WRAP`] blocks so the advection offset
//! can be published modulo the period and stay f32-exact at any world age.
//! Every fbm octave's lattice tiles exactly because [`FEATURE_SIZE`] is a
//! power of two dividing [`WRAP`]. Time-driven lanes reduce the clock in u64
//! tick space before any float conversion, so their error stays bounded (a
//! few ticks of quantization near a lane period's end) at any world age.

/// Field period in blocks. Power of two; all octave lattices tile at it.
pub const WRAP: f32 = 65536.0;
/// Base octave feature size in blocks. Power of two dividing [`WRAP`].
pub const FEATURE_SIZE: f32 = 512.0;
/// Coverage at or above this starts to rain. Deliberately well inside the
/// fbm's reachable range: averaged noise concentrates around its middle, so
/// a high threshold makes rain vanishingly rare (playtest 2026-07-16 —
/// "thick clouds that never rain").
pub const RAIN_START: f32 = 0.45;
/// Fraction of the remaining coverage range over which rain ramps to a full
/// downpour: intensity 1 at `RAIN_START + RAIN_RAMP * (1 - RAIN_START)`,
/// because coverage itself almost never reaches 1. clouds.wgsl's
/// `menace_at` ends its white→slate ramp at the same point (its `0.6` is
/// this constant's twin) — keep them in sync.
pub const RAIN_RAMP: f32 = 0.6;

/// Wind heading turns over roughly this many seconds.
const WIND_TURN_PERIOD_S: f32 = 1200.0;
/// Wind speed gusts over roughly this many seconds.
const WIND_GUST_PERIOD_S: f32 = 420.0;
/// Global calm/stormy cycle length in seconds.
const STORM_PERIOD_S: f32 = 2400.0;
/// Wind speed range in blocks/s. The floor keeps cloud drift ALWAYS
/// perceptible (0.5 read as a frozen sky at deck altitude — playtest
/// 2026-07-17); the ceiling keeps rain slant and drift sane.
const WIND_MIN: f32 = 1.75;
const WIND_MAX: f32 = 7.0;

/// One tick = 1/20 s; the clock is `petramond:clock` absolute ticks.
const TICKS_PER_S: u64 = 20;
/// Ticks per morph epoch: the field cross-fades between two seedings of
/// itself over this window, so cloud shapes continuously REFORM while they
/// drift — without it the whole sky translates as one rigid pattern
/// (playtest 2026-07-17: "clouds move in a uniform straight line").
pub const EVOLVE_TICKS: u64 = 6000;

/// Replicated field parameters. The server mod publishes these as shader
/// params; the client mod reads them back; the shader receives them directly.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FieldParams {
    /// Accumulated wind advection offset, already reduced modulo [`WRAP`].
    pub off: [f32; 2],
    /// Global storm bias in [0, 1]: how much of the noise range is cloud.
    pub storm: f32,
    /// World-seed mix for the lattice hashes.
    pub seed: u32,
    /// Morph epoch (`clock / EVOLVE_TICKS`) and the blend fraction into the
    /// next epoch: the field is `lerp(field_e, field_e+1, frac)`.
    pub epoch: u32,
    pub epoch_frac: f32,
}

/// The epoch/fraction pair for a clock value.
pub fn epoch_at(clock_ticks: u64) -> (u32, f32) {
    (
        (clock_ticks / EVOLVE_TICKS) as u32,
        (clock_ticks % EVOLVE_TICKS) as f32 / EVOLVE_TICKS as f32,
    )
}

/// murmur3 fmix32 — the 32-bit finalizer both this crate and clouds.wgsl use.
/// Do not change one without the other.
#[inline]
pub fn fmix32(mut h: u32) -> u32 {
    h ^= h >> 16;
    h = h.wrapping_mul(0x85EB_CA6B);
    h ^= h >> 13;
    h = h.wrapping_mul(0xC2B2_AE35);
    h ^= h >> 16;
    h
}

/// Lattice corner hash in [0, 1). `ix`/`iz` are PRE-WRAPPED lattice indices.
#[inline]
fn corner(ix: u32, iz: u32, seed: u32) -> f32 {
    let h = fmix32(ix.wrapping_mul(0x9E37_79B9) ^ iz.wrapping_mul(0x85EB_CA6B) ^ seed);
    (h >> 8) as f32 / 16_777_216.0
}

#[inline]
fn smooth(t: f32) -> f32 {
    t * t * (3.0 - 2.0 * t)
}

#[inline]
fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// Periodic 2D value noise in [0, 1). `p` in lattice units; the lattice
/// tiles every `period` cells (power of two, so `& (period - 1)` wraps).
fn vnoise2(px: f32, pz: f32, period: u32, seed: u32) -> f32 {
    let fx = px.floor();
    let fz = pz.floor();
    let tx = smooth(px - fx);
    let tz = smooth(pz - fz);
    let mask = period - 1;
    // rem_euclid before the cast keeps negatives correct; the mask wraps the
    // +1 neighbours.
    let ix = (fx as i64).rem_euclid(period as i64) as u32;
    let iz = (fz as i64).rem_euclid(period as i64) as u32;
    let x1 = (ix + 1) & mask;
    let z1 = (iz + 1) & mask;
    let a = corner(ix, iz, seed);
    let b = corner(x1, iz, seed);
    let c = corner(ix, z1, seed);
    let d = corner(x1, z1, seed);
    lerp(lerp(a, b, tx), lerp(c, d, tx), tz)
}

/// Periodic 1D value noise in [0, 1) over a `period`-cell lattice.
fn vnoise1(t: f32, period: u32, seed: u32) -> f32 {
    let ft = t.floor();
    let tt = smooth(t - ft);
    let mask = period - 1;
    let i0 = (ft as i64).rem_euclid(period as i64) as u32;
    let i1 = (i0 + 1) & mask;
    lerp(corner(i0, 0, seed), corner(i1, 0, seed), tt)
}

/// Three-octave fbm of the periodic noise, normalized to [0, 1). `qx/qz`
/// are the UNADVECTED lattice coordinates and `ox/oz` the advection offset
/// in lattice units: the middle octave slides at 2× the wind (an INTEGER
/// multiple — anything else breaks the wrap-exactness — so structure shears
/// through the larger shapes instead of riding them rigidly).
fn fbm2(qx: f32, qz: f32, ox: f32, oz: f32, seed: u32) -> f32 {
    let base = (WRAP / FEATURE_SIZE) as u32;
    let n0 = vnoise2(qx - ox, qz - oz, base, seed);
    let n1 = vnoise2(
        (qx - 2.0 * ox) * 2.0,
        (qz - 2.0 * oz) * 2.0,
        base * 2,
        seed ^ 0x9E37_79B9,
    );
    let n2 = vnoise2((qx - ox) * 4.0, (qz - oz) * 4.0, base * 4, seed ^ 0x3C6E_F372);
    (n0 + 0.5 * n1 + 0.25 * n2) / 1.75
}

#[inline]
fn saturate(v: f32) -> f32 {
    v.clamp(0.0, 1.0)
}

/// Wrap a world coordinate or offset into [0, WRAP).
#[inline]
pub fn wrap_coord(v: f64) -> f32 {
    v.rem_euclid(WRAP as f64) as f32
}

/// Time-lane sample: reduce the clock in tick space, then evaluate a slow
/// periodic 1D noise. `period_s` must divide the lattice tiling evenly, which
/// it does by construction (the lattice is `TIME_CELLS` cells of `period_s`).
fn time_lane(clock_ticks: u64, period_s: f32, seed: u32) -> f32 {
    /// Cells in every time lattice; the lane repeats after
    /// `period_s * TIME_CELLS` seconds (days–weeks: unobservable).
    const TIME_CELLS: u32 = 4096;
    let period_ticks = (period_s as u64) * TICKS_PER_S * TIME_CELLS as u64;
    let reduced = (clock_ticks % period_ticks) as f32 / TICKS_PER_S as f32;
    vnoise1(reduced / period_s, TIME_CELLS, seed)
}

/// Current wind velocity in blocks/s (already direction × speed).
pub fn wind(clock_ticks: u64, seed: u32) -> [f32; 2] {
    let angle =
        std::f32::consts::TAU * time_lane(clock_ticks, WIND_TURN_PERIOD_S, seed ^ 0xA511_E9B3);
    let speed = WIND_MIN
        + (WIND_MAX - WIND_MIN) * time_lane(clock_ticks, WIND_GUST_PERIOD_S, seed ^ 0x63D8_3595);
    [speed * angle.cos(), speed * angle.sin()]
}

/// Global calm↔stormy cycle in [0.35, 0.75].
pub fn storm(clock_ticks: u64, seed: u32) -> f32 {
    0.35 + 0.4 * time_lane(clock_ticks, STORM_PERIOD_S, seed ^ 0x94D0_49BB)
}

/// Cloud coverage in [0, 1] at world xz. Denser = darker = closer to rain.
/// Cross-fades between two epoch seedings so shapes morph while drifting.
pub fn coverage(x: f32, z: f32, p: &FieldParams) -> f32 {
    let qx = x.rem_euclid(WRAP) / FEATURE_SIZE;
    let qz = z.rem_euclid(WRAP) / FEATURE_SIZE;
    let ox = p.off[0] / FEATURE_SIZE;
    let oz = p.off[1] / FEATURE_SIZE;
    let seed_a = p.seed ^ fmix32(p.epoch);
    let seed_b = p.seed ^ fmix32(p.epoch.wrapping_add(1));
    let n = lerp(
        fbm2(qx, qz, ox, oz, seed_a),
        fbm2(qx, qz, ox, oz, seed_b),
        p.epoch_frac.clamp(0.0, 1.0),
    );
    let lo = 1.0 - p.storm;
    saturate((n - lo) / (1.0 - lo).max(1e-3))
}

/// Rain intensity in [0, 1] from a coverage value: 0 below [`RAIN_START`],
/// a full downpour from `RAIN_START + RAIN_RAMP * (1 - RAIN_START)` up.
#[inline]
pub fn rain_from_coverage(cov: f32) -> f32 {
    saturate((cov - RAIN_START) / ((1.0 - RAIN_START) * RAIN_RAMP))
}

/// Convenience: rain intensity at world xz.
pub fn rain(x: f32, z: f32, p: &FieldParams) -> f32 {
    rain_from_coverage(coverage(x, z, p))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params(off: [f32; 2], storm: f32) -> FieldParams {
        FieldParams {
            off,
            storm,
            seed: 0xDEAD_BEEF,
            epoch: 3,
            epoch_frac: 0.4,
        }
    }

    #[test]
    fn coverage_and_rain_stay_in_unit_range() {
        let p = params([123.4, 9876.5], 0.6);
        for i in 0..500 {
            let x = (i as f32) * 731.7 - 100_000.0;
            let z = (i as f32) * -211.3 + 5_000.0;
            let c = coverage(x, z, &p);
            assert!((0.0..=1.0).contains(&c), "coverage {c} at {x},{z}");
            let r = rain_from_coverage(c);
            assert!((0.0..=1.0).contains(&r));
            assert!(
                c >= RAIN_START || r == 0.0,
                "no rain below the threshold (cov {c}, rain {r})"
            );
            assert!(
                c < RAIN_START + (1.0 - RAIN_START) * RAIN_RAMP || r == 1.0,
                "a full downpour from the ramp's end (cov {c}, rain {r})"
            );
        }
    }

    #[test]
    fn field_is_periodic_in_wrap() {
        let p = params([777.0, 3333.0], 0.55);
        for i in 0..64 {
            let x = i as f32 * 917.3;
            let z = i as f32 * 391.9;
            let a = coverage(x, z, &p);
            let b = coverage(x + WRAP, z - 2.0 * WRAP, &p);
            assert!((a - b).abs() < 1e-4, "period broken at {x},{z}: {a} vs {b}");
        }
    }

    #[test]
    fn advection_wraps_exactly_and_actually_moves_the_field() {
        // The middle octave advects at 2x (shear), so the old rigid
        // translation identity is gone BY DESIGN. What must still hold:
        // a full-period offset shift is an exact identity (2x an integer
        // multiple of the period is still one), and a partial shift really
        // moves the pattern.
        let base = params([1000.0, 2000.0], 0.6);
        let mut wrapped = base;
        wrapped.off = [base.off[0] + WRAP, base.off[1] - WRAP];
        let mut shifted = base;
        shifted.off = [base.off[0] + 37.0, base.off[1] + 61.0];
        let mut moved = 0;
        for i in 0..64 {
            let x = i as f32 * 137.0;
            let z = i as f32 * 89.0;
            let a = coverage(x, z, &base);
            assert!(
                (a - coverage(x, z, &wrapped)).abs() < 1e-4,
                "full-period offset must be identity at {x},{z}"
            );
            if (a - coverage(x, z, &shifted)).abs() > 0.02 {
                moved += 1;
            }
        }
        assert!(moved > 16, "a partial offset shift must move the field");
    }

    #[test]
    fn coverage_is_continuous_across_lattice_cell_edges() {
        let p = params([0.0, 0.0], 0.6);
        // Walk across several base-lattice boundaries in small steps; the
        // field must never jump more than the local slope allows.
        let mut prev = coverage(FEATURE_SIZE - 2.0, 100.0, &p);
        let mut x = FEATURE_SIZE - 2.0;
        while x < FEATURE_SIZE + 2.0 {
            x += 0.05;
            let c = coverage(x, 100.0, &p);
            assert!(
                (c - prev).abs() < 0.02,
                "discontinuity near cell edge at x={x}"
            );
            prev = c;
        }
    }

    #[test]
    fn storm_widens_coverage_monotonically() {
        let calm = params([50.0, 60.0], 0.36);
        let stormy = params([50.0, 60.0], 0.74);
        let mut widened = 0;
        for i in 0..200 {
            let x = i as f32 * 419.1;
            let z = i as f32 * 267.7;
            let a = coverage(x, z, &calm);
            let b = coverage(x, z, &stormy);
            assert!(b >= a - 1e-6, "storm bias must never shrink coverage");
            if b > a + 0.05 {
                widened += 1;
            }
        }
        assert!(
            widened > 40,
            "a stormier bias should visibly widen cloud cover"
        );
    }

    #[test]
    fn time_lanes_are_smooth_and_exact_at_huge_clocks() {
        // A clock deep into a world's life must still produce smooth wind.
        let base: u64 = 20 * 3600 * 24 * 3650; // ten game-years of ticks
        let mut prev = wind(base, 7);
        for step in 1..200u64 {
            let w = wind(base + step, 7);
            let d = ((w[0] - prev[0]).powi(2) + (w[1] - prev[1]).powi(2)).sqrt();
            assert!(d < 0.05, "wind jumped {d} in one tick at step {step}");
            prev = w;
        }
        let s = storm(base, 7);
        assert!((0.35..=0.75).contains(&s));
    }

    #[test]
    fn epoch_morph_is_continuous_at_the_boundary() {
        // frac→1 of epoch e equals frac=0 of epoch e+1 exactly.
        let mut a = params([1200.0, 400.0], 0.6);
        a.epoch = 9;
        a.epoch_frac = 1.0;
        let mut b = a;
        b.epoch = 10;
        b.epoch_frac = 0.0;
        for i in 0..64 {
            let x = i as f32 * 173.3;
            let z = i as f32 * 91.7;
            let ca = coverage(x, z, &a);
            let cb = coverage(x, z, &b);
            assert!((ca - cb).abs() < 1e-5, "epoch seam at {x},{z}: {ca} vs {cb}");
        }
        // And epoch_at ticks over exactly at the period.
        assert_eq!(epoch_at(EVOLVE_TICKS * 7), (7, 0.0));
        let (e, f) = epoch_at(EVOLVE_TICKS * 7 + EVOLVE_TICKS / 2);
        assert_eq!(e, 7);
        assert!((f - 0.5).abs() < 1e-6);
    }

    #[test]
    fn wrap_coord_reduces_exactly() {
        assert_eq!(wrap_coord(65536.0 * 3.0 + 12.25), 12.25);
        assert_eq!(wrap_coord(-1.5), WRAP as f32 - 1.5);
    }
}
