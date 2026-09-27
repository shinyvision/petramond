use std::cell::{Cell, RefCell};

use petramond_math::math::IVec3;

use super::CellCache;

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

pub const MAX_CELLS: i64 = 1 << 20;

pub fn fits_a_table(min: IVec3, max: IVec3) -> bool {
    BoxGrid::new(min, max).cells() > 0
}

pub struct BoxFacts {
    grid: BoxGrid,
    cells: Vec<Cell<u32>>,
}

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

    pub fn cells(&self) -> usize {
        self.cells.len()
    }

    pub fn forget(&self, min: IVec3, max: IVec3) {
        for slot in self.grid.slots_in(min, max) {
            self.cells[slot].set(0);
        }
    }

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
        slot.set(slot.get() | known | if answer { yes } else { 0 });
        answer
    }
}

pub struct BoxLeads {
    grid: BoxGrid,
    slots: Vec<Cell<(u32, u32)>>,
    lists: RefCell<Vec<IVec3>>,
}

impl BoxLeads {
    const SPILL: usize = 1 << 20;

    pub fn new(min: IVec3, max: IVec3) -> Self {
        let grid = BoxGrid::new(min, max);
        BoxLeads {
            slots: vec![Cell::new((0, 0)); grid.cells()],
            grid,
            lists: RefCell::default(),
        }
    }

    pub fn forget(&self, min: IVec3, max: IVec3) {
        for slot in self.grid.slots_in(min, max) {
            self.slots[slot].set((0, 0));
        }
        if self.lists.borrow().len() > Self::SPILL {
            self.slots.iter().for_each(|slot| slot.set((0, 0)));
            self.lists.borrow_mut().clear();
        }
    }

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
