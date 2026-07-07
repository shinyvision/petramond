//! The four-way horizontal facing shared by every oriented block-entity and
//! block state: chest/furnace fronts, doors, stairs, slab uprights, model
//! blocks. A neutral leaf module — nothing block-specific belongs here.

#[repr(u8)]
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum Facing {
    #[default]
    North = 0, // front faces -Z
    South = 1, // +Z
    West = 2,  // -X
    East = 3,  // +X
}

impl Facing {
    #[inline]
    pub fn to_u8(self) -> u8 {
        self as u8
    }
    #[inline]
    pub fn from_u8(v: u8) -> Self {
        match v {
            1 => Facing::South,
            2 => Facing::West,
            3 => Facing::East,
            _ => Facing::North,
        }
    }
}
