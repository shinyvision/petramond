use crate::biome::Biome;
use crate::chunk::CHUNK_SX;

use super::super::density::surface::SurfaceDensitySystem;
use super::super::region::RegionCells;
use super::{feature_candidate_bounds, feature_region_bounds};

pub(crate) trait FeatureField {
    fn column_at(&mut self, wx: i32, wz: i32) -> (i32, Biome);

    fn surf_at(&mut self, wx: i32, wz: i32) -> i32 {
        self.column_at(wx, wz).0
    }
}

impl FeatureField for &RegionCells {
    fn column_at(&mut self, wx: i32, wz: i32) -> (i32, Biome) {
        self.at(wx, wz)
    }
}

pub(crate) struct RuntimeFeatureField<'a> {
    surface: &'a SurfaceDensitySystem,
    caves: &'a crate::worldgen::noise::height::CaveField,
    seed: u32,
    candidates: RegionCells,
    support_bounds: (i32, i32, usize, usize),
    support_surfaces: Option<SurfaceHeights>,
}

impl<'a> RuntimeFeatureField<'a> {
    /// Candidate surfaces come pre cave-adjusted from the shared per-thread
    /// window memo (`cached_feature_region`) — eager per cell, because the
    /// spacing scans re-query the same cells many times over. This mirrors
    /// the cubic path's `finish_feature_windows`, so both paths read
    /// identical values.
    pub(crate) fn new(
        surface: &'a SurfaceDensitySystem,
        caves: &'a crate::worldgen::noise::height::CaveField,
        seed: u32,
        ox: i32,
        oz: i32,
    ) -> Self {
        let (x0, z0, w, h) = feature_candidate_bounds(ox, oz);
        let (candidates, _raw) = cached_feature_region(surface, caves, seed, x0, z0, w, h);
        Self {
            surface,
            caves,
            seed,
            candidates,
            support_bounds: feature_region_bounds(ox, oz),
            support_surfaces: None,
        }
    }

    fn support_surfaces(&mut self) -> &SurfaceHeights {
        if self.support_surfaces.is_none() {
            let (x0, z0, w, h) = self.support_bounds;
            let (region, _raw) =
                cached_feature_region(self.surface, self.caves, self.seed, x0, z0, w, h);
            self.support_surfaces = Some(SurfaceHeights::new(x0, z0, w, region.surf));
        }
        self.support_surfaces.as_ref().unwrap()
    }
}

/// One memoized 16×16 world tile of the feature windows: raw surfaces,
/// cave-adjusted surfaces, and biomes.
#[derive(Clone, Copy)]
struct RegionTile {
    init: bool,
    seed: u32,
    tcx: i32,
    tcz: i32,
    raw: [i32; 256],
    adj: [i32; 256],
    biomes: [Biome; 256],
}

/// Tile count of the window memo (direct-mapped, ~5 MB process-wide).
/// A streaming row touches a few hundred live tiles; 2048 keeps row-to-row
/// revisits hitting.
const TILE_MEMO_BITS: u32 = 11;

/// SHARED across worker threads (per-slot locks, compute-under-lock =
/// single-flight): tiles are pure functions of `(seed, tile coords)`, so any
/// worker's computation serves every other. The previous per-thread memos
/// made every pool worker redundantly recompute the same nearby tiles — at
/// world open ~20 cold workers each paid the whole spawn area's tile bill
/// (~10–30 ms per first column) before their memos warmed, and the pool held
/// ~5 MB × threads of duplicate tiles.
static TILE_MEMO: std::sync::LazyLock<Box<[std::sync::Mutex<RegionTile>]>> =
    std::sync::LazyLock::new(|| {
        (0..1usize << TILE_MEMO_BITS)
            .map(|_| {
                std::sync::Mutex::new(RegionTile {
                    init: false,
                    seed: 0,
                    tcx: 0,
                    tcz: 0,
                    raw: [0; 256],
                    adj: [0; 256],
                    biomes: [Biome::Ocean; 256],
                })
            })
            .collect()
    });

