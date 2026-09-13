//! Fixed-permutation simplex fields for reproducible spatial recipes.

mod cellular;
pub use cellular::{cellular2, Cell2};

const PERM: [u8; 256] = [
    151, 160, 137, 91, 90, 15, 131, 13, 201, 95, 96, 53, 194, 233, 7, 225, 140, 36, 103, 30, 69,
    142, 8, 99, 37, 240, 21, 10, 23, 190, 6, 148, 247, 120, 234, 75, 0, 26, 197, 62, 94, 252, 219,
    203, 117, 35, 11, 32, 57, 177, 33, 88, 237, 149, 56, 87, 174, 20, 125, 136, 171, 168, 68, 175,
    74, 165, 71, 134, 139, 48, 27, 166, 77, 146, 158, 231, 83, 111, 229, 122, 60, 211, 133, 230,
    220, 105, 92, 41, 55, 46, 245, 40, 244, 102, 143, 54, 65, 25, 63, 161, 1, 216, 80, 73, 209, 76,
    132, 187, 208, 89, 18, 169, 200, 196, 135, 130, 116, 188, 159, 86, 164, 100, 109, 198, 173,
    186, 3, 64, 52, 217, 226, 250, 124, 123, 5, 202, 38, 147, 118, 126, 255, 82, 85, 212, 207, 206,
    59, 227, 47, 16, 58, 17, 182, 189, 28, 42, 223, 183, 170, 213, 119, 248, 152, 2, 44, 154, 163,
    70, 221, 153, 101, 155, 167, 43, 172, 9, 129, 22, 39, 253, 19, 98, 108, 110, 79, 113, 224, 232,
    178, 185, 112, 104, 218, 246, 97, 228, 251, 34, 242, 193, 238, 210, 144, 12, 191, 179, 162,
    241, 81, 51, 145, 235, 249, 14, 239, 107, 49, 192, 214, 31, 181, 199, 106, 157, 184, 84, 204,
    176, 115, 121, 50, 45, 127, 4, 150, 254, 138, 236, 205, 93, 222, 114, 67, 29, 24, 72, 243, 141,
    128, 195, 78, 66, 215, 61, 156, 180,
];

const GRADIENTS: [[f64; 3]; 12] = [
    [1., 1., 0.],
    [-1., 1., 0.],
    [1., -1., 0.],
    [-1., -1., 0.],
    [1., 0., 1.],
    [-1., 0., 1.],
    [1., 0., -1.],
    [-1., 0., -1.],
    [0., 1., 1.],
    [0., -1., 1.],
    [0., 1., -1.],
    [0., -1., -1.],
];

#[inline]
fn perm(i: i32) -> i32 {
    i32::from(PERM[(i & 255) as usize])
}

#[inline]
fn contribution(p: [f64; 3], gradient: i32, radius: f64) -> f64 {
    let t = radius - p[0] * p[0] - p[1] * p[1] - p[2] * p[2];
    if t < 0.0 {
        return 0.0;
    }
    let g = GRADIENTS[gradient as usize % 12];
    let square = t * t;
    square * square * (g[0] * p[0] + g[1] * p[1] + g[2] * p[2])
}

/// Two-dimensional simplex noise with the standard fixed permutation.
pub fn simplex2(x: f64, z: f64) -> f64 {
    simplex2_permutation(&PERM, x, z)
}

/// Two-dimensional simplex noise with a caller-supplied permutation.
pub fn simplex2_permutation(permutation: &[u8; 256], x: f64, z: f64) -> f64 {
    let perm = |i: i32| i32::from(permutation[(i & 255) as usize]);
    let skew = (x + z) * (0.5 * (3.0_f64.sqrt() - 1.0));
    let cell = [(x + skew).floor() as i32, (z + skew).floor() as i32];
    let unskew = (3.0 - 3.0_f64.sqrt()) / 6.0;
    let shift = (cell[0] + cell[1]) as f64 * unskew;
    let local = [x - (cell[0] as f64 - shift), z - (cell[1] as f64 - shift)];
    let middle = if local[0] > local[1] { [1, 0] } else { [0, 1] };
    let mut sum = 0.0;
    for (index, offset) in [[0, 0], middle, [1, 1]].into_iter().enumerate() {
        let p = [
            local[0] - offset[0] as f64 + index as f64 * unskew,
            local[1] - offset[1] as f64 + index as f64 * unskew,
            0.0,
        ];
        let gradient = perm(cell[0] + offset[0] + perm(cell[1] + offset[1]));
        sum += contribution(p, gradient, 0.5);
    }
    70.0 * sum
}

/// Three-dimensional simplex noise with the standard fixed permutation.
pub fn simplex3(p: [f64; 3]) -> f64 {
    let skew = (p[0] + p[1] + p[2]) / 3.0;
    let cell = p.map(|v| (v + skew).floor() as i32);
    let shift = (cell[0] + cell[1] + cell[2]) as f64 / 6.0;
    let local: [f64; 3] = std::array::from_fn(|a| p[a] - (cell[a] as f64 - shift));
    let mut rank = [0; 3];
    for a in 0..3 {
        for b in a + 1..3 {
            if local[a] >= local[b] {
                rank[a] += 1;
            } else {
                rank[b] += 1;
            }
        }
    }
    let mut sum = 0.0;
    for corner in 0..4 {
        let offset = rank.map(|r| i32::from(r >= 3 - corner));
        let delta = std::array::from_fn(|a| local[a] - offset[a] as f64 + corner as f64 / 6.0);
        let gradient =
            perm(cell[0] + offset[0] + perm(cell[1] + offset[1] + perm(cell[2] + offset[2])));
        sum += contribution(delta, gradient, 0.6);
    }
    32.0 * sum
}

/// Spatial sampling in block coordinates, offset by one period.
pub fn scaled_simplex2(x: f64, z: f64, scale: f64) -> f64 {
    simplex2((x + scale) / scale, (z + scale) / scale)
}

/// Spatial sampling in block coordinates, offset by one period.
pub fn scaled_simplex3(p: [f64; 3], scale: f64) -> f64 {
    simplex3(p.map(|v| (v + scale) / scale))
}

#[cfg(test)]
mod tests;
