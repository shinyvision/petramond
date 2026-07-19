use crate::biome::Biome;
use crate::worldgen::biome::climate::{
    BiomeClimateIndex, ClimateSampleCell, ClimateSampler, SurfaceClimate, CLIMATE_SAMPLE_CELL_X,
    CLIMATE_SAMPLE_CELL_Z,
};
use rustc_hash::FxHashMap;

/// Memoized base climate for one shared 4×4 climate cell: the sampled climate
/// vector plus its classified base biome. Coast/beach derivation still layers the
/// per-column surface height on top, but the expensive noise sample + nearest-rect
/// classification happens once per cell.
#[derive(Copy, Clone)]
pub(super) struct CellClimate {
    pub(super) climate: SurfaceClimate,
    pub(super) base: Biome,
}

pub(super) struct ClimateCellCache<'a> {
    sampler: ClimateSampler<'a>,
    index: &'a BiomeClimateIndex,
    seed: u32,
    /// Raw climate sampled once per 4×4 cell corner (the expensive noise step).
    climate: FxHashMap<ClimateSampleCell, SurfaceClimate>,
    /// Coarse per-cell classification of that corner — only the cheap ocean
    /// proximity scan needs cell-resolution biomes.
    base: FxHashMap<ClimateSampleCell, Biome>,
    /// The most recent 4×4 block's four corner climates (+ lazily, whether its
    /// corner BASES agree). Region loops visit whole blocks of columns sharing
    /// one climate cell, so this collapses their per-column corner fetches to
    /// one map round-trip per block. Pure memoization of pure values.
    block: Option<BlockMemo>,
    /// Memoized beach ocean-proximity scans, keyed by the query's climate cell —
    /// the scan offsets are cell-size multiples, so the answer is a pure
    /// function of the cell (see `near_ocean_climate`).
    near_ocean: FxHashMap<(i32, i32), bool>,
}

/// Cached corners of one climate block (see [`ClimateCellCache::block`]).
struct BlockMemo {
    cx: i32,
    cz: i32,
    corners: [SurfaceClimate; 4],
    /// `None` = not yet derived; `Some(base_agreement)` mirrors
    /// [`ClimateCellCache::uniform_base`]'s answer for this block.
    uniform: Option<Option<Biome>>,
}

/// One memoized quart-cell climate sample (+ optionally its DEFAULT-index base
/// classification — see [`ClimateCellCache::cell_base`]).
#[derive(Clone, Copy)]
struct ClimateMemoEntry {
    init: bool,
    seed: u32,
    cell: ClimateSampleCell,
    climate: SurfaceClimate,
    /// `Some` only when classified with the process-wide default surface index
    /// (`BiomeClimateIndex::default_surface` — pointer identity), so a custom
    /// (test) index can never read another index's classification.
    base: Option<Biome>,
}

/// Per-thread, world-anchored memo of raw quart-cell climate samples (the
/// 5-channel double-perlin — the expensive step). A fresh [`ClimateCellCache`]
/// is built per region call, and adjacent window tiles share edge cells, so
/// without this the same quart corner is re-sampled by several tile builds.
/// Keyed by exact `(seed, cell)`: pure dedupe, values byte-identical.
const CLIMATE_MEMO_BITS: u32 = 15;

thread_local! {
    static CLIMATE_MEMO: std::cell::RefCell<Box<[ClimateMemoEntry]>> =
        std::cell::RefCell::new(
            vec![
                ClimateMemoEntry {
                    init: false,
                    seed: 0,
                    cell: ClimateSampleCell::surface(0, 0),
                    climate: SurfaceClimate::new(0.0, 0.0, 0.0, 0.0, 0.0),
                    base: None,
                };
                1 << CLIMATE_MEMO_BITS
            ]
            .into_boxed_slice(),
        );
}

fn climate_memo_idx(seed: u32, cell: ClimateSampleCell) -> usize {
    let (x, y, z) = cell.coords();
    let key = (((x as u32 as u64) << 32) | (z as u32 as u64))
        ^ ((seed as u64) << 16)
        ^ ((y as u32 as u64) << 8);
    let h = key.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    (h >> (64 - CLIMATE_MEMO_BITS)) as usize
}

impl<'a> ClimateCellCache<'a> {
    pub(super) fn new(
        sampler: ClimateSampler<'a>,
        index: &'a BiomeClimateIndex,
        seed: u32,
    ) -> Self {
        Self {
            sampler,
            index,
            seed,
            climate: FxHashMap::default(),
            base: FxHashMap::default(),
            block: None,
            near_ocean: FxHashMap::default(),
        }
    }

    /// Memoize `scan` per climate cell of `(wx, wz)`. Only sound for scans whose
    /// answer is a pure function of the query's cell — the beach ocean-proximity
    /// scan proves that by construction (cell-size-multiple offsets).
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

    /// The four corner climates of the climate block holding cell indices
    /// `(cx, cz)`, through the one-block memo.
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
        let sampler = self.sampler;
        let climate = CLIMATE_MEMO.with(|memo| {
            let mut memo = memo.borrow_mut();
            let e = &mut memo[climate_memo_idx(seed, cell)];
            if e.init && e.seed == seed && e.cell == cell {
                return e.climate;
            }
            let climate = sampler
                .sample_surface_cell(cell)
                .expect("surface density graph must expose climate channels")
                .climate;
            *e = ClimateMemoEntry {
                init: true,
                seed,
                cell,
                climate,
                base: None,
            };
            climate
        });
        self.climate.insert(cell, climate);
        climate
    }

    pub(super) fn cell_base(&mut self, cell: ClimateSampleCell) -> Biome {
        if let Some(cached) = self.base.get(&cell) {
            return *cached;
        }
        // With the process-wide DEFAULT index, the classification is a pure
        // function of `(seed, cell)` like the climate itself — memoize it in
        // the same thread-local slot so region rebuilds and beach scans don't
        // re-classify shared corners. A custom index (tests) skips the memo.
        let is_default = std::ptr::eq(self.index, BiomeClimateIndex::default_surface());
        let seed = self.seed;
        if is_default {
            let memoized = CLIMATE_MEMO.with(|memo| {
                let memo = memo.borrow();
                let e = &memo[climate_memo_idx(seed, cell)];
                if e.init && e.seed == seed && e.cell == cell {
                    e.base
                } else {
                    None
                }
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
                let mut memo = memo.borrow_mut();
                let e = &mut memo[climate_memo_idx(seed, cell)];
                if e.init && e.seed == seed && e.cell == cell {
                    e.base = Some(base);
                }
            });
        }
        base
    }

    /// Per-column climate, bilinearly interpolated from the four surrounding 4×4
    /// cell corners so biome edges resolve to single blocks instead of 4×4 steps.
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
        // Only boundary cells (corners disagreeing on biome) need a per-column
        // classification; a cell whose four corners agree is biome interior, so
        // reuse that biome and skip the nearest-rect query for all 16 columns.
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

