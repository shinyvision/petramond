//! Euclidean cellular noise on a jittered square lattice.

mod vectors;

#[derive(Clone, Copy, Debug)]
pub struct Cell2 {
    pub center: [f64; 2],
    pub distance: f64,
    pub hash: i32,
}

/// Nearest site, with `jitter` in `[0, 1]`. Signed hashes also provide an
/// independent, uniformly distributed label for each site.
pub fn cellular2(seed: u32, point: [f64; 2], jitter: f64) -> Cell2 {
    let origin = point.map(|v| (v + 0.5).floor() as i32);
    let amplitude = f64::from(0.437_015_95_f32) * jitter;
    let mut nearest = Cell2 {
        center: [0.0; 2],
        distance: f64::INFINITY,
        hash: 0,
    };
    for dx in -1..=1 {
        for dz in -1..=1 {
            let x = origin[0].wrapping_add(dx);
            let z = origin[1].wrapping_add(dz);
            let hash =
                ((seed as i32) ^ x.wrapping_mul(501_125_321) ^ z.wrapping_mul(1_720_413_743))
                    .wrapping_mul(0x27d4_eb2d);
            let direction = vectors::DIRECTIONS[((hash as u32 >> 1) & 255) as usize];
            let offset = direction.map(|v| f64::from(v) * amplitude);
            let delta = [
                f64::from(x) - point[0] + offset[0],
                f64::from(z) - point[1] + offset[1],
            ];
            let distance = delta[0] * delta[0] + delta[1] * delta[1];
            if distance < nearest.distance {
                nearest = Cell2 {
                    center: [f64::from(x) + offset[0], f64::from(z) + offset[1]],
                    distance,
                    hash,
                };
            }
        }
    }
    nearest.distance = nearest.distance.sqrt();
    nearest
}
