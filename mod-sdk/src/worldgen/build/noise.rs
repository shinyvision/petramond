fn hash(seed: u32, x: i32, z: i32) -> u32 {
    let mut h = x
        .wrapping_mul(374_761_393)
        .wrapping_add(z.wrapping_mul(668_265_263))
        .wrapping_add(seed as i32) as u32;
    h = (h ^ (h >> 13)).wrapping_mul(1_274_126_177);
    h ^ (h >> 16)
}

fn lattice(seed: u32, x: i32, z: i32) -> f32 {
    // `u32::MAX as f32` is 2^32, so this is exactly `/ u32::MAX as f32`.
    hash(seed, x, z) as f32 * (1.0 / 4_294_967_296.0)
}

#[inline]
fn fade(t: f32) -> f32 {
    t * t * (3.0 - 2.0 * t)
}

#[inline]
fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// [`fade`] in 16.16 fixed point, `t` below 1.
#[inline]
fn fade_fixed(t: u32) -> u32 {
    let t2 = (t * t) >> 16;
    ((u64::from(t2) * u64::from((3 << 16) - 2 * t)) >> 16) as u32
}

/// [`lerp`] in 16.16 fixed point.
#[inline]
fn lerp_fixed(a: u32, b: u32, t: u32) -> u32 {
    (i64::from(a) + (((i64::from(b) - i64::from(a)) * i64::from(t)) >> 16)) as u32
}

/// Smooth value noise in [-1, 1], a pure function of its seed and position. Past splitting off
/// the cell it works in 16.16 fixed point: a guest pays for every float operation twice, the
/// host canonicalizing each result.
#[derive(Clone, Copy, Debug)]
pub struct Noise2(pub u32);

impl Noise2 {
    #[inline]
    pub fn at(self, x: f32, z: f32) -> f32 {
        let (xf, zf) = (x.floor(), z.floor());
        let (xi, zi) = (xf as i32, zf as i32);
        let (u, v) = (
            fade_fixed(((x - xf) * 65536.0) as u32),
            fade_fixed(((z - zf) * 65536.0) as u32),
        );
        let c = |dx: i32, dz: i32| hash(self.0, xi.wrapping_add(dx), zi.wrapping_add(dz)) >> 16;
        let near = lerp_fixed(c(0, 0), c(1, 0), u);
        let far = lerp_fixed(c(0, 1), c(1, 1), u);
        lerp_fixed(near, far, v) as f32 * (2.0 / 65535.0) - 1.0
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Noise3(pub u32);

impl Noise3 {
    #[inline]
    pub fn at(self, x: f32, y: f32, z: f32) -> f32 {
        let cell = [x.floor() as i32, y.floor() as i32, z.floor() as i32];
        Self::blend(self.corners(cell), [x, y, z], cell)
    }

    #[inline]
    fn corners(self, [xi, yi, zi]: [i32; 3]) -> [f32; 8] {
        let c = |dx: i32, dy: i32, dz: i32| {
            let y = yi + dy;
            lattice(
                self.0,
                xi + dx + y.wrapping_mul(7919),
                zi + dz - y.wrapping_mul(3571),
            )
        };
        [
            c(0, 0, 0),
            c(1, 0, 0),
            c(0, 0, 1),
            c(1, 0, 1),
            c(0, 1, 0),
            c(1, 1, 0),
            c(0, 1, 1),
            c(1, 1, 1),
        ]
    }

    #[inline]
    fn blend(c: [f32; 8], [x, y, z]: [f32; 3], [xi, yi, zi]: [i32; 3]) -> f32 {
        let (u, v, w) = (
            fade(x - xi as f32),
            fade(y - yi as f32),
            fade(z - zi as f32),
        );
        let plane = |p: usize| lerp(lerp(c[p], c[p + 1], u), lerp(c[p + 2], c[p + 3], u), w);
        lerp(plane(0), plane(4), v) * 2.0 - 1.0
    }
}
