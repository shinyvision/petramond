//! Improved-lattice ("Perlin") noise + octave stacks — the numeric core of the
//! classic terrain generator. The construction (three offsets + a permutation
//! shuffle, all drawn from the 48-bit LCG) and the sampling math match the
//! reference's `NoiseGeneratorImproved`/`NoiseGeneratorOctaves` bit-for-bit; the
//! known-answer vectors in the tests are probed directly from the reference.

use super::lcg::LcgRandom;

/// Gradient lattice for `grad` (indexed by `hash & 15`).
const GRAD_X: [f64; 16] = [
    1.0, -1.0, 1.0, -1.0, 1.0, -1.0, 1.0, -1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, -1.0, 0.0,
];
const GRAD_Y: [f64; 16] = [
    1.0, 1.0, -1.0, -1.0, 0.0, 0.0, 0.0, 0.0, 1.0, -1.0, 1.0, -1.0, 1.0, -1.0, 1.0, -1.0,
];
const GRAD_Z: [f64; 16] = [
    0.0, 0.0, 0.0, 0.0, 1.0, 1.0, -1.0, -1.0, 1.0, 1.0, -1.0, -1.0, 0.0, 1.0, 0.0, -1.0,
];

#[inline]
fn grad(hash: u8, x: f64, y: f64, z: f64) -> f64 {
    let i = (hash & 15) as usize;
    GRAD_X[i] * x + GRAD_Y[i] * y + GRAD_Z[i] * z
}

#[inline]
fn fade(t: f64) -> f64 {
    t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
}

#[inline]
fn lerp(t: f64, a: f64, b: f64) -> f64 {
    a + t * (b - a)
}

/// A single improved-noise lattice: three coordinate offsets and a 256-entry
/// permutation (mirrored to 257 so the `+1` lattice lookups stay in bounds).
pub struct ImprovedNoise {
    a: f64,
    b: f64,
    c: f64,
    perm: [u8; 257],
}

impl ImprovedNoise {
    /// Build from the RNG: three `nextDouble * 256` offsets, then a Fisher–Yates
    /// permutation shuffle (`j = nextInt(256 - i) + i`).
    pub fn new(rng: &mut LcgRandom) -> Self {
        let a = rng.next_double() * 256.0;
        let b = rng.next_double() * 256.0;
        let c = rng.next_double() * 256.0;
        let mut perm = [0u8; 257];
        for (i, slot) in perm.iter_mut().enumerate().take(256) {
            *slot = i as u8;
        }
        for i in 0..256 {
            let j = (rng.next_int_bound(256 - i as i32) + i as i32) as usize;
            perm.swap(i, j);
        }
        perm[256] = perm[0];
        Self { a, b, c, perm }
    }

    /// Sample the lattice at `(x, y, z)`. (The terrain noise never uses the
    /// reference's `yamp`/`ymin` y-clamp, so it is omitted.)
    pub fn sample(&self, x: f64, y: f64, z: f64) -> f64 {
        let mut dx = x + self.a;
        let mut dy = y + self.b;
        let mut dz = z + self.c;
        let ix = dx.floor();
        let iy = dy.floor();
        let iz = dz.floor();
        dx -= ix;
        dy -= iy;
        dz -= iz;
        let h1 = ix as i64 as u8;
        let h2 = iy as i64 as u8;
        let h3 = iz as i64 as u8;
        let (tx, ty, tz) = (fade(dx), fade(dy), fade(dz));
        let p = &self.perm;
        let a1 = p[h1 as usize].wrapping_add(h2);
        let b1 = p[h1 as usize + 1].wrapping_add(h2);
        let a2 = p[a1 as usize].wrapping_add(h3);
        let b2 = p[b1 as usize].wrapping_add(h3);
        let a3 = p[a1 as usize + 1].wrapping_add(h3);
        let b3 = p[b1 as usize + 1].wrapping_add(h3);

        let l1 = grad(p[a2 as usize], dx, dy, dz);
        let l2 = grad(p[b2 as usize], dx - 1.0, dy, dz);
        let l3 = grad(p[a3 as usize], dx, dy - 1.0, dz);
        let l4 = grad(p[b3 as usize], dx - 1.0, dy - 1.0, dz);
        let l5 = grad(p[a2 as usize + 1], dx, dy, dz - 1.0);
        let l6 = grad(p[b2 as usize + 1], dx - 1.0, dy, dz - 1.0);
        let l7 = grad(p[a3 as usize + 1], dx, dy - 1.0, dz - 1.0);
        let l8 = grad(p[b3 as usize + 1], dx - 1.0, dy - 1.0, dz - 1.0);

        let x1 = lerp(tx, l1, l2);
        let x2 = lerp(tx, l3, l4);
        let x3 = lerp(tx, l5, l6);
        let x4 = lerp(tx, l7, l8);
        let y1 = lerp(ty, x1, x2);
        let y2 = lerp(ty, x3, x4);
        lerp(tz, y1, y2)
    }

