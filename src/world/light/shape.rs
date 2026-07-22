use crate::block::{Block, BlockLightShape};
use crate::block_state::{SlabState, StairState};

pub(super) enum SparseCellState {
    Stair { idx: usize, state: StairState },
    Slab { idx: usize, state: SlabState },
    /// A Layer-3 custom shape's baked light opacity at `idx` (true = blocks
    /// light like a full cube).
    CustomAperture { idx: usize, opaque: bool },
}

#[derive(Default)]
pub(super) struct ShapeStateSnapshot {
    stair_states: Option<Box<[u8]>>,
    slab_states: Option<Box<[SlabState]>>,
    /// Per-cell custom-aperture opacity (true = opaque); absent cells pass light.
    custom_apertures: Option<Box<[bool]>>,
}

impl ShapeStateSnapshot {
    /// `volume` is the flood cube's cell count (48³ for a per-section bake, 64³ for
    /// a 2×2×2 batch bake); sparse indices are already in that cube's coordinates.
    pub(super) fn from_sparse(states: &[SparseCellState], volume: usize) -> Self {
        let mut stair_states: Option<Box<[u8]>> = None;
        let mut slab_states: Option<Box<[SlabState]>> = None;
        let mut custom_apertures: Option<Box<[bool]>> = None;
        for state in states {
            match *state {
                SparseCellState::Stair { idx, state } => {
                    if idx >= volume {
                        continue;
                    }
                    let states = stair_states.get_or_insert_with(|| {
                        vec![StairState::default().encode(); volume].into_boxed_slice()
                    });
                    states[idx] = state.encode();
                }
                SparseCellState::Slab { idx, state } => {
                    if idx >= volume {
                        continue;
                    }
                    let states = slab_states
                        .get_or_insert_with(|| vec![SlabState::EMPTY; volume].into_boxed_slice());
                    states[idx] = state;
                }
                SparseCellState::CustomAperture { idx, opaque } => {
                    if idx >= volume {
                        continue;
                    }
                    let states = custom_apertures
                        .get_or_insert_with(|| vec![false; volume].into_boxed_slice());
                    states[idx] = opaque;
                }
            }
        }
        Self {
            stair_states,
            slab_states,
            custom_apertures,
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

    fn custom_aperture_opaque(&self, idx: usize) -> bool {
        self.custom_apertures
            .as_ref()
            .and_then(|f| f.get(idx).copied())
            .unwrap_or(false)
    }
}

#[derive(Copy, Clone)]
pub(super) struct LightCells<'a> {
    blocks: &'a [u8],
    states: &'a ShapeStateSnapshot,
    /// Cube side length in cells (48 per-section, 64 for a 2×2×2 batch).
    dim: usize,
}

impl<'a> LightCells<'a> {
    pub(super) fn new(blocks: &'a [u8], states: &'a ShapeStateSnapshot, dim: usize) -> Self {
        debug_assert_eq!(blocks.len(), dim * dim * dim);
        Self {
            blocks,
            states,
            dim,
        }
    }

    #[inline]
    fn idx(self, x: usize, y: usize, z: usize) -> usize {
        (y * self.dim + z) * self.dim + x
    }

    pub(super) fn can_cross(
        self,
        from: (usize, usize, usize),
        to: (usize, usize, usize),
        dir: (i32, i32, i32),
    ) -> bool {
        let fi = self.idx(from.0, from.1, from.2);
        let ti = self.idx(to.0, to.1, to.2);
        let from_mask = self.side_aperture(fi, dir);
        let to_mask = self.side_aperture(ti, (-dir.0, -dir.1, -dir.2));
        from_mask & to_mask != 0
    }

    pub(super) fn transmits_direct_skylight(self, at: (usize, usize, usize)) -> bool {
        Block::from_id(self.blocks[self.idx(at.0, at.1, at.2)]).transmits_direct_skylight()
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
            // Coarse per-cell opacity from the SIM bake: an opaque custom cell
            // blocks every quadrant (like a cube), an open one passes all.
            BlockLightShape::CustomAperture => {
                if self.states.custom_aperture_opaque(idx) {
                    0
                } else {
                    0b1111
                }
            }
        }
    }
}