fn tile_memo_idx(seed: u32, tcx: i32, tcz: i32) -> usize {
    let key = (((tcx as u32 as u64) << 32) | (tcz as u32 as u64)) ^ ((seed as u64) << 16);
    let h = key.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    (h >> (64 - TILE_MEMO_BITS)) as usize
}

/// The feature window for `(x0,z0,w,h)`: cave-adjusted surfaces + biomes in
/// the returned [`RegionCells`], plus the RAW (pre-adjustment) surfaces the
/// column core needs. Assembled from per-thread memoized 16×16 world tiles:
/// neighbouring chunks' candidate windows overlap ~18×, and this window
/// build dominated whole-world generation (~75%, 2026-07-13) before the
/// memo. Tiles are the memo unit because windows are chunk-aligned — every
/// window covers whole tiles, so a tile computes once and is copied ever
/// after (per-cell memoization died by scattered evictions defeating bulk
/// recomputation).
///
/// Byte-identical by construction: a tile is keyed by exact
/// `(seed, tile coords)` and every value is a pure world-anchored function of
/// that key (the density lattice's corner grid is world-anchored, so region
/// bounds don't affect per-column results) — the memo can only dedupe work.
/// `runtime_field_matches_full_region_field` pins this against an uncached
/// reference.
pub(crate) fn cached_feature_region(
    surface: &SurfaceDensitySystem,
    caves: &crate::worldgen::noise::height::CaveField,
    seed: u32,
    x0: i32,
    z0: i32,
    w: usize,
    h: usize,
) -> (RegionCells, Vec<i32>) {
    const T: i32 = CHUNK_SX as i32;
    let mut region = RegionCells::new(x0, z0, w, h);
    let mut raw = vec![0i32; w * h];
    let (x1, z1) = (x0 + w as i32, z0 + h as i32);

    for tcz in z0.div_euclid(T)..=(z1 - 1).div_euclid(T) {
        for tcx in x0.div_euclid(T)..=(x1 - 1).div_euclid(T) {
            // Holding the slot lock across the miss computation is the
            // single-flight: a second worker needing this tile blocks until
            // the bytes exist instead of recomputing them. A poisoned slot
            // (a panicked gen job) is safe to adopt — `init` is published
            // only after the locals below are fully computed.
            let mut tile = TILE_MEMO[tile_memo_idx(seed, tcx, tcz)]
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if !(tile.init && tile.seed == seed && tile.tcx == tcx && tile.tcz == tcz) {
                let (tx0, tz0) = (tcx * T, tcz * T);
                let bulk = surface.region(tx0, tz0, T as usize, T as usize);
                let mut adj = [0i32; 256];
                for (i, slot) in adj.iter_mut().enumerate() {
                    let wx = tx0 + (i % T as usize) as i32;
                    let wz = tz0 + (i / T as usize) as i32;
                    *slot = caves.feature_surface_after_caves(wx, wz, bulk.surf[i]);
                }
                tile.seed = seed;
                tile.tcx = tcx;
                tile.tcz = tcz;
                tile.raw.copy_from_slice(&bulk.surf);
                tile.adj = adj;
                tile.biomes.copy_from_slice(&bulk.biomes);
                tile.init = true;
            }
            // Copy the tile ∩ window intersection.
            let (ix0, ix1) = (x0.max(tcx * T), x1.min(tcx * T + T));
            let (iz0, iz1) = (z0.max(tcz * T), z1.min(tcz * T + T));
            for wz in iz0..iz1 {
                let trow = ((wz - tcz * T) * T) as usize;
                let rrow = (wz - z0) as usize * w;
                for wx in ix0..ix1 {
                    let ti = trow + (wx - tcx * T) as usize;
                    let ri = rrow + (wx - x0) as usize;
                    region.surf[ri] = tile.adj[ti];
                    region.biomes[ri] = tile.biomes[ti];
                    raw[ri] = tile.raw[ti];
                }
            }
        }
    }

    (region, raw)
}

