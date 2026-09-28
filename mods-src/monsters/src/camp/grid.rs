/// A square of per-column values around a camp site, addressed by world column.
#[derive(Clone)]
pub(super) struct Grid<T> {
    min: [i32; 2],
    size: i32,
    cells: Vec<T>,
    outside: T,
}

impl<T: Copy> Grid<T> {
    pub(super) fn new(min: [i32; 2], size: i32, fill: T) -> Grid<T> {
        Grid {
            min,
            size,
            cells: vec![fill; (size * size) as usize],
            outside: fill,
        }
    }

    #[inline]
    fn index(&self, [x, z]: [i32; 2]) -> Option<usize> {
        let (lx, lz) = (x - self.min[0], z - self.min[1]);
        (lx >= 0 && lz >= 0 && lx < self.size && lz < self.size)
            .then(|| (lz * self.size + lx) as usize)
    }

    /// A grid holding `cells` row by row along x; `outside` answers beyond it.
    pub(super) fn from_cells(min: [i32; 2], size: i32, cells: Vec<T>, outside: T) -> Grid<T> {
        debug_assert_eq!(cells.len(), (size * size) as usize);
        Grid {
            min,
            size,
            cells,
            outside,
        }
    }

    pub(super) fn min(&self) -> [i32; 2] {
        self.min
    }

    pub(super) fn size(&self) -> i32 {
        self.size
    }

    #[inline]
    pub(super) fn contains(&self, c: [i32; 2]) -> bool {
        self.index(c).is_some()
    }

    #[inline]
    pub(super) fn get(&self, c: [i32; 2]) -> T {
        self.index(c).map_or(self.outside, |i| self.cells[i])
    }

    #[inline]
    pub(super) fn set(&mut self, c: [i32; 2], v: T) {
        if let Some(i) = self.index(c) {
            self.cells[i] = v;
        }
    }

    /// Row `z` from `x0` to `x1`, all inside the grid.
    #[inline]
    pub(super) fn row(&self, z: i32, x0: i32, x1: i32) -> &[T] {
        let i = (z - self.min[1]) as usize * self.size as usize + (x0 - self.min[0]) as usize;
        &self.cells[i..=i + (x1 - x0) as usize]
    }

    /// [`row`](Grid::row), writable.
    #[inline]
    pub(super) fn row_mut(&mut self, z: i32, x0: i32, x1: i32) -> &mut [T] {
        let i = (z - self.min[1]) as usize * self.size as usize + (x0 - self.min[0]) as usize;
        &mut self.cells[i..=i + (x1 - x0) as usize]
    }
}
