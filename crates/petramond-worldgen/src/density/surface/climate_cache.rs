use crate::biome::climate::{
    BiomeClimateIndex, ClimateSampleCell, SurfaceClimate, CLIMATE_SAMPLE_CELL_X,
    CLIMATE_SAMPLE_CELL_Z,
};
use crate::cache::local::{self, LocalTable};
use crate::density::columns::Columns;
use petramond_world::biome::Biome;
use rustc_hash::FxHashMap;

#[derive(Copy, Clone)]
pub(super) struct CellClimate {
    pub(super) climate: SurfaceClimate,
    pub(super) base: Biome,
}

pub(super) struct ClimateCellCache<'a> {
    columns: &'a Columns,
    index: &'a BiomeClimateIndex,
    seed: u32,
    climate: FxHashMap<ClimateSampleCell, SurfaceClimate>,
    base: FxHashMap<ClimateSampleCell, Biome>,
    block: Option<BlockMemo>,
    near_ocean: FxHashMap<(i32, i32), bool>,
}

struct BlockMemo {
    cx: i32,
    cz: i32,
    corners: [SurfaceClimate; 4],
    uniform: Option<Option<Biome>>,
}

/// Per-thread, world-anchored memo of raw quart-cell climate samples (the
/// 5-channel double-perlin — the expensive step), plus — once classified with
/// the process-wide DEFAULT surface index (`BiomeClimateIndex::default_surface`,
/// pointer identity, so a custom test index can never read another index's
/// answer) — the cell's base biome. A fresh [`ClimateCellCache`] is built per
/// region call, and adjacent window tiles share edge cells, so without this
/// the same quart corner is re-sampled by several tile builds. Keyed by exact
/// `(seed, cell)`: pure dedupe, values unchanged.
type MemoValue = (SurfaceClimate, Option<Biome>);

thread_local! {
    static CLIMATE_MEMO: LocalTable<(u32, ClimateSampleCell), MemoValue> =
        LocalTable::new(&local::SURFACE_CLIMATE);
}

fn climate_memo_hash(seed: u32, cell: ClimateSampleCell) -> u64 {
    let (x, y, z) = cell.coords();
    local::spread(
        (((x as u32 as u64) << 32) | (z as u32 as u64))
            ^ ((seed as u64) << 16)
            ^ ((y as u32 as u64) << 8),
    )
}

impl<'a> ClimateCellCache<'a> {
    pub(super) fn new(columns: &'a Columns, index: &'a BiomeClimateIndex, seed: u32) -> Self {
        Self {
            columns,
            index,
            seed,
            climate: FxHashMap::default(),
            base: FxHashMap::default(),
            block: None,
            near_ocean: FxHashMap::default(),
        }
    }

    pub(super) fn near_ocean_memo(
        &mut self,
        wx: i32,
        wz: i32,
        scan: impl FnOnce(&mut Self, i32, i32) -> bool,
    ) -> bool {
        let key = (
            wx.div_euclid(CLIMATE_SAMPLE_CELL_X),
            wz.div_euclid(CLIMATE_SAMPLE_CELL_Z),
        );
        if let Some(&v) = self.near_ocean.get(&key) {
            return v;
        }
        let v = scan(self, wx, wz);
        self.near_ocean.insert(key, v);
        v
    }

    fn block_corners(&mut self, cx: i32, cz: i32) -> [SurfaceClimate; 4] {
        if let Some(b) = &self.block {
            if b.cx == cx && b.cz == cz {
                return b.corners;
            }
        }
        let corners = [
            self.cell_climate(ClimateSampleCell::at_surface_indices(cx, cz)),
            self.cell_climate(ClimateSampleCell::at_surface_indices(cx + 1, cz)),
            self.cell_climate(ClimateSampleCell::at_surface_indices(cx, cz + 1)),
            self.cell_climate(ClimateSampleCell::at_surface_indices(cx + 1, cz + 1)),
        ];
        self.block = Some(BlockMemo {
            cx,
            cz,
            corners,
            uniform: None,
        });
        corners
    }

