use crate::block::Aabb;
use crate::door::THICKNESS;
use crate::facing::Facing;

const FAR: f32 = 1.0 - THICKNESS;

#[derive(Copy, Clone, Debug, PartialEq, Eq, Default)]
pub struct TrapdoorState {
    pub facing: Facing,
    pub open: bool,
    pub top: bool,
}

impl crate::block::CellView for Option<TrapdoorState> {
    fn owns(block: crate::block::Block) -> bool {
        crate::block::shape_kind_families::is_trapdoor(block)
    }
    fn from_cell(s: crate::block::ShapeState) -> Self {
        if s.is_empty() {
            return None;
        }
        Some(TrapdoorState::decode(s.byte(0)))
    }
}

impl crate::block::CellView for TrapdoorState {
    fn owns(block: crate::block::Block) -> bool {
        crate::block::shape_kind_families::is_trapdoor(block)
    }
    fn from_cell(s: crate::block::ShapeState) -> Self {
        TrapdoorState::decode(s.byte(0))
    }
}
impl crate::block::CellCodec for TrapdoorState {
    fn to_cell(&self) -> crate::block::ShapeState {
        crate::block::ShapeState::new(&[self.encode()])
    }
}

impl TrapdoorState {
    #[inline]
    pub fn encode(self) -> u8 {
        self.facing.to_u8() | ((self.open as u8) << 2) | ((self.top as u8) << 3)
    }

    #[inline]
    pub fn decode(b: u8) -> TrapdoorState {
        TrapdoorState {
            facing: Facing::from_u8(b & 0b11),
            open: (b & 0b100) != 0,
            top: (b & 0b1000) != 0,
        }
    }
}

macro_rules! slab {
    (y, $lo:expr, $hi:expr) => {
        &[Aabb {
            min: [0.0, $lo, 0.0],
            max: [1.0, $hi, 1.0],
        }]
    };
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

const CLOSED_FLOOR: &[Aabb] = slab!(y, 0.0, THICKNESS);
const CLOSED_CEILING: &[Aabb] = slab!(y, FAR, 1.0);
const OPEN_NORTH: &[Aabb] = slab!(z, 0.0, THICKNESS);
const OPEN_SOUTH: &[Aabb] = slab!(z, FAR, 1.0);
const OPEN_WEST: &[Aabb] = slab!(x, 0.0, THICKNESS);
const OPEN_EAST: &[Aabb] = slab!(x, FAR, 1.0);

#[inline]
pub fn collision_boxes(state: TrapdoorState) -> &'static [Aabb] {
    if !state.open {
        return if state.top {
            CLOSED_CEILING
        } else {
            CLOSED_FLOOR
        };
    }
    match state.facing {
        Facing::North => OPEN_NORTH,
        Facing::South => OPEN_SOUTH,
        Facing::West => OPEN_WEST,
        Facing::East => OPEN_EAST,
    }
}

#[inline]
pub fn selection_aabb(state: TrapdoorState) -> ([f32; 3], [f32; 3]) {
    let b = collision_boxes(state)[0];
    (b.min, b.max)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FACINGS: [Facing; 4] = [Facing::North, Facing::South, Facing::West, Facing::East];

    #[test]
    fn state_byte_round_trips() {
        for &facing in &FACINGS {
            for &open in &[false, true] {
                for &top in &[false, true] {
                    let s = TrapdoorState { facing, open, top };
                    assert_eq!(TrapdoorState::decode(s.encode()), s);
                }
            }
        }
    }

    #[test]
    fn an_open_panel_stands_on_the_edge_it_is_hinged_to() {
        for &facing in &FACINGS {
            for &top in &[false, true] {
                let open = collision_boxes(TrapdoorState {
                    facing,
                    open: true,
                    top,
                })[0];
                assert!(
                    (open.max[1] - open.min[1] - 1.0).abs() < 1e-5,
                    "{facing:?}: an open panel is full height"
                );
                let (axis, near) = match facing {
                    Facing::North => (2, true),
                    Facing::South => (2, false),
                    Facing::West => (0, true),
                    Facing::East => (0, false),
                };
                let (lo, hi) = (open.min[axis], open.max[axis]);
                assert!(hi - lo < 0.5, "{facing:?}: the open panel is thin");
                assert!(
                    if near { lo < 1e-5 } else { hi > 1.0 - 1e-5 },
                    "{facing:?}: the open panel stands on its hinged edge"
                );
            }
        }
    }
}