impl FeatureField for RuntimeFeatureField<'_> {
    fn column_at(&mut self, wx: i32, wz: i32) -> (i32, Biome) {
        debug_assert!(
            region_contains(&self.candidates, wx, wz),
            "feature candidate lookup must stay inside the spacing candidate window"
        );
        self.candidates.at(wx, wz)
    }

    fn surf_at(&mut self, wx: i32, wz: i32) -> i32 {
        if region_contains(&self.candidates, wx, wz) {
            return self.candidates.at(wx, wz).0;
        }
        self.support_surfaces().at(wx, wz)
    }
}

/// A precomputed square surface-height window (the redwood-support halo), shared by
/// the runtime feature field and the cubic per-section field. World-anchored at
/// `(x0,z0)`, `w×w`, row-major.
pub(crate) struct SurfaceHeights {
    x0: i32,
    z0: i32,
    w: usize,
    surf: Vec<i32>,
}

impl SurfaceHeights {
    pub(crate) fn new(x0: i32, z0: i32, w: usize, surf: Vec<i32>) -> Self {
        debug_assert_eq!(surf.len(), w * w);
        Self { x0, z0, w, surf }
    }

    pub(crate) fn at(&self, wx: i32, wz: i32) -> i32 {
        debug_assert!(
            bounds_contains(self.x0, self.z0, self.w, self.w, wx, wz),
            "feature surface support lookup must stay inside the support window"
        );
        let x = (wx - self.x0) as usize;
        let z = (wz - self.z0) as usize;
        self.surf[z * self.w + x]
    }
}

/// Per-section feature field backed by data precomputed ONCE per column (in
/// [`super::driver::ColumnGen`]) and shared, immutably, by every section job of that
/// column. Returns values identical to [`RuntimeFeatureField`] — the candidate region
/// and support surfaces are the same `region`/`surface_heights` queries — but holds no
/// `SurfaceDensitySystem` and does no lazy work, so it is cheap to clone per section
/// and `Send + Sync` for parallel section generation.
pub(crate) struct ColumnFeatureField<'a> {
    candidates: &'a RegionCells,
    /// The redwood-support halo, present only when a redwood-supporting biome is in
    /// range. `surf_at` only reaches outside the candidate window for a redwood support
    /// check, which can only fire when that biome — and hence this window — is present.
    support: Option<&'a SurfaceHeights>,
}

impl<'a> ColumnFeatureField<'a> {
    pub(crate) fn new(candidates: &'a RegionCells, support: Option<&'a SurfaceHeights>) -> Self {
        Self {
            candidates,
            support,
        }
    }
}

impl FeatureField for ColumnFeatureField<'_> {
    fn column_at(&mut self, wx: i32, wz: i32) -> (i32, Biome) {
        debug_assert!(
            region_contains(self.candidates, wx, wz),
            "feature candidate lookup must stay inside the spacing candidate window"
        );
        self.candidates.at(wx, wz)
    }

    fn surf_at(&mut self, wx: i32, wz: i32) -> i32 {
        if region_contains(self.candidates, wx, wz) {
            return self.candidates.at(wx, wz).0;
        }
        self.support
            .expect("redwood support window is present whenever a redwood support check reaches outside the candidate window")
            .at(wx, wz)
    }
}

fn region_contains(region: &RegionCells, wx: i32, wz: i32) -> bool {
    bounds_contains(region.x0, region.z0, region.w, region.h, wx, wz)
}

fn bounds_contains(x0: i32, z0: i32, w: usize, h: usize, wx: i32, wz: i32) -> bool {
    let x = i64::from(wx) - i64::from(x0);
    let z = i64::from(wz) - i64::from(z0);
    x >= 0 && z >= 0 && x < w as i64 && z < h as i64
}
