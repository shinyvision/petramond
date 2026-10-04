//! A spawn post's cell data: `[role, yaw]`, then the box of the camp the post belongs to (min and
//! max corners, inclusive, each axis an `i32` little-endian).

/// The cells a camp stands in, inclusive.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CampBox {
    pub min: [i32; 3],
    pub max: [i32; 3],
}

impl CampBox {
    pub fn contains(&self, c: [i32; 3]) -> bool {
        (0..3).all(|a| c[a] >= self.min[a] && c[a] <= self.max[a])
    }
}

const LEN: usize = 2 + 6 * 4;

pub fn encode(role: u8, yaw: u8, camp: CampBox) -> Vec<u8> {
    let mut out = Vec::with_capacity(LEN);
    out.extend([role, yaw]);
    for v in camp.min.into_iter().chain(camp.max) {
        out.extend(v.to_le_bytes());
    }
    out
}

/// `(role, yaw, camp)`.
pub fn decode(bytes: &[u8]) -> Option<(u8, u8, CampBox)> {
    if bytes.len() != LEN {
        return None;
    }
    let mut v = bytes[2..]
        .chunks_exact(4)
        .map(|b| i32::from_le_bytes([b[0], b[1], b[2], b[3]]));
    let mut corner = || [0; 3].map(|_| v.next().unwrap_or(0));
    let (min, max) = (corner(), corner());
    Some((bytes[0], bytes[1], CampBox { min, max }))
}
