use crate::block::{Block, BlockLightShape};

/// Collect every per-cell aperture OVERRIDE a section holds, in the sparse
/// currency the floods read: each `Shaped`-light cell with stored state has its
/// FAMILY answer its packed per-face apertures (`ShapeSim::light_apertures`),
/// then each WASM custom-shape cell's baked "opaque to light" bit lands on top
/// (opaque blocks every quadrant, open passes all). Later entries win in
/// [`ShapeStateSnapshot::from_sparse`], so a custom bake overrides its cell's
/// family answer.
///
/// This is the ONE producer of light overrides: the per-section and batched
/// span gathers and the incremental relight all read a section through it, so
/// no light path can drop a kind of override the others honour.
pub fn collect_light_overrides(
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
    if let Some(aps) = section.custom_light_apertures() {
        states.extend(aps.iter().map(|(&key, &opaque)| {
            let (lx, ly, lz) = crate::chunk::section_local(key as usize);
            SparseCellState {
                idx: idx(lx, ly, lz),
                masks: if opaque {
                    0
                } else {
                    crate::block::LIGHT_APERTURES_OPEN
                },
            }
        }));
    }
}

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

pub struct SparseCellState {
    pub idx: usize,
    pub masks: u32,
}

const NO_ENTRY: u32 = u32::MAX;

#[derive(Default)]
pub struct ShapeStateSnapshot {
    apertures: Option<Box<[u32]>>,
}

impl ShapeStateSnapshot {
    pub fn from_sparse(states: &[SparseCellState], volume: usize) -> Self {
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

    fn apertures(&self) -> Option<&[u32]> {
        self.apertures.as_deref()
    }
}

#[derive(Copy, Clone)]
pub struct LightCells<'a> {
    blocks: &'a [u16],
    apertures: Option<&'a [u32]>,
    cells: &'static [u32],
}

impl<'a> LightCells<'a> {
    pub fn new(blocks: &'a [u16], states: &'a ShapeStateSnapshot, dim: usize) -> Self {
        debug_assert_eq!(blocks.len(), dim * dim * dim);
        Self {
            blocks,
            apertures: states.apertures(),
            cells: crate::block::light_cells(),
        }
    }

    #[inline]
    pub fn word(self, idx: usize) -> u32 {
        resolve_word(self.cells, self.blocks[idx], || match self.apertures {
            Some(a) if a[idx] != NO_ENTRY => Some(a[idx]),
            _ => None,
        })
    }
}

#[inline]
pub(super) fn resolve_word(
    cells: &[u32],
    id: u16,
    override_masks: impl FnOnce() -> Option<u32>,
) -> u32 {
    let w = cells.get(id as usize).copied().unwrap_or(cells[0]);
    if w & crate::block::LIGHT_CELL_SHAPED == 0 {
        return w;
    }
    match override_masks() {
        Some(masks) => masks | (w & crate::block::LIGHT_CELL_DIRECT_SKY),
        None => w,
    }
}
