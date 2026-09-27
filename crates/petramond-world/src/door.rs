use crate::block::Aabb;
use crate::facing::Facing;

pub const THICKNESS: f32 = 3.0 / 16.0;
const FAR: f32 = 1.0 - THICKNESS;

#[derive(Copy, Clone, Debug, PartialEq, Eq, Default)]
pub struct DoorState {
    pub facing: Facing,
    pub open: bool,
    pub top: bool,
}

impl crate::block::CellView for Option<DoorState> {
    fn owns(block: crate::block::Block) -> bool {
        crate::block::shape_kind_families::is_door(block)
    }
    fn from_cell(s: crate::block::ShapeState) -> Self {
        if s.is_empty() {
            return None;
        }
        Some(DoorState::decode(s.byte(0)))
    }
}

impl crate::block::CellView for DoorState {
    fn owns(block: crate::block::Block) -> bool {
        crate::block::shape_kind_families::is_door(block)
    }
    fn from_cell(s: crate::block::ShapeState) -> Self {
        DoorState::decode(s.byte(0))
    }
}
impl crate::block::CellCodec for DoorState {
    fn to_cell(&self) -> crate::block::ShapeState {
        crate::block::ShapeState::new(&[self.encode()])
    }
}

impl DoorState {
    #[inline]
    pub fn encode(self) -> u8 {
        self.facing.to_u8() | ((self.open as u8) << 2) | ((self.top as u8) << 3)
    }

    #[inline]
    pub fn decode(b: u8) -> DoorState {
        DoorState {
            facing: Facing::from_u8(b & 0b11),
            open: (b & 0b100) != 0,
            top: (b & 0b1000) != 0,
        }
    }
}

macro_rules! slab {
    (z, $lo:expr, $hi:expr) => {
        &[Aabb {
            min: [0.0, 0.0, $lo],
            max: [1.0, 1.0, $hi],
        }]
    };
    (x, $lo:expr, $hi:expr) => {
        &[Aabb {
            min: [$lo, 0.0, 0.0],
            max: [$hi, 1.0, 1.0],
        }]
    };
}

const NORTH_CLOSED: &[Aabb] = slab!(z, 0.0, THICKNESS);
const NORTH_OPEN: &[Aabb] = slab!(x, FAR, 1.0);
const SOUTH_CLOSED: &[Aabb] = slab!(z, FAR, 1.0);
const SOUTH_OPEN: &[Aabb] = slab!(x, 0.0, THICKNESS);
const WEST_CLOSED: &[Aabb] = slab!(x, 0.0, THICKNESS);
const WEST_OPEN: &[Aabb] = slab!(z, 0.0, THICKNESS);
const EAST_CLOSED: &[Aabb] = slab!(x, FAR, 1.0);
const EAST_OPEN: &[Aabb] = slab!(z, FAR, 1.0);

#[inline]
pub fn collision_boxes(state: DoorState) -> &'static [Aabb] {
    match (state.facing, state.open) {
        (Facing::North, false) => NORTH_CLOSED,
        (Facing::North, true) => NORTH_OPEN,
        (Facing::South, false) => SOUTH_CLOSED,
        (Facing::South, true) => SOUTH_OPEN,
        (Facing::West, false) => WEST_CLOSED,
        (Facing::West, true) => WEST_OPEN,
        (Facing::East, false) => EAST_CLOSED,
        (Facing::East, true) => EAST_OPEN,
    }
}

#[inline]
pub fn selection_aabb(state: DoorState) -> ([f32; 3], [f32; 3]) {
    let b = collision_boxes(state)[0];
    (b.min, b.max)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_byte_round_trips() {
        for &facing in &[Facing::North, Facing::South, Facing::West, Facing::East] {
            for &open in &[false, true] {
                for &top in &[false, true] {
                    let s = DoorState { facing, open, top };
                    assert_eq!(DoorState::decode(s.encode()), s);
                }
            }
        }
    }

    #[test]
    fn opening_swaps_the_collision_slab_to_the_adjacent_edge() {
        let thin_axis = |b: Aabb| {
            let dx = b.max[0] - b.min[0];
            let dz = b.max[2] - b.min[2];
            if dx < dz {
                0
            } else {
                2
            }
        };
        for &facing in &[Facing::North, Facing::South, Facing::West, Facing::East] {
            let closed = collision_boxes(DoorState {
                facing,
                open: false,
                top: false,
            })[0];
            let open = collision_boxes(DoorState {
                facing,
                open: true,
                top: false,
            })[0];
            assert_ne!(
                thin_axis(closed),
                thin_axis(open),
                "{facing:?}: opening must rotate the slab onto the perpendicular edge"
            );
        }
    }
}
