use crate::block::{Block, BlockLightShape};
use crate::block_state::{SlabState, StairState};

use super::{nbhd_idx, NBHD_VOLUME};

pub(super) enum SparseCellState {
    Stair { idx: usize, state: StairState },
    Slab { idx: usize, state: SlabState },
}

#[derive(Default)]
pub(super) struct ShapeStateSnapshot {
    stair_states: Option<Box<[u8]>>,
    slab_states: Option<Box<[SlabState]>>,
}

impl ShapeStateSnapshot {
    pub(super) fn from_sparse(states: &[SparseCellState]) -> Self {
        let mut stair_states: Option<Box<[u8]>> = None;
        let mut slab_states: Option<Box<[SlabState]>> = None;
        for state in states {
            match *state {
                SparseCellState::Stair { idx, state } => {
                    if idx >= NBHD_VOLUME {
                        continue;
                    }
                    let states = stair_states.get_or_insert_with(|| {
                        vec![StairState::default().encode(); NBHD_VOLUME].into_boxed_slice()
                    });
                    states[idx] = state.encode();
                }
                SparseCellState::Slab { idx, state } => {
                    if idx >= NBHD_VOLUME {
                        continue;
                    }
                    let states = slab_states.get_or_insert_with(|| {
                        vec![SlabState::EMPTY; NBHD_VOLUME].into_boxed_slice()
                    });
                    states[idx] = state;
                }
            }
        }
        Self {
            stair_states,
            slab_states,
        }
    }

    fn stair_state(&self, idx: usize) -> StairState {
        self.stair_states
            .as_ref()
            .and_then(|f| f.get(idx).copied())
            .map(StairState::decode)
            .unwrap_or_default()
    }

    fn slab_state(&self, idx: usize, block: Block) -> SlabState {
        self.slab_states
            .as_ref()
            .and_then(|f| f.get(idx).copied())
            .map(|state| crate::slab::normalize_state(block, state))
            .unwrap_or_else(|| crate::slab::default_state(block))
    }
}

#[derive(Copy, Clone)]
pub(super) struct LightCells<'a> {
    blocks: &'a [u8],
    states: &'a ShapeStateSnapshot,
}

impl<'a> LightCells<'a> {
    pub(super) fn new(blocks: &'a [u8], states: &'a ShapeStateSnapshot) -> Self {
        Self { blocks, states }
    }

    pub(super) fn can_cross(
        self,
        from: (usize, usize, usize),
        to: (usize, usize, usize),
        dir: (i32, i32, i32),
    ) -> bool {
        let fi = nbhd_idx(from.0, from.1, from.2);
        let ti = nbhd_idx(to.0, to.1, to.2);
        let from_mask = self.side_aperture(fi, dir);
        let to_mask = self.side_aperture(ti, (-dir.0, -dir.1, -dir.2));
        from_mask & to_mask != 0
    }

    fn side_aperture(self, idx: usize, dir: (i32, i32, i32)) -> u8 {
        let block = Block::from_id(self.blocks[idx]);
        match block.light_shape() {
            BlockLightShape::OpaqueCube => 0,
            BlockLightShape::Open => 0b1111,
            BlockLightShape::Stair => {
                crate::stair::light_side_mask(self.states.stair_state(idx), dir.0, dir.1, dir.2)
            }
            BlockLightShape::Slab => crate::slab::light_side_mask(
                self.states.slab_state(idx, block),
                dir.0,
                dir.1,
                dir.2,
            ),
        }
    }
}
