//! The cave fields' 3-D OpenSimplex sampler.
//!
//! A bit-exact re-expression of `noise-0.9`'s `open_simplex_3d` over a
//! `PermutationTable`, written for the shape this engine calls it in: a plain
//! 256-byte table by value, fixed-size arrays instead of generic `Vector3<T>`,
//! and the hash unrolled instead of an iterator `reduce` over a slice. The
//! crate's generic path spends more time on `numcast`, slice iteration and
//! bounds checks than on the noise, and this is the single hottest primitive in
//! worldgen — every cave lattice corner is eight of these.
//!
//! EVERY float operation is in the crate's order, including the `0.0 +` that
//! starts its `sum`/`dot` folds and its unusual "floor" (truncation, minus one
//! at or below zero). The permutation table is EXTRACTED from the crate's own
//! `PermutationTable` rather than re-derived, so the seed→table chain cannot
//! drift either. `genparity` is the gate on all of it.

use noise::permutationtable::{NoiseHasher, PermutationTable};

const STRETCH: f64 = -1.0 / 6.0;
const SQUISH: f64 = 1.0 / 3.0;
const NORM: f64 = 1.0 / 14.0;

const DIAG: f64 = std::f64::consts::FRAC_1_SQRT_2;
const DIAG2: f64 = 0.577_350_269_189_625_8;

#[rustfmt::skip]
const GRAD3: [[f64; 3]; 32] = [
    [ DIAG,   DIAG,   0.0], [-DIAG,   DIAG,   0.0], [ DIAG,  -DIAG,   0.0], [-DIAG,  -DIAG,   0.0],
    [ DIAG,    0.0,  DIAG], [-DIAG,    0.0,  DIAG], [ DIAG,    0.0, -DIAG], [-DIAG,    0.0, -DIAG],
    [  0.0,   DIAG,  DIAG], [  0.0,  -DIAG,  DIAG], [  0.0,   DIAG, -DIAG], [  0.0,  -DIAG, -DIAG],
    [ DIAG,   DIAG,   0.0], [-DIAG,   DIAG,   0.0], [ DIAG,  -DIAG,   0.0], [-DIAG,  -DIAG,   0.0],
    [ DIAG,    0.0,  DIAG], [-DIAG,    0.0,  DIAG], [ DIAG,    0.0, -DIAG], [-DIAG,    0.0, -DIAG],
    [  0.0,   DIAG,  DIAG], [  0.0,  -DIAG,  DIAG], [  0.0,   DIAG, -DIAG], [  0.0,  -DIAG, -DIAG],
    [ DIAG2,  DIAG2,  DIAG2], [-DIAG2,  DIAG2,  DIAG2],
    [ DIAG2, -DIAG2,  DIAG2], [-DIAG2, -DIAG2,  DIAG2],
    [ DIAG2,  DIAG2, -DIAG2], [-DIAG2,  DIAG2, -DIAG2],
    [ DIAG2, -DIAG2, -DIAG2], [-DIAG2, -DIAG2, -DIAG2],
];

/// One seeded 3-D OpenSimplex field.
#[derive(Clone)]
pub(super) struct Simplex3 {
    perm: [u8; 256],
}

impl Simplex3 {
    pub(super) fn new(seed: u32) -> Self {
        // `hash(&[i])` is the table lookup itself (a one-element fold returns
        // `values[i]`), so this reads the crate's table out through its own
        // public trait instead of re-deriving the seed chain.
        let table = PermutationTable::new(seed);
        Self {
            perm: std::array::from_fn(|i| table.hash(&[i as isize]) as u8),
        }
    }

    #[inline]
    fn hash(&self, x: i64, y: i64, z: i64) -> usize {
        let a = self.perm[(x & 0xff) as usize] as usize ^ (y & 0xff) as usize;
        let b = self.perm[a] as usize ^ (z & 0xff) as usize;
        self.perm[b] as usize
    }

    #[inline]
    fn surflet(&self, vx: i64, vy: i64, vz: i64, dx: f64, dy: f64, dz: f64) -> f64 {
        let mut m = 0.0;
        m += dx * dx;
        m += dy * dy;
        m += dz * dz;
        let t = 2.0 - m;
        if t <= 0.0 {
            return 0.0;
        }
        let g = &GRAD3[self.hash(vx, vy, vz) % 32];
        let mut d = 0.0;
        d += dx * g[0];
        d += dy * g[1];
        d += dz * g[2];
        t.powi(4) * d
    }

