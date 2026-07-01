//! Stair shape and orientation shared by placement, collision, selection, and meshing.
//!
//! A stair's `Facing` is the low/open side, matching the "front faces the player"
//! convention used by furnaces and chests on placement. The solid shape is two
//! non-overlapping boxes: a half-height tread on the low side plus a full-height
//! back half.

use crate::block::{Aabb, Block};
use crate::furnace::Facing;
use crate::mathh::{IVec3, Vec3};

pub const BOX_COUNT: usize = 2;
pub type StairBoxes = [Aabb; BOX_COUNT];
pub type WorldStairBoxes = [(Vec3, Vec3); BOX_COUNT];

const H: f32 = 0.5;

const NORTH_BOXES: StairBoxes = [
    Aabb {
        min: [0.0, 0.0, 0.0],
        max: [1.0, H, H],
    },
    Aabb {
        min: [0.0, 0.0, H],
        max: [1.0, 1.0, 1.0],
    },
];
const SOUTH_BOXES: StairBoxes = [
    Aabb {
        min: [0.0, 0.0, H],
        max: [1.0, H, 1.0],
    },
    Aabb {
        min: [0.0, 0.0, 0.0],
        max: [1.0, 1.0, H],
    },
];
const WEST_BOXES: StairBoxes = [
    Aabb {
        min: [0.0, 0.0, 0.0],
        max: [H, H, 1.0],
    },
    Aabb {
        min: [H, 0.0, 0.0],
        max: [1.0, 1.0, 1.0],
    },
];
const EAST_BOXES: StairBoxes = [
    Aabb {
        min: [H, 0.0, 0.0],
        max: [1.0, H, 1.0],
    },
    Aabb {
        min: [0.0, 0.0, 0.0],
        max: [H, 1.0, 1.0],
    },
];

#[inline]
pub fn boxes(facing: Facing) -> &'static StairBoxes {
    match facing {
        Facing::North => &NORTH_BOXES,
        Facing::South => &SOUTH_BOXES,
        Facing::West => &WEST_BOXES,
        Facing::East => &EAST_BOXES,
    }
}

#[inline]
pub fn world_boxes(origin: IVec3, facing: Facing) -> WorldStairBoxes {
    let base = Vec3::new(origin.x as f32, origin.y as f32, origin.z as f32);
    boxes(facing).map(|b| {
        (
            base + Vec3::new(b.min[0], b.min[1], b.min[2]),
            base + Vec3::new(b.max[0], b.max[1], b.max[2]),
        )
    })
}

#[inline]
pub fn is_stair(block: Block) -> bool {
    matches!(
        block,
        Block::OakStairs
            | Block::SpruceStairs
            | Block::BirchStairs
            | Block::JungleStairs
            | Block::AcaciaStairs
            | Block::DarkOakStairs
            | Block::CherryStairs
            | Block::MangroveStairs
            | Block::RedwoodStairs
            | Block::CobblestoneStairs
            | Block::StoneStairs
            | Block::DirtStairs
    )
}

/// A 2x2 mask of the open half-face quadrants on a stair boundary. The light flood
/// intersects the two cells' masks; light crosses only where their gaps overlap.
#[inline]
pub fn light_side_mask(facing: Facing, dx: i32, dy: i32, dz: i32) -> u8 {
    if dy < 0 {
        return 0;
    }
    if dy > 0 {
        return top_mask(facing);
    }
    let (fx, fz) = facing_xz(facing);
    if (dx, dz) == (-fx, -fz) {
        return 0;
    }
    if (dx, dz) == (fx, fz) {
        return UPPER_FULL;
    }
    match (dx, dz, facing) {
        (-1, 0, Facing::North) | (1, 0, Facing::North) => UPPER_U0,
        (-1, 0, Facing::South) | (1, 0, Facing::South) => UPPER_U1,
        (0, -1, Facing::West) | (0, 1, Facing::West) => UPPER_U0,
        (0, -1, Facing::East) | (0, 1, Facing::East) => UPPER_U1,
        _ => 0,
    }
}

#[inline]
fn facing_xz(facing: Facing) -> (i32, i32) {
    match facing {
        Facing::North => (0, -1),
        Facing::South => (0, 1),
        Facing::West => (-1, 0),
        Facing::East => (1, 0),
    }
}

const UPPER_FULL: u8 = 0b1100;
const UPPER_U0: u8 = 0b0100;
const UPPER_U1: u8 = 0b1000;

#[inline]
fn top_mask(facing: Facing) -> u8 {
    match facing {
        Facing::North => 0b0011,
        Facing::South => 0b1100,
        Facing::West => 0b0101,
        Facing::East => 0b1010,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stair_facings_stay_inside_one_cell() {
        for facing in [Facing::North, Facing::South, Facing::West, Facing::East] {
            for b in boxes(facing) {
                for axis in 0..3 {
                    assert!(b.min[axis] >= 0.0, "{facing:?} min {b:?}");
                    assert!(b.max[axis] <= 1.0, "{facing:?} max {b:?}");
                    assert!(b.min[axis] < b.max[axis], "{facing:?} span {b:?}");
                }
            }
        }
    }

    #[test]
    fn stair_light_masks_match_the_cut_out_gap() {
        assert_eq!(light_side_mask(Facing::East, -1, 0, 0), 0);
        assert_ne!(light_side_mask(Facing::East, 1, 0, 0), 0);
        assert_ne!(light_side_mask(Facing::East, 0, 1, 0), 0);
        assert_eq!(light_side_mask(Facing::East, 0, -1, 0), 0);

        let north_side_gap = light_side_mask(Facing::North, 1, 0, 0);
        let south_side_gap = light_side_mask(Facing::South, -1, 0, 0);
        assert_eq!(
            north_side_gap & south_side_gap,
            0,
            "two partial side gaps on opposite stair halves should not connect"
        );
    }
}
