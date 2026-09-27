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

mod field_constants;
use field_constants::SHEET_B_SALT;
pub use field_constants::{FEATURE_SIZE, RAIN_RAMP, SHEET_B_ADVECT, SHEET_B_FEATURE, WRAP};
/// Coverage at or above this starts to rain. With the two-sheet
/// sum distribution, at a
/// fixed point it rains ~15% of the time in ~3-min showers grouped into
/// ~20-min rainy spells, and never blankets outside a storm peak.
pub const RAIN_START: f32 = 0.55;

const WIND_TURN_PERIOD_S: f32 = 1200.0;
const WIND_GUST_PERIOD_S: f32 = 420.0;
const STORM_PERIOD_S: f32 = 2400.0;
const WIND_MIN: f32 = 0.75;
const WIND_MAX: f32 = 2.0;

/// One tick = 1/20 s; the clock is `petramond:clock` absolute ticks.
const TICKS_PER_S: u64 = 20;
/// Ticks per morph epoch: the field cross-fades between two seedings of
/// itself over this window, so cloud shapes continuously reform while they
/// drift. The two sheets also slide over each other; this 10-minute morph
/// window preserves shower length while changing the cloud shapes.
pub const EVOLVE_TICKS: u64 = 12000;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FieldParams {
    pub off: [f32; 2],
    pub storm: f32,
    pub seed: u32,
    pub epoch: u32,
    pub epoch_frac: f32,
}

pub fn epoch_at(clock_ticks: u64) -> (u32, f32) {
    (
        (clock_ticks / EVOLVE_TICKS) as u32,
        (clock_ticks % EVOLVE_TICKS) as f32 / EVOLVE_TICKS as f32,
    )
}

#[inline]
pub fn fmix32(mut h: u32) -> u32 {
    h ^= h >> 16;
    h = h.wrapping_mul(0x85EB_CA6B);
    h ^= h >> 13;
    h = h.wrapping_mul(0xC2B2_AE35);
    h ^= h >> 16;
    h
}

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