    pub(super) fn get(&self, p: [f64; 3]) -> f64 {
        let mut sum = 0.0;
        sum += p[0];
        sum += p[1];
        sum += p[2];
        let stretch_offset = sum * STRETCH;
        let s = [
            p[0] + stretch_offset,
            p[1] + stretch_offset,
            p[2] + stretch_offset,
        ];

        // The crate's `floor_to_isize`: truncation, minus one at or below zero.
        let fi: [i64; 3] = std::array::from_fn(|i| {
            let t = s[i] as i64;
            if s[i] <= 0.0 {
                t - 1
            } else {
                t
            }
        });
        let f = [fi[0] as f64, fi[1] as f64, fi[2] as f64];

        let mut fsum = 0.0;
        fsum += f[0];
        fsum += f[1];
        fsum += f[2];
        let squish_offset = fsum * SQUISH;
        let rel = [
            p[0] - (f[0] + squish_offset),
            p[1] - (f[1] + squish_offset),
            p[2] - (f[2] + squish_offset),
        ];

        let mut region = 0.0;
        region += s[0] - f[0];
        region += s[1] - f[1];
        region += s[2] - f[2];

        macro_rules! contribute {
            ($ox:expr, $oy:expr, $oz:expr) => {{
                let mut osum = 0.0;
                osum += $ox;
                osum += $oy;
                osum += $oz;
                let sq = SQUISH * osum;
                self.surflet(
                    fi[0] + $ox as i64,
                    fi[1] + $oy as i64,
                    fi[2] + $oz as i64,
                    rel[0] - sq - $ox,
                    rel[1] - sq - $oy,
                    rel[2] - sq - $oz,
                )
            }};
        }

        let mut value = 0.0;
        if region <= 1.0 {
            value += contribute!(0.0, 0.0, 0.0);
            value += contribute!(1.0, 0.0, 0.0);
            value += contribute!(0.0, 1.0, 0.0);
            value += contribute!(0.0, 0.0, 1.0);
        } else if region >= 2.0 {
            value += contribute!(1.0, 1.0, 0.0);
            value += contribute!(1.0, 0.0, 1.0);
            value += contribute!(0.0, 1.0, 1.0);
            value += contribute!(1.0, 1.0, 1.0);
        } else {
            value += contribute!(1.0, 0.0, 0.0);
            value += contribute!(0.0, 1.0, 0.0);
            value += contribute!(0.0, 0.0, 1.0);
            value += contribute!(1.0, 1.0, 0.0);
            value += contribute!(1.0, 0.0, 1.0);
            value += contribute!(0.0, 1.0, 1.0);
        }
        value * NORM
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use noise::NoiseFn;

    /// The whole point of this file is that it is the crate's function with the
    /// generics unrolled. Anything else is a silent world change, so it is
    /// pinned bit for bit — over the coordinate shapes the cave lattice
    /// actually produces (world-anchored corners scaled by the field
    /// frequencies, which land on both sides of every branch and on exact
    /// integers and zeros).
    #[test]
    fn matches_the_reference_sampler_bit_for_bit() {
        for seed in [0u32, 1, 0x312, 0xDEAD_BEEF, u32::MAX] {
            let ours = Simplex3::new(seed);
            let theirs = noise::OpenSimplex::new(seed);
            let mut st = 0x2545_F491_4F6C_DD1Du64 ^ seed as u64;
            let mut next = || {
                st ^= st << 13;
                st ^= st >> 7;
                st ^= st << 17;
                (st >> 11) as f64 / (1u64 << 53) as f64
            };
            for i in 0..4000 {
                // A mix of exact lattice-like coordinates and arbitrary ones,
                // spanning both signs and zero.
                let p = if i % 4 == 0 {
                    let c = |k: i32| (k * 4) as f64 * 0.011;
                    [c(i - 2000), c((i * 7) % 511 - 255), c(-i)]
                } else {
                    [
                        (next() - 0.5) * 900.0,
                        (next() - 0.5) * 300.0,
                        (next() - 0.5) * 900.0,
                    ]
                };
                assert_eq!(
                    ours.get(p).to_bits(),
                    theirs.get(p).to_bits(),
                    "seed {seed} at {p:?}"
                );
            }
            for p in [
                [0.0; 3],
                [1.0, 0.0, -1.0],
                [-0.0, 3.0, 0.0],
                [-7.0, -7.0, 7.0],
            ] {
                assert_eq!(ours.get(p).to_bits(), theirs.get(p).to_bits(), "{p:?}");
            }
        }
    }
}
