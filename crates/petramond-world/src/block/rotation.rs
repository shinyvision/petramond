//! A quarter turn of a cell, dispatched through its owning shape family.
use super::{Block, CellCodec, CellView, ShapeState};
use crate::block_state::{EntityFront, LogAxis};
use crate::facing::Facing;

pub fn facing(f: Facing) -> Facing {
    match f {
        Facing::North => Facing::East,
        Facing::East => Facing::South,
        Facing::South => Facing::West,
        Facing::West => Facing::North,
    }
}

#[derive(Clone, Copy)]
pub struct CellRotation {
    pub block: Block,
    pub state: ShapeState,
    /// The negative and positive sub-cell parts exchange addresses.
    pub swap_parts: bool,
}

impl CellRotation {
    pub fn unchanged(block: Block, state: ShapeState) -> Self {
        Self {
            block,
            state,
            swap_parts: false,
        }
    }
}

pub fn common(block: Block, state: ShapeState) -> CellRotation {
    let mut out = CellRotation::unchanged(block, state);
    if block.is_log() {
        out.state = match LogAxis::from_cell(state) {
            LogAxis::X => LogAxis::Z,
            LogAxis::Z => LogAxis::X,
            other => other,
        }
        .to_cell();
    } else if block.directional_view() {
        let mut bytes = state.bytes().to_vec();
        if bytes.is_empty() {
            bytes.push(0);
        }
        bytes[0] = facing(EntityFront::from_cell(state).0).to_u8();
        out.state = ShapeState::with_ids(&bytes, state.id_mask());
    }
    out
}

pub fn connection(block: Block, state: ShapeState) -> CellRotation {
    use crate::connect::{EAST, NORTH, SOUTH, WEST};
    let mask = state.byte(0);
    let mut turned = 0;
    for (from, to) in [(WEST, NORTH), (NORTH, EAST), (EAST, SOUTH), (SOUTH, WEST)] {
        if mask & from != 0 {
            turned |= to;
        }
    }
    CellRotation::unchanged(block, ShapeState::new(&[turned]))
}