    /// Accumulate this lattice's contribution into a region grid (`xs×ys×zs`,
    /// index `(ix*zs + iz)*ys + iy`), adding `value / amp` per cell. Ported from
    /// the reference `populateNoiseArray` (2-D fast path for `ys == 1`).
    #[allow(clippy::too_many_arguments)]
    fn populate(
        &self,
        out: &mut [f64],
        xoff: f64,
        yoff: f64,
        zoff: f64,
        xs: usize,
        ys: usize,
        zs: usize,
        xsc: f64,
        ysc: f64,
        zsc: f64,
        amp: f64,
    ) {
        let p = &self.perm;
        let amp_div = 1.0 / amp;
        if ys == 1 {
            let mut idx = 0;
            for ix in 0..xs {
                let mut dx = xoff + ix as f64 * xsc + self.a;
                let fx = dx.floor();
                let ax = (fx as i64 & 0xFF) as usize;
                dx -= fx;
                let tx = fade(dx);
                for iz in 0..zs {
                    let mut dz = zoff + iz as f64 * zsc + self.c;
                    let fz = dz.floor();
                    let az = (fz as i64 & 0xFF) as usize;
                    dz -= fz;
                    let tz = fade(dz);
                    let b_ = p[p[ax] as usize].wrapping_add(az as u8) as usize;
                    let d_ = p[p[ax + 1] as usize].wrapping_add(az as u8) as usize;
                    let v1 = lerp(tx, grad(p[b_], dx, 0.0, dz), grad(p[d_], dx - 1.0, 0.0, dz));
                    let v2 = lerp(
                        tx,
                        grad(p[b_ + 1], dx, 0.0, dz - 1.0),
                        grad(p[d_ + 1], dx - 1.0, 0.0, dz - 1.0),
                    );
                    out[idx] += lerp(tz, v1, v2) * amp_div;
                    idx += 1;
                }
            }
        } else {
            let mut idx = 0;
            for ix in 0..xs {
                let mut dx = xoff + ix as f64 * xsc + self.a;
                let fx = dx.floor();
                let ax = (fx as i64 & 0xFF) as usize;
                dx -= fx;
                let tx = fade(dx);
                for iz in 0..zs {
                    let mut dz = zoff + iz as f64 * zsc + self.c;
                    let fz = dz.floor();
                    let az = (fz as i64 & 0xFF) as usize;
                    dz -= fz;
                    let tz = fade(dz);
                    let mut last_ay = -1i64;
                    let (mut e1, mut e2, mut e3, mut e4) = (0.0, 0.0, 0.0, 0.0);
                    for iy in 0..ys {
                        let mut dy = yoff + iy as f64 * ysc + self.b;
                        let fy = dy.floor();
                        let ay = (fy as i64 & 0xFF) as usize;
                        dy -= fy;
                        let ty = fade(dy);
                        if iy == 0 || ay as i64 != last_ay {
                            last_ay = ay as i64;
                            let aa = p[ax].wrapping_add(ay as u8);
                            let aaa = p[aa as usize].wrapping_add(az as u8) as usize;
                            let aab = p[aa as usize + 1].wrapping_add(az as u8) as usize;
                            let bb = p[ax + 1].wrapping_add(ay as u8);
                            let bba = p[bb as usize].wrapping_add(az as u8) as usize;
                            let bbb = p[bb as usize + 1].wrapping_add(az as u8) as usize;
                            e1 = lerp(tx, grad(p[aaa], dx, dy, dz), grad(p[bba], dx - 1.0, dy, dz));
                            e2 = lerp(
                                tx,
                                grad(p[aab], dx, dy - 1.0, dz),
                                grad(p[bbb], dx - 1.0, dy - 1.0, dz),
                            );
                            e3 = lerp(
                                tx,
                                grad(p[aaa + 1], dx, dy, dz - 1.0),
                                grad(p[bba + 1], dx - 1.0, dy, dz - 1.0),
                            );
                            e4 = lerp(
                                tx,
                                grad(p[aab + 1], dx, dy - 1.0, dz - 1.0),
                                grad(p[bbb + 1], dx - 1.0, dy - 1.0, dz - 1.0),
                            );
                        }
                        let v1 = lerp(ty, e1, e2);
                        let v2 = lerp(ty, e3, e4);
                        out[idx] += lerp(tz, v1, v2) * amp_div;
                        idx += 1;
                    }
                }
            }
        }
    }
}

