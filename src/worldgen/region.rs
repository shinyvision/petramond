use crate::biome::Biome;

/// A generated block-region's final biome and top-solid surface data.
///
/// `surf` and `biomes` are row-major `(wx-x0) + (wz-z0)*w`.
pub(crate) struct RegionCells {
    pub x0: i32,
    pub z0: i32,
    pub w: usize,
    pub h: usize,
    pub surf: Vec<i32>,
    pub biomes: Vec<Biome>,
}

impl RegionCells {
    pub(crate) fn new(x0: i32, z0: i32, w: usize, h: usize) -> Self {
        Self {
            x0,
            z0,
            w,
            h,
            surf: vec![0; w * h],
            biomes: vec![Biome::Ocean; w * h],
        }
    }

    #[inline]
    pub(crate) fn index(&self, wx: i32, wz: i32) -> usize {
        (wz - self.z0) as usize * self.w + (wx - self.x0) as usize
    }

    #[inline]
    pub(crate) fn at(&self, wx: i32, wz: i32) -> (i32, Biome) {
        let i = self.index(wx, wz);
        (self.surf[i], self.biomes[i])
    }
}
