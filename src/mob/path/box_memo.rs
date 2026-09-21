//! What searches over one box keep about its cells: per-cell predicate memos
//! ([`BoxFacts`]) and per-foothold move lists ([`BoxLeads`]), both flat tables
//! over the box. A search confined to a box (a flood) asks about every cell of
//! it many times over, which a direct-mapped [`super::CellMemo`] far smaller
//! than the box mostly misses; and the tables may outlive the search, so the
//! next one over the same box starts warm.

use std::cell::{Cell, RefCell};

use petramond_math::math::IVec3;

use super::CellCache;

/// The cells of an inclusive box, numbered for a flat table. A box too large
/// for a table has none: every cell reads as outside it.
#[derive(Clone, Copy)]
struct BoxGrid {
    min: IVec3,
    dims: IVec3,
}

impl BoxGrid {
    fn new(min: IVec3, max: IVec3) -> Self {
        let dims = (max - min + IVec3::ONE).max(IVec3::ZERO);
        let volume = i64::from(dims.x) * i64::from(dims.y) * i64::from(dims.z);
        BoxGrid {
            min,
            dims: if volume > MAX_CELLS {
                IVec3::ZERO
            } else {
                dims
            },
        }
    }

    fn cells(&self) -> usize {
        (self.dims.x * self.dims.y * self.dims.z) as usize
    }

    #[inline]
    fn slot(&self, c: IVec3) -> Option<usize> {
        let d = c - self.min;
        ((d.x as u32) < self.dims.x as u32
            && (d.y as u32) < self.dims.y as u32
            && (d.z as u32) < self.dims.z as u32)
            .then(|| ((d.y * self.dims.z + d.z) * self.dims.x + d.x) as usize)
    }

    /// Every table slot of the cells in `min..=max` that lie in the box.
    fn slots_in(&self, min: IVec3, max: IVec3) -> impl Iterator<Item = usize> + '_ {
        let lo = (min - self.min).max(IVec3::ZERO);
        let hi = (max - self.min).min(self.dims - IVec3::ONE);
        (lo.y..=hi.y).flat_map(move |y| {
            (lo.z..=hi.z).flat_map(move |z| {
                (lo.x..=hi.x).map(move |x| ((y * self.dims.z + z) * self.dims.x + x) as usize)
            })
        })
    }
}

/// Boxes past this many cells get no table at all.
pub const MAX_CELLS: i64 = 1 << 20;

/// Whether a table over `min..=max` would hold the box (see [`MAX_CELLS`]).
pub fn fits_a_table(min: IVec3, max: IVec3) -> bool {
    BoxGrid::new(min, max).cells() > 0
}

/// Memos for up to sixteen per-cell predicates over one box, a lane each. A
/// cell outside the box is simply computed.
pub struct BoxFacts {
    grid: BoxGrid,
    /// Per cell and lane, two bits: known, and the answer.
    cells: Vec<Cell<u32>>,
}

/// One predicate's lane of a [`BoxFacts`].
#[derive(Clone, Copy)]
pub struct Fact<'a> {
    facts: &'a BoxFacts,
    shift: u32,
}

impl BoxFacts {
    pub fn new(min: IVec3, max: IVec3) -> Self {
        let grid = BoxGrid::new(min, max);
        BoxFacts {
            cells: vec![Cell::new(0); grid.cells()],
            grid,
        }
    }

    /// How many cells the table holds: what keeping it costs.
    pub fn cells(&self) -> usize {
        self.cells.len()
    }

    /// Forget every lane of the cells in `min..=max`: the world changed there.
    pub fn forget(&self, min: IVec3, max: IVec3) {
        for slot in self.grid.slots_in(min, max) {
            self.cells[slot].set(0);
        }
    }

    /// Lane `n` of sixteen.
    pub fn fact(&self, n: u32) -> Fact<'_> {
        assert!(n < 16);
        Fact {
            facts: self,
            shift: n * 2,
        }
    }
}

impl CellCache for Fact<'_> {
    #[inline]
    fn get(&self, c: IVec3, compute: impl FnOnce(IVec3) -> bool) -> bool {
        let Some(slot) = self.facts.grid.slot(c) else {
            return compute(c);
        };
        let slot = &self.facts.cells[slot];
        let (known, yes) = (1u32 << self.shift, 2u32 << self.shift);
        let bits = slot.get();
        if bits & known != 0 {
            return bits & yes != 0;
        }
        let answer = compute(c);
        // Re-read: computing one fact may have filled others of this cell.
        slot.set(slot.get() | known | if answer { yes } else { 0 });
        answer
    }
}

/// A list of cells per cell of a box — where a foothold leads in one move, or
/// what leads into a cell. Working a list out is most of a flood, and it
/// stays true until a cell near it changes. Cells outside the box are simply
/// worked out each time.
pub struct BoxLeads {
    grid: BoxGrid,
    /// Per cell: 1 + where its list starts in `lists`, and its length;
    /// 0 = not worked out.
    slots: Vec<Cell<(u32, u32)>>,
    lists: RefCell<Vec<IVec3>>,
}

impl BoxLeads {
    /// Forgotten lists stay in `lists` until it holds this many cells.
    const SPILL: usize = 1 << 20;

    pub fn new(min: IVec3, max: IVec3) -> Self {
        let grid = BoxGrid::new(min, max);
        BoxLeads {
            slots: vec![Cell::new((0, 0)); grid.cells()],
            grid,
            lists: RefCell::default(),
        }
    }

    /// Forget the lists of the cells in `min..=max`: the world changed there.
    pub fn forget(&self, min: IVec3, max: IVec3) {
        for slot in self.grid.slots_in(min, max) {
            self.slots[slot].set((0, 0));
        }
        if self.lists.borrow().len() > Self::SPILL {
            self.slots.iter().for_each(|slot| slot.set((0, 0)));
            self.lists.borrow_mut().clear();
        }
    }

    /// The list of `a`, into `out`: kept from an earlier ask unless `volatile`
    /// (this ask's own terms change it), else worked out by `work`.
    pub(super) fn of(
        &self,
        a: IVec3,
        volatile: bool,
        out: &mut Vec<IVec3>,
        work: impl FnOnce(&mut Vec<IVec3>),
    ) {
        out.clear();
        let Some(slot) = self.grid.slot(a).filter(|_| !volatile) else {
            return work(out);
        };
        let slot = &self.slots[slot];
        let (at, len) = slot.get();
        if at != 0 {
            let lists = self.lists.borrow();
            out.extend_from_slice(&lists[at as usize - 1..(at - 1 + len) as usize]);
            return;
        }
        work(out);
        let mut lists = self.lists.borrow_mut();
        slot.set((lists.len() as u32 + 1, out.len() as u32));
        lists.extend_from_slice(out);
    }
}