fn vnoise2(px: f32, pz: f32, period: u32, seed: u32) -> f32 {
    let fx = px.floor();
    let fz = pz.floor();
    let tx = smooth(px - fx);
    let tz = smooth(pz - fz);
    let mask = period - 1;
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

fn vnoise1(t: f32, period: u32, seed: u32) -> f32 {
    let ft = t.floor();
    let tt = smooth(t - ft);
    let mask = period - 1;
    let i0 = (ft as i64).rem_euclid(period as i64) as u32;
    let i1 = (i0 + 1) & mask;
    lerp(corner(i0, 0, seed), corner(i1, 0, seed), tt)
}

/// Three-octave fbm of the periodic noise, normalized to [0, 1). `qx/qz`
/// are the UNADVECTED lattice coordinates (in `feature`-sized cells) and
/// `ox/oz` the advection offset in the same units: the middle octave slides
/// at 2× the wind (an INTEGER multiple — anything else breaks the
/// wrap-exactness — so structure shears through the larger shapes instead
/// of riding them rigidly).
fn fbm2(qx: f32, qz: f32, ox: f32, oz: f32, seed: u32, feature: f32) -> f32 {
    let base = (WRAP / feature) as u32;
    let n0 = vnoise2(qx - ox, qz - oz, base, seed);
    let n1 = vnoise2(
        (qx - 2.0 * ox) * 2.0,
        (qz - 2.0 * oz) * 2.0,
        base * 2,
        seed ^ 0x9E37_79B9,
    );
    let n2 = vnoise2(
        (qx - ox) * 4.0,
        (qz - oz) * 4.0,
        base * 4,
        seed ^ 0x3C6E_F372,
    );
    (n0 + 0.5 * n1 + 0.25 * n2) / 1.75
}

#[inline]
fn saturate(v: f32) -> f32 {
    v.clamp(0.0, 1.0)
}

#[inline]
pub fn wrap_coord(v: f64) -> f32 {
    v.rem_euclid(WRAP as f64) as f32
}

fn time_lane(clock_ticks: u64, period_s: f32, seed: u32) -> f32 {
    const TIME_CELLS: u32 = 4096;
    let period_ticks = (period_s as u64) * TICKS_PER_S * TIME_CELLS as u64;
    let reduced = (clock_ticks % period_ticks) as f32 / TICKS_PER_S as f32;
    vnoise1(reduced / period_s, TIME_CELLS, seed)
}

pub fn wind(clock_ticks: u64, seed: u32) -> [f32; 2] {
    let angle =
        std::f32::consts::TAU * time_lane(clock_ticks, WIND_TURN_PERIOD_S, seed ^ 0xA511_E9B3);
    let speed = WIND_MIN
        + (WIND_MAX - WIND_MIN) * time_lane(clock_ticks, WIND_GUST_PERIOD_S, seed ^ 0x63D8_3595);
    [speed * angle.cos(), speed * angle.sin()]
}

pub fn storm(clock_ticks: u64, seed: u32) -> f32 {
    0.35 + 0.37 * time_lane(clock_ticks, STORM_PERIOD_S, seed ^ 0x94D0_49BB)
}

pub fn field_params(off: [f64; 2], clock_ticks: u64, seed: u32) -> FieldParams {
    let (epoch, epoch_frac) = epoch_at(clock_ticks);
    FieldParams {
        off: [wrap_coord(off[0]), wrap_coord(off[1])],
        storm: storm(clock_ticks, seed),
        seed,
        epoch,
        epoch_frac,
    }
}

pub fn advance_offset(off: [f64; 2], clock_ticks: u64, seed: u32) -> [f64; 2] {
    let w = wind(clock_ticks, seed);
    let wrap = f64::from(WRAP);
    let per_tick = TICKS_PER_S as f64;
    [
        (off[0] + f64::from(w[0]) / per_tick).rem_euclid(wrap),
        (off[1] + f64::from(w[1]) / per_tick).rem_euclid(wrap),
    ]
}

fn sheet(x: f64, z: f64, p: &FieldParams, salt: u32, feature: f32, advect: f32) -> f32 {
    let qx = x.rem_euclid(f64::from(WRAP)) as f32 / feature;
    let qz = z.rem_euclid(f64::from(WRAP)) as f32 / feature;
    let ox = advect * p.off[0] / feature;
    let oz = advect * p.off[1] / feature;
    let seed_a = p.seed ^ fmix32(p.epoch) ^ salt;
    let seed_b = p.seed ^ fmix32(p.epoch.wrapping_add(1)) ^ salt;
    let n = lerp(
        fbm2(qx, qz, ox, oz, seed_a, feature),
        fbm2(qx, qz, ox, oz, seed_b, feature),
        p.epoch_frac.clamp(0.0, 1.0),
    );
    let lo = 1.0 - p.storm;
    saturate((n - lo) / (1.0 - lo).max(1e-3))
}

pub fn coverage(x: f64, z: f64, p: &FieldParams) -> f32 {
    let ca = sheet(x, z, p, 0, FEATURE_SIZE, 1.0);
    let cb = sheet(x, z, p, SHEET_B_SALT, SHEET_B_FEATURE, SHEET_B_ADVECT);
    saturate(ca + cb)
}

#[inline]
pub fn rain_from_coverage(cov: f32) -> f32 {
    saturate((cov - RAIN_START) / ((1.0 - RAIN_START) * RAIN_RAMP))
}

pub fn rain(x: f64, z: f64, p: &FieldParams) -> f32 {
    rain_from_coverage(coverage(x, z, p))
}

pub const CLOCK_KEY: &str = "petramond:clock";

pub fn decode_clock(bytes: &[u8]) -> Option<u64> {
    Some(u64::from_le_bytes(bytes.try_into().ok()?))
}

pub const DIRECT_SKY_MIN: u8 = 45;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FieldRow {
    pub params: FieldParams,
    pub wind: [f32; 2],
    pub clock: u64,
}

impl FieldRow {
    pub const VERSION: u8 = 1;
    pub const ENCODED_LEN: usize = 41;

    pub fn encode(&self) -> [u8; Self::ENCODED_LEN] {
        let mut out = [0u8; Self::ENCODED_LEN];
        out[0] = Self::VERSION;
        out[1..9].copy_from_slice(&self.clock.to_le_bytes());
        let lanes: [u32; 8] = [
            self.params.off[0].to_bits(),
            self.params.off[1].to_bits(),
            self.params.storm.to_bits(),
            self.params.seed,
            self.params.epoch,
            self.params.epoch_frac.to_bits(),
            self.wind[0].to_bits(),
            self.wind[1].to_bits(),
        ];
        for (i, lane) in lanes.into_iter().enumerate() {
            out[9 + i * 4..13 + i * 4].copy_from_slice(&lane.to_le_bytes());
        }
        out
    }

    pub fn decode(bytes: &[u8]) -> Option<Self> {
        if bytes.len() != Self::ENCODED_LEN || bytes[0] != Self::VERSION {
            return None;
        }
        let clock = u64::from_le_bytes(bytes[1..9].try_into().unwrap());
        let lane = |i: usize| u32::from_le_bytes(bytes[9 + i * 4..13 + i * 4].try_into().unwrap());
        let f = |i: usize| f32::from_bits(lane(i));
        let row = FieldRow {
            params: FieldParams {
                off: [f(0), f(1)],
                storm: f(2),
                seed: lane(3),
                epoch: lane(4),
                epoch_frac: f(5),
            },
            wind: [f(6), f(7)],
            clock,
        };
        let floats = [
            row.params.off[0],
            row.params.off[1],
            row.params.storm,
            row.params.epoch_frac,
            row.wind[0],
            row.wind[1],
        ];
        floats.iter().all(|v| v.is_finite()).then_some(row)
    }

    pub fn params_at(&self, clock: u64) -> FieldParams {
        if clock == self.clock {
            return self.params;
        }
        let off = [f64::from(self.params.off[0]), f64::from(self.params.off[1])];
        field_params(
            advance_offset(off, clock, self.params.seed),
            clock,
            self.params.seed,
        )
    }
}

#[cfg(feature = "sdk")]
pub mod feed;

#[cfg(test)]
mod shader_parity;

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
            let x = (i as f64) * 731.7 - 100_000.0;
            let z = (i as f64) * -211.3 + 5_000.0;
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
            let x = i as f64 * 917.3;
            let z = i as f64 * 391.9;
            let a = coverage(x, z, &p);
            let b = coverage(x + f64::from(WRAP), z - 2.0 * f64::from(WRAP), &p);
            assert!((a - b).abs() < 1e-4, "period broken at {x},{z}: {a} vs {b}");
        }
    }

    #[test]
    fn advection_wraps_exactly_and_actually_moves_the_field() {
        let base = params([1000.0, 2000.0], 0.6);
        let mut wrapped = base;
        wrapped.off = [base.off[0] + WRAP, base.off[1] - WRAP];
        let mut shifted = base;
        shifted.off = [base.off[0] + 37.0, base.off[1] + 61.0];
        let mut moved = 0;
        for i in 0..64 {
            let x = i as f64 * 137.0;
            let z = i as f64 * 89.0;
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
        let mut prev = coverage(f64::from(FEATURE_SIZE) - 2.0, 100.0, &p);
        let mut x = f64::from(FEATURE_SIZE) - 2.0;
        while x < f64::from(FEATURE_SIZE) + 2.0 {
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
            let x = i as f64 * 419.1;
            let z = i as f64 * 267.7;
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
        let base: u64 = 20 * 3600 * 24 * 3650;
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
        let mut a = params([1200.0, 400.0], 0.6);
        a.epoch = 9;
        a.epoch_frac = 1.0;
        let mut b = a;
        b.epoch = 10;
        b.epoch_frac = 0.0;
        for i in 0..64 {
            let x = i as f64 * 173.3;
            let z = i as f64 * 91.7;
            let ca = coverage(x, z, &a);
            let cb = coverage(x, z, &b);
            assert!(
                (ca - cb).abs() < 1e-5,
                "epoch seam at {x},{z}: {ca} vs {cb}"
            );
        }
        assert_eq!(epoch_at(EVOLVE_TICKS * 7), (7, 0.0));
        let (e, f) = epoch_at(EVOLVE_TICKS * 7 + EVOLVE_TICKS / 2);
        assert_eq!(e, 7);
        assert!((f - 0.5).abs() < 1e-6);
    }

    #[test]
    fn wrap_coord_reduces_exactly() {
        assert_eq!(wrap_coord(65536.0 * 3.0 + 12.25), 12.25);
        assert_eq!(wrap_coord(-1.5), WRAP - 1.5);
    }

    #[test]
    fn field_row_roundtrips_and_rejects_junk() {
        let row = FieldRow {
            params: params([12345.5, 60000.25], 0.61),
            wind: [-1.25, 0.75],
            clock: u64::MAX - 17,
        };
        let bytes = row.encode();
        assert_eq!(bytes.len(), FieldRow::ENCODED_LEN);
        assert_eq!(FieldRow::decode(&bytes), Some(row), "exact roundtrip");
        assert_eq!(FieldRow::decode(&bytes[..40]), None, "wrong length");
        assert_eq!(FieldRow::decode(&[]), None, "empty");
        let mut other = bytes;
        other[0] = FieldRow::VERSION + 1;
        assert_eq!(FieldRow::decode(&other), None, "another layout version");
        let mut nan = row;
        nan.params.storm = f32::NAN;
        assert_eq!(
            FieldRow::decode(&nan.encode()),
            None,
            "non-finite floats must read as no-weather, never as NaN rain"
        );
    }

    #[test]
    fn a_row_advanced_one_tick_matches_the_producers_next_row() {
        let seed = 0x00AB_CDEF;
        let clock = 20 * 3600 * 7 + 11;
        let off = [60_123.456_7, 12.345_6];
        let row = FieldRow {
            params: field_params(off, clock, seed),
            wind: wind(clock, seed),
            clock,
        };
        assert_eq!(
            row.params_at(clock),
            row.params,
            "an unmoved clock is the row"
        );
        let next = field_params(advance_offset(off, clock + 1, seed), clock + 1, seed);
        let replayed = row.params_at(clock + 1);
        assert_eq!(
            (
                replayed.storm,
                replayed.seed,
                replayed.epoch,
                replayed.epoch_frac
            ),
            (next.storm, next.seed, next.epoch, next.epoch_frac),
            "the clock-driven lanes are exact"
        );
        for axis in 0..2 {
            assert!(
                (replayed.off[axis] - next.off[axis]).abs() < 0.01,
                "the offset differs only by the row's f32 rounding: {:?} vs {:?}",
                replayed.off,
                next.off
            );
        }
        assert_ne!(replayed.off, row.params.off, "the step really advects");
    }
}
