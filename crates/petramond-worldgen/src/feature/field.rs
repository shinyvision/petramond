use std::sync::Arc;

use petramond_world::biome::Biome;
use petramond_world::chunk::CHUNK_SX;

use super::super::density::surface::SurfaceDensitySystem;
use super::super::region::RegionCells;

pub trait FeatureField {
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

/// One memoized 16×16 world tile of the feature windows: raw surfaces,
/// cave-adjusted surfaces, and biomes.
pub(crate) struct RegionTile {
    raw: [i32; 256],
    adj: [i32; 256],
    biomes: [Biome; 256],
}

/// A tile's identity: the cave field's context (the seed, and the catalogs
/// the adjusted surfaces read) and the tile's chunk coordinates. `surface`
/// is always the density system of that seed — a generator's own pair of
/// sources — so the key names every input.
pub(crate) type TileKey = (crate::cache::GenContext, [i32; 2]);

/// One memo tile, derived once for every worker that needs it (the world's
/// `terrain.surface_tiles` memo is single-flight): tiles are pure functions of
/// their key, so any worker's computation serves every other. Per-thread
/// memos made every pool worker recompute the same nearby tiles — at world
/// open ~20 cold workers each paid the whole spawn area's tile bill.
fn cached_tile(
    surface: &SurfaceDensitySystem,
    caves: &crate::noise::cave_field::CaveField,
    tcx: i32,
    tcz: i32,
) -> Arc<RegionTile> {
    const T: i32 = CHUNK_SX as i32;
    let key = (caves.context(), [tcx, tcz]);
    caves.caches().terrain.surface_tiles.get_or_insert(key, || {
        let (tx0, tz0) = (tcx * T, tcz * T);
        let bulk = surface.region(tx0, tz0, T as usize, T as usize);
        let mut tile = RegionTile {
            raw: [0; 256],
            adj: [0; 256],
            biomes: [Biome::OCEAN; 256],
        };
        tile.adj
            .copy_from_slice(&caves.feature_surfaces_after_caves(tx0, tz0, &bulk.surf));
        tile.raw.copy_from_slice(&bulk.surf);
        tile.biomes.copy_from_slice(&bulk.biomes);
        Arc::new(tile)
    })
}

/// The RAW (pre-cave) density surfaces and biomes of one 16×16 tile, `z*16 +
/// x` — what the terrain fill of the tile's column reads.
pub(crate) fn cached_tile_raw(
    surface: &SurfaceDensitySystem,
    caves: &crate::noise::cave_field::CaveField,
    tcx: i32,
    tcz: i32,
) -> ([i32; 256], [Biome; 256]) {
    let tile = cached_tile(surface, caves, tcx, tcz);
    (tile.raw, tile.biomes)
}

/// The BIOMES of one memo tile, and nothing else. The point-query twin of
/// [`cached_feature_region`]: reading one column through that helper allocates
/// a surface vector and a raw vector to throw both away.
pub fn cached_tile_biomes(
    surface: &SurfaceDensitySystem,
    caves: &crate::noise::cave_field::CaveField,
    tcx: i32,
    tcz: i32,
) -> [Biome; 256] {
    cached_tile(surface, caves, tcx, tcz).biomes
}

/// The feature window for `(x0,z0,w,h)`: cave-adjusted surfaces + biomes in
/// the returned [`RegionCells`], plus the RAW (pre-adjustment) surfaces the
/// column core needs. Assembled from memoized 16×16 world tiles:
/// neighbouring chunks' candidate windows overlap ~18×, and this window
/// build dominated whole-world generation (~75%, 2026-07-13) before the
/// memo. Tiles are the memo unit because windows are chunk-aligned — every
/// window covers whole tiles, so a tile computes once and is copied ever
/// after (per-cell memoization died by scattered evictions defeating bulk
/// recomputation).
///
/// Byte-identical by construction: a tile is keyed by exact
/// `(context, tile coords)` and every value is a pure world-anchored
/// function of that key (the density lattice's corner grid is world-anchored,
/// so region bounds don't affect per-column results) — the memo can only
/// dedupe work.
/// `cached_feature_windows_match_the_uncached_region` pins this against an
/// uncached reference.
pub fn cached_feature_region(
    surface: &SurfaceDensitySystem,
    caves: &crate::noise::cave_field::CaveField,
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
            let tile = cached_tile(surface, caves, tcx, tcz);
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

/// A precomputed square surface-height window (the redwood-support halo) of the
/// per-section feature field. World-anchored at
/// `(x0,z0)`, `w×w`, row-major.
pub struct SurfaceHeights {
    x0: i32,
    z0: i32,
    w: usize,
    surf: Vec<i32>,
}

impl SurfaceHeights {
    pub fn new(x0: i32, z0: i32, w: usize, surf: Vec<i32>) -> Self {
        debug_assert_eq!(surf.len(), w * w);
        Self { x0, z0, w, surf }
    }

    pub fn capacity_bytes(&self) -> usize {
        std::mem::size_of::<Self>() + self.surf.capacity() * 4
    }

    pub fn at(&self, wx: i32, wz: i32) -> i32 {
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
/// `super::driver::ColumnGen`) and shared, immutably, by every section job of that
/// column. The candidate region and support surfaces come from the shared window
/// memo ([`cached_feature_region`]); the field holds no `SurfaceDensitySystem` and
/// does no lazy work, so it is cheap per section and `Send + Sync` for parallel
/// section generation.
pub struct ColumnFeatureField<'a> {
    candidates: &'a RegionCells,
    /// The redwood-support halo, present only when a redwood-supporting biome is in
    /// range. `surf_at` only reaches outside the candidate window for a redwood support
    /// check, which can only fire when that biome — and hence this window — is present.
    support: Option<&'a SurfaceHeights>,
}

impl<'a> ColumnFeatureField<'a> {
    pub fn new(candidates: &'a RegionCells, support: Option<&'a SurfaceHeights>) -> Self {
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
