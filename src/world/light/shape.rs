use crate::block::{Block, BlockLightShape};

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
    let nb = SectionCells(section);
    for &key in section.cell_states().keys() {
        let (lx, ly, lz) = crate::chunk::section_local(key as usize);
        let block = section.block(lx, ly, lz);
        if block.light_shape() != BlockLightShape::Shaped {
            continue;
        }
        let k = block.shape_kind_def();
        let pos = crate::mathh::IVec3::new(lx as i32, ly as i32, lz as i32);
        states.push(SparseCellState {
            idx: idx(lx, ly, lz),
            masks: k.sim.light_apertures(&k.params, &nb, pos, block),
        });
    }
}

/// The shape seam over ONE section, in section-local coordinates. A shape's
/// apertures come from its own cell's state (that is what makes them a pure
/// per-cell function), so a view that answers air outside the section is
/// exact rather than merely conservative.
struct SectionCells<'a>(&'a crate::section::Section);

impl crate::block::ShapeNeighborhood for SectionCells<'_> {
    fn block(&self, pos: crate::mathh::IVec3) -> Block {
        match local(pos) {
            Some((x, y, z)) => self.0.block(x, y, z),
            None => Block::Air,
        }
    }

    fn shape_state(&self, pos: crate::mathh::IVec3) -> crate::block::ShapeState {
        match local(pos) {
            Some((x, y, z)) => self.0.cell_state(x, y, z),
            None => crate::block::ShapeState::NONE,
        }
    }
}

fn local(pos: crate::mathh::IVec3) -> Option<(usize, usize, usize)> {
    let n = crate::chunk::SECTION_SIZE as i32;
    ((0..n).contains(&pos.x) && (0..n).contains(&pos.y) && (0..n).contains(&pos.z)).then_some((
        pos.x as usize,
        pos.y as usize,
        pos.z as usize,
    ))
}

/// One shaped cell's packed per-face light apertures in snapshot index space
/// (see `block::pack_light_apertures` for the layout). Family-answered at
/// gather time; the flood only ever reads bits.
pub(super) struct SparseCellState {
    pub idx: usize,
    pub masks: u32,
}

/// A cell with no gathered entry. Aperture words use only the low 24 bits, so
/// this can never collide with a real mask; it means "ask the block", not
/// "fully open" — a stateless shaped cell (a cover, a cactus) is never
/// gathered and must still block light through its own shape.
const NO_ENTRY: u32 = u32::MAX;

#[derive(Default)]
pub(super) struct ShapeStateSnapshot {
    /// Per-cell packed apertures; a cell with no entry falls back to its
    /// block's state-free apertures.
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
            let cells = apertures.get_or_insert_with(|| vec![NO_ENTRY; volume].into_boxed_slice());
            cells[state.idx] = state.masks;
        }
        Self { apertures }
    }

    /// The gathered masks for a cell, or the BLOCK's own state-free apertures
    /// when it has none. A stateless shaped cell (a cover, a cactus) never
    /// appears in the sparse gather at all, so this fallback — not
    /// "fully open" — is what makes its shape block light.
    fn aperture_masks(&self, idx: usize, block: Block) -> u32 {
        match self.apertures.as_ref().and_then(|f| f.get(idx).copied()) {
            Some(masks) if masks != NO_ENTRY => masks,
            _ => block.default_light_apertures(),
        }
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
                crate::block::light_aperture_face(self.states.aperture_masks(idx, block), dir)
            }
        }
    }
}