    fn cell_climate(&mut self, cell: ClimateSampleCell) -> SurfaceClimate {
        if let Some(cached) = self.climate.get(&cell) {
            return *cached;
        }
        let seed = self.seed;
        let columns = self.columns;
        let (climate, _) = CLIMATE_MEMO.with(|memo| {
            memo.get_or_insert_with(climate_memo_hash(seed, cell), (seed, cell), || {
                let (x, _, z) = cell.origin();
                (SurfaceClimate::from_column(&columns.at(x, z)), None)
            })
        });
        self.climate.insert(cell, climate);
        climate
    }

    pub(super) fn cell_base(&mut self, cell: ClimateSampleCell) -> Biome {
        if let Some(cached) = self.base.get(&cell) {
            return *cached;
        }
        let is_default = std::ptr::eq(self.index, BiomeClimateIndex::default_surface());
        let seed = self.seed;
        if is_default {
            let memoized = CLIMATE_MEMO.with(|memo| {
                memo.get(climate_memo_hash(seed, cell), &(seed, cell))
                    .and_then(|(_, base)| base)
            });
            if let Some(base) = memoized {
                self.base.insert(cell, base);
                return base;
            }
        }
        let climate = self.cell_climate(cell);
        let base = self
            .index
            .classify_surface(climate)
            .expect("surface climate index must classify default biomes");
        self.base.insert(cell, base);
        if is_default {
            CLIMATE_MEMO.with(|memo| {
                memo.update(
                    climate_memo_hash(seed, cell),
                    &(seed, cell),
                    |(_, memoized)| {
                        *memoized = Some(base);
                    },
                );
            });
        }
        base
    }

    pub(super) fn climate_at(&mut self, wx: i32, wz: i32) -> SurfaceClimate {
        let cx = wx.div_euclid(CLIMATE_SAMPLE_CELL_X);
        let cz = wz.div_euclid(CLIMATE_SAMPLE_CELL_Z);
        let fx = (wx - cx * CLIMATE_SAMPLE_CELL_X) as f32 / CLIMATE_SAMPLE_CELL_X as f32;
        let fz = (wz - cz * CLIMATE_SAMPLE_CELL_Z) as f32 / CLIMATE_SAMPLE_CELL_Z as f32;
        let [c00, c10, c01, c11] = self.block_corners(cx, cz);
        SurfaceClimate::bilerp(c00, c10, c01, c11, fx, fz)
    }

    pub(super) fn at(&mut self, wx: i32, wz: i32) -> CellClimate {
        let climate = self.climate_at(wx, wz);
        let base = self.uniform_base(wx, wz).unwrap_or_else(|| {
            self.index
                .classify_surface(climate)
                .expect("surface climate index must classify default biomes")
        });
        CellClimate { climate, base }
    }

    fn uniform_base(&mut self, wx: i32, wz: i32) -> Option<Biome> {
        let cx = wx.div_euclid(CLIMATE_SAMPLE_CELL_X);
        let cz = wz.div_euclid(CLIMATE_SAMPLE_CELL_Z);
        if let Some(b) = &self.block {
            if b.cx == cx && b.cz == cz {
                if let Some(uniform) = b.uniform {
                    return uniform;
                }
            }
        }
        let base = self.cell_base(ClimateSampleCell::at_surface_indices(cx, cz));
        let agree = self.cell_base(ClimateSampleCell::at_surface_indices(cx + 1, cz)) == base
            && self.cell_base(ClimateSampleCell::at_surface_indices(cx, cz + 1)) == base
            && self.cell_base(ClimateSampleCell::at_surface_indices(cx + 1, cz + 1)) == base;
        let uniform = agree.then_some(base);
        if let Some(b) = &mut self.block {
            if b.cx == cx && b.cz == cz {
                b.uniform = Some(uniform);
            }
        }
        uniform
    }
}
