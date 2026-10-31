use crate::geometry::FACES;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Bounds {
    pub min: [i32; 3],
    pub size: [usize; 3],
}

impl Bounds {
    pub fn new(min: [i32; 3], max: [i32; 3]) -> Self {
        Self {
            min,
            size: std::array::from_fn(|a| (max[a] - min[a] + 1) as usize),
        }
    }

    pub fn len(self) -> usize {
        self.size.iter().product()
    }

    pub fn index(self, cell: [i32; 3]) -> Option<usize> {
        let p: [i32; 3] = std::array::from_fn(|a| cell[a] - self.min[a]);
        (0..3)
            .all(|a| p[a] >= 0 && (p[a] as usize) < self.size[a])
            .then(|| p[0] as usize + self.size[0] * (p[2] as usize + self.size[2] * p[1] as usize))
    }

    pub fn cell(self, index: usize) -> [i32; 3] {
        [
            self.min[0] + (index % self.size[0]) as i32,
            self.min[1] + (index / (self.size[0] * self.size[2])) as i32,
            self.min[2] + (index / self.size[0] % self.size[2]) as i32,
        ]
    }

    pub fn neighbours(self, index: usize) -> [Option<usize>; 6] {
        let cell = self.cell(index);
        FACES.map(|f| self.index(std::array::from_fn(|a| cell[a] + f[a])))
    }
}
