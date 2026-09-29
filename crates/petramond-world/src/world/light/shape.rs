use std::cell::RefCell;

use rustc_hash::FxHashMap;

use crate::block::{Block, ShapeNeighborhood, ShapeState, LIGHT_CELL_SHAPED};
use crate::mathh::IVec3;

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
///
/// A family's apertures are a function of the cell's own block and stored
/// state (refinement already folded every neighbour into that state), so the
/// answer is memoized per `(block, state)`: a section of dripstone or stairs
/// pays one table read per stateful cell instead of re-resolving its box set
/// for each of the 24 aperture probes.
pub fn collect_light_overrides(
    section: &crate::section::Section,
    mut idx: impl FnMut(usize, usize, usize) -> usize,
    states: &mut Vec<SparseCellState>,
) {
    let map = section.cell_states();
    if !map.is_empty() {
        let cells = crate::block::light_cells();
        let blocks = section.blocks();
        APERTURES.with(|memo| {
            let mut memo = memo.borrow_mut();
            let memo = memo.current();
            for (&key, &state) in map {
                let id = blocks.get(key as usize);
                let word = cells.get(id as usize).copied().unwrap_or(cells[0]);
                if word & LIGHT_CELL_SHAPED == 0 {
                    continue;
                }
                let (lx, ly, lz) = crate::chunk::section_local(key as usize);
                states.push(SparseCellState {
                    idx: idx(lx, ly, lz),
                    masks: memo.apertures(id, state),
                });
            }
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

struct ApertureMemo {
    serial: u64,
    table: FxHashMap<(u16, ShapeState), u32>,
}

thread_local! {
    static APERTURES: RefCell<ApertureMemo> = RefCell::new(ApertureMemo {
        serial: 0,
        table: FxHashMap::default(),
    });
}

impl ApertureMemo {
    fn current(&mut self) -> &mut Self {
        let serial = crate::content::current().serial();
        if serial != self.serial {
            self.serial = serial;
            self.table.clear();
        }
        self
    }

    fn apertures(&mut self, id: u16, state: ShapeState) -> u32 {
        *self.table.entry((id, state)).or_insert_with(|| {
            let block = Block::from_id(id);
            let k = block.shape_kind_def();
            k.sim
                .light_apertures(&k.params, &SoloCell { block, state }, IVec3::ZERO, block)
        })
    }
}

struct SoloCell {
    block: Block,
    state: ShapeState,
}

impl ShapeNeighborhood for SoloCell {
    fn block(&self, pos: IVec3) -> Block {
        if pos == IVec3::ZERO {
            self.block
        } else {
            Block::Air
        }
    }

    fn shape_state(&self, pos: IVec3) -> ShapeState {
        if pos == IVec3::ZERO {
            self.state
        } else {
            ShapeState::NONE
        }
    }
}

pub struct SparseCellState {
    pub idx: usize,
    pub masks: u32,
}

/// The per-cell aperture overrides of one flood cube, kept on the worker between bakes: each
/// entry carries the generation it was written in, so a bake that has overrides stamps only its
/// own cells instead of allocating and clearing a 48³ (or 64³) table.
#[derive(Default)]
pub struct ApertureScratch {
    gen: u32,
    cells: Vec<u64>,
    any: bool,
}

impl ApertureScratch {
    pub fn fill(&mut self, states: &[SparseCellState], volume: usize) {
        self.any = !states.is_empty();
        if !self.any {
            return;
        }
        if self.cells.len() < volume {
            self.cells.resize(volume, 0);
        }
        self.gen = self.gen.wrapping_add(1);
        if self.gen == 0 {
            self.cells.fill(0);
            self.gen = 1;
        }
        let tag = u64::from(self.gen) << 32;
        for state in states {
            if state.idx < volume {
                self.cells[state.idx] = tag | u64::from(state.masks);
            }
        }
    }

    fn view(&self, volume: usize) -> Option<(&[u64], u32)> {
        self.any.then(|| (&self.cells[..volume], self.gen))
    }
}

/// The block ids of a flood cube, at whichever width the sections came in.
pub trait BlockIds: Copy {
    fn id(self, i: usize) -> u16;
    fn count(self) -> usize;
}

impl BlockIds for &[u8] {
    #[inline]
    fn id(self, i: usize) -> u16 {
        u16::from(self[i])
    }

    #[inline]
    fn count(self) -> usize {
        self.len()
    }
}

impl BlockIds for &[u16] {
    #[inline]
    fn id(self, i: usize) -> u16 {
        self[i]
    }

    #[inline]
    fn count(self) -> usize {
        self.len()
    }
}

#[derive(Copy, Clone)]
pub enum Ids<'a> {
    Narrow(&'a [u8]),
    Wide(&'a [u16]),
}

/// Run `$body` with `$cells` bound to the [`LightCells`] over `$ids` at either width.
macro_rules! with_light_cells {
    ($ids:expr, $apertures:expr, $dim:expr, |$cells:ident| $body:expr) => {
        match $ids {
            $crate::world::light::shape::Ids::Narrow(b) => {
                let $cells = $crate::world::light::shape::LightCells::new(b, $apertures, $dim);
                $body
            }
            $crate::world::light::shape::Ids::Wide(b) => {
                let $cells = $crate::world::light::shape::LightCells::new(b, $apertures, $dim);
                $body
            }
        }
    };
}
pub(super) use with_light_cells;

#[derive(Copy, Clone)]
pub struct LightCells<'a, B: BlockIds> {
    blocks: B,
    apertures: Option<(&'a [u64], u32)>,
    cells: &'static [u32],
}

impl<'a, B: BlockIds> LightCells<'a, B> {
    pub fn new(blocks: B, states: &'a ApertureScratch, dim: usize) -> Self {
        debug_assert_eq!(blocks.count(), dim * dim * dim);
        Self {
            blocks,
            apertures: states.view(dim * dim * dim),
            cells: crate::block::light_cells(),
        }
    }

    #[inline]
    pub fn word(self, idx: usize) -> u32 {
        resolve_word(self.cells, self.blocks.id(idx), || match self.apertures {
            Some((a, gen)) => {
                let v = a[idx];
                ((v >> 32) as u32 == gen).then_some(v as u32)
            }
            None => None,
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
