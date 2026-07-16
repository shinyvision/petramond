//! The four-way horizontal facing shared by every oriented block-entity and
//! block state: chest/furnace fronts, doors, stairs, slab uprights, model
//! blocks. A neutral leaf module — nothing block-specific belongs here.

use crate::mathh::IVec3;
use crate::wire_enum::wire_enum;

wire_enum! {
    pub enum Facing: u8 {
        North = 0, // front faces -Z
        South = 1, // +Z
        West = 2,  // -X
        East = 3,  // +X
    }
    default North
}

impl Facing {
    /// The horizontal unit direction this facing points toward.
    #[inline]
    pub fn dir(self) -> IVec3 {
        match self {
            Facing::North => IVec3::new(0, 0, -1),
            Facing::South => IVec3::new(0, 0, 1),
            Facing::West => IVec3::new(-1, 0, 0),
            Facing::East => IVec3::new(1, 0, 0),
        }
    }

    /// The facing whose [`dir`](Self::dir) equals `normal`. Only horizontal
    /// unit normals map; a vertical (`±Y`) or degenerate normal yields `None` —
    /// wall-mounted placements (ladders, wall torches) key off exactly that.
    #[inline]
    pub fn from_horizontal_normal(normal: IVec3) -> Option<Self> {
        match (normal.x, normal.y, normal.z) {
            (1, 0, 0) => Some(Facing::East),
            (-1, 0, 0) => Some(Facing::West),
            (0, 0, 1) => Some(Facing::South),
            (0, 0, -1) => Some(Facing::North),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn horizontal_normals_round_trip_and_vertical_normals_refuse() {
        for f in [Facing::North, Facing::South, Facing::West, Facing::East] {
            assert_eq!(Facing::from_horizontal_normal(f.dir()), Some(f));
        }
        assert_eq!(Facing::from_horizontal_normal(IVec3::new(0, 1, 0)), None);
        assert_eq!(Facing::from_horizontal_normal(IVec3::new(0, -1, 0)), None);
        assert_eq!(Facing::from_horizontal_normal(IVec3::ZERO), None);
    }
}