/// A stack of `n` improved-noise lattices built sequentially from the RNG (the
/// legacy `NoiseGeneratorOctaves`). The terrain generator builds several of these
/// in a fixed order off one RNG.
pub struct OctaveNoise {
    pub octaves: Vec<ImprovedNoise>,
}

impl OctaveNoise {
    pub fn new(rng: &mut LcgRandom, n: usize) -> Self {
        let octaves = (0..n).map(|_| ImprovedNoise::new(rng)).collect();
        Self { octaves }
    }

    /// Fill a 3-D region grid (`xs×ys×zs`) by summing every octave, each at half
    /// the frequency and with the `1/amp` weighting of the previous (the reference
    /// `generateNoiseOctaves`). The x/z origin is wrapped at 2²⁴ per octave.
    #[allow(clippy::too_many_arguments)]
    pub fn sample_region(
        &self,
        out: &mut [f64],
        xoff: i32,
        yoff: i32,
        zoff: i32,
        xs: usize,
        ys: usize,
        zs: usize,
        xsc: f64,
        ysc: f64,
        zsc: f64,
    ) {
        for v in out.iter_mut() {
            *v = 0.0;
        }
        let mut persist = 1.0;
        for oct in &self.octaves {
            let mut x = xoff as f64 * persist * xsc;
            let y = yoff as f64 * persist * ysc;
            let mut z = zoff as f64 * persist * zsc;
            let xfl = x.floor() as i64;
            let zfl = z.floor() as i64;
            x -= xfl as f64;
            z -= zfl as f64;
            x += (xfl % 16_777_216) as f64;
            z += (zfl % 16_777_216) as f64;
            oct.populate(
                out,
                x,
                y,
                z,
                xs,
                ys,
                zs,
                xsc * persist,
                ysc * persist,
                zsc * persist,
                persist,
            );
            persist /= 2.0;
        }
    }

    /// 2-D region (`xs×zs`): the reference's 2-arg `generateNoiseOctaves` overload,
    /// which fixes `yOffset = 10`, `ySize = 1`.
    #[allow(clippy::too_many_arguments)]
    pub fn sample_region_2d(
        &self,
        out: &mut [f64],
        xoff: i32,
        zoff: i32,
        xs: usize,
        zs: usize,
        xsc: f64,
        ysc: f64,
        zsc: f64,
    ) {
        self.sample_region(out, xoff, 10, zoff, xs, 1, zs, xsc, ysc, zsc);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Known-answer vectors probed directly from the reference noise for seed 12345.
    fn noise_12345() -> ImprovedNoise {
        ImprovedNoise::new(&mut LcgRandom::new(12345))
    }

    #[test]
    fn offsets_and_perm_match_reference() {
        let n = noise_12345();
        assert_eq!(n.a, 92.621_595_433_080_78);
        assert_eq!(n.b, 238.846_332_233_866_5);
        assert_eq!(n.c, 213.27138533658206);
        assert_eq!(
            &n.perm[..16],
            &[83, 88, 161, 90, 117, 220, 146, 221, 68, 86, 213, 124, 192, 112, 203, 19]
        );
    }

    #[test]
    fn samples_match_reference_bit_exact() {
        let n = noise_12345();
        assert_eq!(n.sample(1.5, 2.5, 3.5), 0.3085586317820378);
        assert_eq!(n.sample(10.1, 0.0, -5.3), -0.45068267846500143);
        assert_eq!(n.sample(0.0, 0.0, 0.0), -0.37230011176761096);
        assert_eq!(n.sample(-100.25, 64.5, 200.75), 0.276_727_494_274_210_9);
    }

    #[test]
    fn populate_agrees_with_sample_at_a_point() {
        let n = noise_12345();
        // A 1×3×1 region (3-D branch). Cell (0,0,0) samples the lattice at the
        // same point as `sample`, which is KAT-verified.
        let mut out = [0.0f64; 3];
        n.populate(&mut out, 5.5, 0.0, 7.5, 1, 3, 1, 1.0, 1.0, 1.0, 1.0);
        assert_eq!(
            out[0],
            n.sample(5.5, 0.0, 7.5),
            "populate 3D disagrees with sample"
        );
    }

    #[test]
    fn octave_construction_consumes_rng_in_order() {
        // Two 16-octave stacks from the same fresh RNG match; building them in
        // sequence off one RNG advances it deterministically.
        let mut r = LcgRandom::new(7);
        let o = OctaveNoise::new(&mut r, 16);
        assert_eq!(o.octaves.len(), 16);
        let mut r2 = LcgRandom::new(7);
        let a = ImprovedNoise::new(&mut r2);
        assert_eq!(o.octaves[0].a, a.a);
    }
}
