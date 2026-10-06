//! A spawn post's marker: the cell data the camp generator writes on a post's floor cell and the
//! skeleton garrison reads back. Bytes: `[role, facing]`, then the box of the camp the post
//! belongs to (min and max corners, inclusive, each axis an `i32` little-endian).

use std::f32::consts::TAU;

/// The cell data key a marker is stored under.
pub const KEY: &str = "monsters:camp_post";

/// The facing byte of a post whose guard may face any way.
const ANY_WAY: u8 = 0xFF;

const LEN: usize = 2 + 6 * 4;

/// What a guard on a post is for; the discriminant is the role's byte.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PostRole {
    Yard = 0,
    /// A tower top or a fortress wall walk.
    Watch = 1,
    /// Just inside a gate.
    Gate = 2,
    /// By the well, statue or arena.
    Centre = 3,
    /// By a hut's door, or inside it.
    Hut = 4,
}

impl PostRole {
    const ALL: [PostRole; 5] = [
        PostRole::Yard,
        PostRole::Watch,
        PostRole::Gate,
        PostRole::Centre,
        PostRole::Hut,
    ];

    pub fn code(self) -> u8 {
        self as u8
    }

    pub fn from_code(code: u8) -> Option<PostRole> {
        Self::ALL.into_iter().find(|r| r.code() == code)
    }
}

/// The way a post's guard faces, in 256ths of a turn the way mobs face: `(-sin yaw, -cos yaw)`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Facing(u8);

impl Facing {
    /// Facing along `(dx, dz)`; `None` for no direction at all. A bearing that would round to
    /// the any-way byte is nudged off it.
    pub fn along(dx: f32, dz: f32) -> Option<Facing> {
        if dx == 0.0 && dz == 0.0 {
            return None;
        }
        let turns = ((-dx).atan2(-dz) / TAU).rem_euclid(1.0);
        Some(Facing(
            ((turns * 256.0).round() as u32 % 256).min(u32::from(ANY_WAY) - 1) as u8,
        ))
    }

    /// The mob yaw, in radians.
    pub fn yaw(self) -> f32 {
        f32::from(self.0) / 256.0 * TAU
    }
}

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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PostMarker {
    pub role: PostRole,
    /// `None`: the guard may face any way.
    pub facing: Option<Facing>,
    pub camp: CampBox,
}

impl PostMarker {
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(LEN);
        out.extend([self.role.code(), self.facing.map_or(ANY_WAY, |f| f.0)]);
        for v in self.camp.min.into_iter().chain(self.camp.max) {
            out.extend(v.to_le_bytes());
        }
        out
    }

    /// `None` for anything but a whole marker of a known role.
    pub fn decode(bytes: &[u8]) -> Option<PostMarker> {
        if bytes.len() != LEN {
            return None;
        }
        let role = PostRole::from_code(bytes[0])?;
        let facing = (bytes[1] != ANY_WAY).then_some(Facing(bytes[1]));
        let mut v = bytes[2..]
            .chunks_exact(4)
            .map(|b| i32::from_le_bytes([b[0], b[1], b[2], b[3]]));
        let mut corner = || [0; 3].map(|_| v.next().unwrap_or(0));
        let (min, max) = (corner(), corner());
        Some(PostMarker {
            role,
            facing,
            camp: CampBox { min, max },
        })
    }

    pub fn watch(&self) -> bool {
        self.role == PostRole::Watch
    }
}
