/// Terrain surface heights (the top solid block's y) over a rectangle of columns, read
/// positionally so every section deriving the same plan sees the same ground.
#[derive(Clone, Debug)]
pub struct Heights {
    min: [i32; 2],
    size: [i32; 2],
    values: Vec<i32>,
}

impl Heights {
    /// `None` when the host answered short (a plan must never guess missing ground).
    pub fn load(min: [i32; 2], max: [i32; 2]) -> Option<Heights> {
        let size = [max[0] - min[0] + 1, max[1] - min[1] + 1];
        let rows = (crate::TERRAIN_HEIGHTS_IN_MAX / size[0].max(1) as usize).max(1) as i32;
        let mut values = Vec::with_capacity((size[0] * size[1]).max(0) as usize);
        let mut z = min[1];
        while z <= max[1] {
            let last = (z + rows - 1).min(max[1]);
            values.extend(crate::terrain_heights_in([min[0], z], [max[0], last])?);
            z = last + 1;
        }
        Heights::from_values(min, max, values)
    }

    /// Heights over `min..=max` from `values` row by row along x; `None` when the count is wrong.
    pub fn from_values(min: [i32; 2], max: [i32; 2], values: Vec<i32>) -> Option<Heights> {
        let size = [max[0] - min[0] + 1, max[1] - min[1] + 1];
        (values.len() == (size[0].max(0) * size[1].max(0)) as usize).then_some(Heights {
            min,
            size,
            values,
        })
    }

    pub fn from_fn(min: [i32; 2], max: [i32; 2], f: impl Fn(i32, i32) -> i32) -> Heights {
        let values = Self::columns(min, max)
            .iter()
            .map(|&[x, z]| f(x, z))
            .collect();
        Heights {
            min,
            size: [max[0] - min[0] + 1, max[1] - min[1] + 1],
            values,
        }
    }

    fn columns(min: [i32; 2], max: [i32; 2]) -> Vec<[i32; 2]> {
        (min[1]..=max[1])
            .flat_map(|z| (min[0]..=max[0]).map(move |x| [x, z]))
            .collect()
    }

    #[inline]
    fn index(&self, x: i32, z: i32) -> Option<usize> {
        let (lx, lz) = (x - self.min[0], z - self.min[1]);
        (lx >= 0 && lz >= 0 && lx < self.size[0] && lz < self.size[1])
            .then(|| (lz * self.size[0] + lx) as usize)
    }

    #[inline]
    pub fn get(&self, x: i32, z: i32) -> Option<i32> {
        self.index(x, z).map(|i| self.values[i])
    }

    pub fn set(&mut self, x: i32, z: i32, y: i32) {
        if let Some(i) = self.index(x, z) {
            self.values[i] = y;
        }
    }

    /// The heights of row `z` from `x0` to `x1`; `None` unless all of them are held.
    pub fn row(&self, z: i32, x0: i32, x1: i32) -> Option<&[i32]> {
        let start = self.index(x0, z)?;
        self.index(x1, z)?;
        self.values.get(start..=start + (x1 - x0) as usize)
    }

    /// Every height, row by row along x from [`min`](Heights::min).
    pub fn values(&self) -> &[i32] {
        &self.values
    }

    pub fn min(&self) -> [i32; 2] {
        self.min
    }

    pub fn max(&self) -> [i32; 2] {
        [
            self.min[0] + self.size[0] - 1,
            self.min[1] + self.size[1] - 1,
        ]
    }
}

/// Surface biome ids for `columns`, or `None` on a short reply.
pub fn surface_biomes(columns: Vec<[i32; 2]>) -> Option<Vec<u8>> {
    let want = columns.len();
    let biomes = crate::paged(columns, crate::surface_biome_at);
    (biomes.len() == want).then_some(biomes)
}
