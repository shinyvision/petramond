use crate::block::{Block, BlockLightShape, LIGHT_APERTURES_OPEN};

/// Collect a section's light-relevant per-cell apertures out of the UNIFIED
/// state store: every `Shaped`-light cell's FAMILY answers its packed
/// per-face apertures from its own stored state (`ShapeSim::light_apertures`)
/// — no family knowledge here or anywhere downstream. Shared by the
/// per-section and the batched light gathers so the two can't diverge.
pub(super) fn collect_shape_states(
    section: &crate::section::Section,
    mut idx: impl FnMut(usize, usize, usize) -> usize,
    states: &mut Vec<SparseCellState>,
) {
    for (&key, &state) in section.cell_states() {
        let (lx, ly, lz) = crate::chunk::section_local(key as usize);
        let block = section.block(lx, ly, lz);
        if block.light_shape() != BlockLightShape::Shaped {
            continue;
        }
        let k = block.shape_kind_def();
        states.push(SparseCellState {
            idx: idx(lx, ly, lz),
            masks: k.sim.light_apertures(&k.params, block, state),
        });
    }
}

/// One shaped cell's packed per-face light apertures in snapshot index space
/// (see `block::pack_light_apertures` for the layout). Family-answered at
/// gather time; the flood only ever reads bits.
pub(super) struct SparseCellState {
    pub idx: usize,
    pub masks: u32,
}

#[derive(Default)]
pub(super) struct ShapeStateSnapshot {
    /// Per-cell packed apertures; absent cells (and an absent array) read
    /// fully open — a `Shaped` cell missing its entry degrades to open, the
    /// same fallback the baked custom aperture always had.
    apertures: Option<Box<[u32]>>,
}

impl ShapeStateSnapshot {
    /// `volume` is the flood cube's cell count (48³ for a per-section bake, 64³ for
    /// a 2×2×2 batch bake); sparse indices are already in that cube's coordinates.
    pub(super) fn from_sparse(states: &[SparseCellState], volume: usize) -> Self {
        let mut apertures: Option<Box<[u32]>> = None;
        for state in states {
            if state.idx >= volume {
                continue;
            }
            let cells = apertures
                .get_or_insert_with(|| vec![LIGHT_APERTURES_OPEN; volume].into_boxed_slice());
            cells[state.idx] = state.masks;
        }
        Self { apertures }
    }

    fn aperture_masks(&self, idx: usize) -> u32 {
        self.apertures
            .as_ref()
            .and_then(|f| f.get(idx).copied())
            .unwrap_or(LIGHT_APERTURES_OPEN)
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
            // The family answered at gather time; here it's a bit read.
            BlockLightShape::Shaped => {
                crate::block::light_aperture_face(self.states.aperture_masks(idx), dir)
            }
        }
    }
}
