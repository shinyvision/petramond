//! Composable feature system — replaces the bespoke `trees::oak_*` functions.
//!
//! A feature is split into reusable, data-driven pieces:
//!   - `Feature`        — the imperative voxel-writing shape (e.g. `TreeFeature`)
//!   - `TrunkPlacer` / `FoliagePlacer` — reusable sub-shapes a tree composes
//!   - `ConfiguredFeature` — a feature + baked params (the oaks are rows)
//!
//! Strata P3: the abstraction is established and the oaks become data, but the
//! per-column placement loop reproduces the god file's exact two-roll
//! (`tree_probability` chance → `pick_oak_variant` `next_i32(0,99)`) and every
//! placer mirrors its original RNG draw order and block-write order, so output
//! is byte-parity under the unchanged per-chunk xorshift64 stream.

pub mod placers;
pub mod scatter;
pub mod tree;
pub mod vegetation;

use crate::biome::Biome;
use crate::block::Block;
use crate::chunk::{Chunk, CHUNK_SX, CHUNK_SY, CHUNK_SZ, SEA_LEVEL, SECTION_SIZE};
use crate::mathh::IVec3;
use crate::section::Section;

use self::tree::{redwood_base_trunk_contains, REDWOOD_BASE_SUPPORT_REACH};
use super::biome::{self, spec, TreeSupport};
use super::density::surface::SurfaceDensitySystem;
use super::region::RegionCells;
use super::rng::FeatureRng;

/// Highest surface a tree will root on — above this (bare snow/stone peaks) the
/// canopy is left off regardless of biome.
pub(crate) const TREELINE: i32 = 118;

/// Worst-case vertical reach of a tree ABOVE its root anchor, used to bound which
/// cubic sections a column's features can touch. The tallest tree (redwood) has a
/// height-clearance of 56; the crown / leaf blobs add a few more, so 64 is a safe
/// over-estimate. Trees never write BELOW their anchor (every trunk placer starts at
/// the anchor and builds up), so there is no matching downward reach.
pub(crate) const MAX_TREE_REACH_ABOVE: i32 = 64;

pub(crate) fn feature_region_bounds(ox: i32, oz: i32) -> (i32, i32, usize, usize) {
    let pad = super::proto::MARGIN + biome::MAX_TREE_SPACING_RADIUS + REDWOOD_BASE_SUPPORT_REACH;
    feature_bounds_with_pad(ox, oz, pad)
}

pub(crate) fn feature_candidate_bounds(ox: i32, oz: i32) -> (i32, i32, usize, usize) {
    let pad = super::proto::MARGIN + biome::MAX_TREE_SPACING_RADIUS;
    feature_bounds_with_pad(ox, oz, pad)
}

fn feature_bounds_with_pad(ox: i32, oz: i32, pad: i32) -> (i32, i32, usize, usize) {
    let w = (CHUNK_SX as i32 + 2 * pad) as usize;
    (ox - pad, oz - pad, w, w)
}

/// A worldgen feature: imperatively writes voxels around a world origin.
pub trait Feature: Send + Sync {
    fn generate(&self, ctx: &mut FeatureCtx, origin: IVec3, rng: &mut FeatureRng);

    /// Ground-anchoring gate, consulted for an ACCEPTED origin just before
    /// `generate`: return false to skip the feature at this site entirely
    /// (e.g. oak roots that would hang over a drop). `surf` is the
    /// cave-adjusted generation surface per column; `rng` is a COPY of the
    /// stream `generate` will receive (positioned right after the variant
    /// pick), so an implementation may dry-run its draw prefix. Must read
    /// only `surf` and the rng — never chunk content — and must stay within
    /// `MAX_TREE_SPACING_RADIUS` of the origin so the candidate window covers
    /// every read on both placement paths. Default: anchored everywhere.
    fn is_anchored(
        &self,
        surf: &mut dyn FnMut(i32, i32) -> i32,
        origin: IVec3,
        rng: FeatureRng,
    ) -> bool {
        let _ = (surf, origin, rng);
        true
    }
}

/// A feature plus its baked parameters.
pub struct ConfiguredFeature {
    pub feature: &'static dyn Feature,
}

/// A destination a feature paints voxels into. Abstracting WHERE the writes land
/// lets the SAME `Feature` / placer code drive two callers: worldgen, which writes
/// into one [`Chunk`] clipped to its footprint ([`ChunkSink`]), and runtime sapling
/// growth, which writes into the live `World` through a validating overlay (see
/// `world::sapling`). `get` returns the sink's CURRENT occupant so the overwrite
/// predicates on [`FeatureCtx`] see a feature's own earlier writes; it reads `Air`
/// for any cell the sink can't address.
pub trait VoxelSink {
    fn get(&self, p: IVec3) -> Block;
    fn set(&mut self, p: IVec3, b: Block);
}

/// Bulk voxel storage a [`ClippedSink`] clips into: a world-anchored writable
/// box plus raw local-index accessors.
pub trait SinkTarget {
    /// `(min world corner, size in blocks)` of the writable footprint.
    fn world_box(&self) -> (IVec3, IVec3);
    fn block(&self, x: usize, y: usize, z: usize) -> Block;
    fn set_block_raw(&mut self, x: usize, y: usize, z: usize, id: u8);
}

/// Worldgen voxel sink: writes into one [`SinkTarget`], in WORLD coords clipped to
/// the target's own footprint. Out-of-footprint writes are dropped and
/// out-of-footprint reads return `Air`. That clipping IS the seam mechanism:
/// every retained write predicates only on the cell it writes (`set_leaf`/
/// `set_branch` read `get(p)` at the same `p`), never a neighbour, so a feature
/// rooted anywhere materialises its overlapping voxels identically whether they
/// land in the owner target or a neighbour — seam-consistent cross-boundary
/// features with no shared buffer.
pub struct ClippedSink<'a, T: SinkTarget> {
    target: &'a mut T,
    origin: IVec3,
    size: IVec3,
}

impl<'a, T: SinkTarget> ClippedSink<'a, T> {
    pub fn new(target: &'a mut T) -> Self {
        let (origin, size) = target.world_box();
        Self {
            target,
            origin,
            size,
        }
    }

    /// Map a world position to in-footprint local indices, or `None` if outside.
    #[inline]
    fn local(&self, p: IVec3) -> Option<(usize, usize, usize)> {
        let l = p - self.origin;
        if l.cmpge(IVec3::ZERO).all() && l.cmplt(self.size).all() {
            Some((l.x as usize, l.y as usize, l.z as usize))
        } else {
            None
        }
    }
}

impl<T: SinkTarget> VoxelSink for ClippedSink<'_, T> {
    #[inline]
    fn get(&self, p: IVec3) -> Block {
        match self.local(p) {
            Some((x, y, z)) => self.target.block(x, y, z),
            None => Block::Air,
        }
    }
    #[inline]
    fn set(&mut self, p: IVec3, b: Block) {
        if let Some((x, y, z)) = self.local(p) {
            self.target.set_block_raw(x, y, z, b.id());
        }
    }
}

impl SinkTarget for Chunk {
    fn world_box(&self) -> (IVec3, IVec3) {
        let (ox, oz) = self.chunk_origin_world();
        let size = IVec3::new(CHUNK_SX as i32, CHUNK_SY as i32, CHUNK_SZ as i32);
        (IVec3::new(ox, 0, oz), size)
    }
    #[inline]
    fn block(&self, x: usize, y: usize, z: usize) -> Block {
        Chunk::block(self, x, y, z)
    }
    #[inline]
    fn set_block_raw(&mut self, x: usize, y: usize, z: usize, id: u8) {
        Chunk::set_block_raw(self, x, y, z, id);
    }
}

impl SinkTarget for Section {
    fn world_box(&self) -> (IVec3, IVec3) {
        let (ox, oy, oz) = self.origin_world();
        (IVec3::new(ox, oy, oz), IVec3::splat(SECTION_SIZE as i32))
    }
    #[inline]
    fn block(&self, x: usize, y: usize, z: usize) -> Block {
        Section::block(self, x, y, z)
    }
    #[inline]
    fn set_block_raw(&mut self, x: usize, y: usize, z: usize, id: u8) {
        Section::set_block_raw(self, x, y, z, id);
    }
}

/// [`ClippedSink`] over one [`Chunk`]'s `[0,16)×[0,256)×[0,16)` footprint —
/// seam-consistent cross-chunk features with no shared buffer.
pub type ChunkSink<'a> = ClippedSink<'a, Chunk>;

/// [`ClippedSink`] over one 16³ [`Section`] for the cubic path — the same seam
/// mechanism in 3D, so a feature materialises its in-section voxels identically
/// whether the section is generated alone or as part of a whole column, across
/// VERTICAL seams as well as horizontal ones.
pub type SectionSink<'a> = ClippedSink<'a, Section>;

/// Apply a mod worldgen hook's write list (world position, registered block
/// id) to one section through the SAME clipping sink engine features use —
/// out-of-section writes drop, in-section writes go through the counted
/// setter. That clip is the mod-feature seam mechanism: every section
/// materialises exactly its own slice of a cross-boundary feature.
pub(crate) fn apply_gen_writes(section: &mut Section, writes: &[([i32; 3], u8)]) {
    let mut sink = SectionSink::new(section);
    for &([x, y, z], id) in writes {
        sink.set(IVec3::new(x, y, z), Block(id));
    }
}

/// Bounded voxel writer — the ONLY place imperative feature writes happen. Holds a
/// `&mut dyn VoxelSink` so one set of placer code targets either a chunk (worldgen)
/// or the world (growth). The overwrite predicates (`set_leaf` over air/water,
/// `set_branch` over air/leaves/water, `replace_block` over an expected block) read
/// the sink's CURRENT occupant, so a feature's own earlier writes are honoured.
/// Reproduces the god file's three overwrite predicates
/// (`log_at`/`leaf_at`/`oak_big`-branch).
pub struct FeatureCtx<'a> {
    sink: &'a mut dyn VoxelSink,
}

impl<'a> FeatureCtx<'a> {
    pub fn new(sink: &'a mut dyn VoxelSink) -> Self {
        Self { sink }
    }

    /// Unconditional write (== `trees::log_at`).
    pub fn set_log(&mut self, p: IVec3, b: Block) {
        self.sink.set(p, b);
    }

    /// Write over Air/Water only (== `trees::leaf_at`).
    pub fn set_leaf(&mut self, p: IVec3, b: Block) {
        let c = self.sink.get(p);
        if c == Block::Air || c == Block::Water {
            self.sink.set(p, b);
        }
    }

    /// Write over Air/leaves/Water (== branch predicate). A branch may pass
    /// through leaves placed earlier by its own crown or a neighbouring canopy.
    pub fn set_branch(&mut self, p: IVec3, b: Block) {
        let c = self.sink.get(p);
        if c == Block::Air || c.is_leaves() || c == Block::Water {
            self.sink.set(p, b);
        }
    }

    /// Replace a voxel only when it currently equals `expect`. Used by the
    /// underground ore / stone-blob veins, which overwrite Stone (and never air,
    /// dirt, or an already-placed ore). World coords; clipped to this chunk.
    pub fn replace_block(&mut self, p: IVec3, expect: Block, b: Block) {
        if self.sink.get(p) == expect {
            self.sink.set(p, b);
        }
    }
}

/// Salt distinguishing the tree-feature positional RNG stream from other users.
const FEATURE_SALT: u64 = 0x0000_7A3E_0AC0_FFEE;
/// Separate stream used only to break ties between nearby tree candidates.
const TREE_PRIORITY_SALT: u64 = 0x0000_7A3E_51AC_1EAF;

#[derive(Copy, Clone)]
struct TreeCandidate {
    anchor: i32,
    biome: Biome,
    density: f32,
    spacing_radius: i32,
    priority: u64,
}

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

/// Tile count of the per-thread window memo (direct-mapped, ~5 MB/thread).
/// A streaming row touches a few hundred live tiles; 2048 keeps row-to-row
/// revisits hitting.
const TILE_MEMO_BITS: u32 = 11;

thread_local! {
    static TILE_MEMO: std::cell::RefCell<Box<[RegionTile]>> =
        std::cell::RefCell::new(
            vec![
                RegionTile {
                    init: false,
                    seed: 0,
                    tcx: 0,
                    tcz: 0,
                    raw: [0; 256],
                    adj: [0; 256],
                    biomes: [Biome::Ocean; 256],
                };
                1 << TILE_MEMO_BITS
            ]
            .into_boxed_slice(),
        );
}

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

    TILE_MEMO.with(|memo| {
        let mut memo = memo.borrow_mut();
        for tcz in z0.div_euclid(T)..=(z1 - 1).div_euclid(T) {
            for tcx in x0.div_euclid(T)..=(x1 - 1).div_euclid(T) {
                let tile = &mut memo[tile_memo_idx(seed, tcx, tcz)];
                if !(tile.init && tile.seed == seed && tile.tcx == tcx && tile.tcz == tcz) {
                    let (tx0, tz0) = (tcx * T, tcz * T);
                    let bulk = surface.region(tx0, tz0, T as usize, T as usize);
                    tile.init = true;
                    tile.seed = seed;
                    tile.tcx = tcx;
                    tile.tcz = tcz;
                    for i in 0..(T * T) as usize {
                        let wx = tx0 + (i % T as usize) as i32;
                        let wz = tz0 + (i / T as usize) as i32;
                        tile.raw[i] = bulk.surf[i];
                        tile.adj[i] = caves.feature_surface_after_caves(wx, wz, bulk.surf[i]);
                        tile.biomes[i] = bulk.biomes[i];
                    }
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
    });

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

#[inline]
fn tree_priority(seed: u32, wx: i32, wz: i32) -> u64 {
    FeatureRng::positional(seed, TREE_PRIORITY_SALT, wx, 0, wz).next_u64()
}

#[inline]
fn tree_candidate_beats(
    lhs_priority: u64,
    lhs_wx: i32,
    lhs_wz: i32,
    rhs_priority: u64,
    rhs_wx: i32,
    rhs_wz: i32,
) -> bool {
    lhs_priority > rhs_priority
        || (lhs_priority == rhs_priority && (lhs_wz, lhs_wx) < (rhs_wz, rhs_wx))
}

fn tree_candidate_at(
    field: &mut impl FeatureField,
    seed: u32,
    wx: i32,
    wz: i32,
) -> Option<TreeCandidate> {
    // Anchor on the final region surface. Ocean and wet river-channel columns sit
    // at/below their waterline, so the water guard keeps trees off them.
    let (surf, biome) = field.column_at(wx, wz);
    let anchor = surf;
    if anchor <= SEA_LEVEL || surf > TREELINE {
        return None;
    }

    let tree = spec(biome).trees;
    // place_oak height guard (origin too low / too near the world top).
    if anchor < 1 || anchor + tree.height_clearance >= CHUNK_SY as i32 {
        return None;
    }

    let density = tree.density;
    if density <= 0.0 {
        return None;
    }

    let mut rng = FeatureRng::positional(seed, FEATURE_SALT, wx, 0, wz);
    if !rng.chance(density) {
        return None;
    }

    match tree.support {
        TreeSupport::None => {}
        TreeSupport::RedwoodBase => {
            if !redwood_trunk_is_supported(field, wx, wz, anchor) {
                return None;
            }
        }
    }

    Some(TreeCandidate {
        anchor,
        biome,
        density,
        spacing_radius: tree.spacing_radius,
        priority: tree_priority(seed, wx, wz),
    })
}

fn tree_spacing_allows(
    candidate: TreeCandidate,
    field: &mut impl FeatureField,
    seed: u32,
    wx: i32,
    wz: i32,
) -> bool {
    for dz in -biome::MAX_TREE_SPACING_RADIUS..=biome::MAX_TREE_SPACING_RADIUS {
        for dx in -biome::MAX_TREE_SPACING_RADIUS..=biome::MAX_TREE_SPACING_RADIUS {
            if dx == 0 && dz == 0 {
                continue;
            }
            let nx = wx + dx;
            let nz = wz + dz;
            if let Some(other) = tree_candidate_at(field, seed, nx, nz) {
                let spacing = candidate.spacing_radius.max(other.spacing_radius);
                if dx.abs() > spacing || dz.abs() > spacing {
                    continue;
                }
                if tree_candidate_beats(other.priority, nx, nz, candidate.priority, wx, wz) {
                    return false;
                }
            }
        }
    }
    true
}

fn redwood_trunk_is_supported(
    field: &mut impl FeatureField,
    wx: i32,
    wz: i32,
    anchor: i32,
) -> bool {
    for dz in -REDWOOD_BASE_SUPPORT_REACH..=REDWOOD_BASE_SUPPORT_REACH {
        for dx in -REDWOOD_BASE_SUPPORT_REACH..=REDWOOD_BASE_SUPPORT_REACH {
            if !redwood_base_trunk_contains(dx, dz) {
                continue;
            }
            let support_surf = field.surf_at(wx + dx, wz + dz);
            if support_surf < anchor - 1 {
                return false;
            }
        }
    }
    true
}

/// Per-chunk feature placement (P4). Iterates feature origins across the chunk
/// plus a `MARGIN` border, in canonical (wz, wx) order, so a tree rooted in a
/// neighbour that reaches into this chunk is generated here too. Each origin
/// seeds its OWN positional RNG (`FeatureRng::positional`), so the per-biome
/// density roll, variant pick, and geometry are pure functions of (seed, wx, wz)
/// — independent of chunk and order. Candidate origins are then thinned by a
/// deterministic configured spacing rule. Features write in world coords and
/// are clipped to this chunk, so seams are continuous with no double-placement
/// and the old chunk-edge skip is gone.
pub(crate) fn place_features_with_field(
    chunk: &mut Chunk,
    field: &mut impl FeatureField,
    seed: u32,
) {
    let (ox, oz) = chunk.chunk_origin_world();
    let mut sink = ChunkSink::new(chunk);
    let mut ctx = FeatureCtx::new(&mut sink);
    place_feature_origins(&mut ctx, field, seed, ox, oz);
}

/// Cubic per-section feature placement: run the SAME origin loop into one 16³
/// [`Section`] through a [`SectionSink`]. Because each feature write predicates only
/// on its own cell, the section's voxels come out byte-identical to what the
/// whole-column [`place_features_with_field`] would write there — for the section's
/// own vertical slab, with no neighbour buffer. `field` covers this section's column
/// (origin `ox,oz = section column origin`) plus the feature margin.
pub(crate) fn place_features_section(
    section: &mut Section,
    field: &mut impl FeatureField,
    seed: u32,
) {
    let (ox, _oy, oz) = section.origin_world();
    let mut sink = SectionSink::new(section);
    let mut ctx = FeatureCtx::new(&mut sink);
    place_feature_origins(&mut ctx, field, seed, ox, oz);
}

/// The shared feature origin loop: iterate candidate origins across one column's XZ
/// footprint plus a `MARGIN` border, thin by the spacing rule, and generate each
/// accepted tree into `ctx` (whose sink clips to wherever the caller is writing —
/// a chunk or one section). `ox,oz` is the column's world origin.
fn place_feature_origins(
    ctx: &mut FeatureCtx,
    field: &mut impl FeatureField,
    seed: u32,
    ox: i32,
    oz: i32,
) {
    let margin = super::proto::MARGIN;
    for wz in (oz - margin)..(oz + CHUNK_SZ as i32 + margin) {
        for wx in (ox - margin)..(ox + CHUNK_SX as i32 + margin) {
            let Some(candidate) = tree_candidate_at(field, seed, wx, wz) else {
                continue;
            };

            if !tree_spacing_allows(candidate, field, seed, wx, wz) {
                continue;
            }

            // Recreate the accepted origin's stream and consume the already-proven
            // density roll so variant and geometry draws stay on the tree stream.
            let mut rng = FeatureRng::positional(seed, FEATURE_SALT, wx, 0, wz);
            let _density_hit = rng.chance(candidate.density);
            debug_assert!(_density_hit);
            let cf = (spec(candidate.biome).trees.picker)(&mut rng);
            let origin = IVec3::new(wx, candidate.anchor, wz);
            // Ground-anchoring gate, on the accepted origin only. Spacing-scan
            // neighbours are NOT gated, so an unanchorable neighbour still
            // suppresses candidates around it — deterministic either way, and
            // it keeps the gate's surface reads inside the candidate window
            // (origins lie within MARGIN of the chunk; the gate adds at most
            // MAX_TREE_SPACING_RADIUS). Every chunk replaying this origin
            // reaches the same verdict: the window values are world-anchored.
            if !cf
                .feature
                .is_anchored(&mut |sx, sz| field.surf_at(sx, sz), origin, rng)
            {
                continue;
            }
            cf.feature.generate(ctx, origin, &mut rng);
        }
    }
}

#[cfg(all(test, feature = "worldgen-tests"))]
mod tests {
    use super::super::proto::MARGIN;
    use super::{
        feature_region_bounds, place_features_with_field, tree_candidate_at, tree_spacing_allows,
        RuntimeFeatureField,
    };
    use crate::biome::Biome;
    use crate::block::Block;
    use crate::chunk::{Chunk, CHUNK_SX, CHUNK_SY, CHUNK_SZ};
    use crate::worldgen::density::surface::SurfaceDensitySystem;
    use crate::worldgen::generate_chunk;
    use crate::worldgen::region::RegionCells;

    fn is_tree(id: u8) -> bool {
        let block = Block::from_id(id);
        block.is_log() || block.is_leaves()
    }

    fn synthetic_tree_region(x0: i32, z0: i32, w: usize, h: usize) -> RegionCells {
        let mut region = RegionCells::new(x0, z0, w, h);
        region.surf.fill(70);
        region.biomes.fill(Biome::RedwoodForest);
        region
    }

    /// Unclipped write collector: trees now overhang a single chunk (footprint
    /// up to MARGIN), so the shape invariants below inspect the feature's FULL
    /// write set instead of one chunk's clipped slice.
    struct MapSink(std::collections::HashMap<crate::mathh::IVec3, Block>);

    impl super::VoxelSink for MapSink {
        fn get(&self, p: crate::mathh::IVec3) -> Block {
            self.0.get(&p).copied().unwrap_or(Block::Air)
        }
        fn set(&mut self, p: crate::mathh::IVec3, b: Block) {
            self.0.insert(p, b);
        }
    }

    fn generate_into_map(
        feat: &'static super::ConfiguredFeature,
        seed: u32,
    ) -> std::collections::HashMap<crate::mathh::IVec3, Block> {
        use crate::mathh::IVec3;
        use crate::worldgen::rng::FeatureRng;
        let mut sink = MapSink(std::collections::HashMap::new());
        let mut rng = FeatureRng::positional(seed, 0xACAC, 0, 0, 0);
        {
            let mut ctx = super::FeatureCtx::new(&mut sink);
            feat.feature.generate(&mut ctx, IVec3::new(0, 64, 0), &mut rng);
        }
        sink.0
    }

    /// Every leaf a configured tree places must reach one of its logs within
    /// `MAX_LOG_DISTANCE` FACE-steps travelling only through leaves — the exact
    /// rule `block::behavior::leaves` decays against. Diagonal-only attachment (the
    /// acacia umbrella bug) does not count, so this guards against canopies that
    /// silently rot after generation.
    #[test]
    fn configured_trees_place_only_orthogonally_supported_leaves() {
        use crate::worldgen::data::features::{
            ACACIA, OAK_BIG, OAK_SMALL, OAK_YOUNG, REDWOOD, SPRUCE,
        };
        use std::collections::{HashSet, VecDeque};

        const MAX_LOG_DISTANCE: i32 = 6; // mirrors block::behavior::leaves
        const FACES: [(i32, i32, i32); 6] = [
            (1, 0, 0),
            (-1, 0, 0),
            (0, 1, 0),
            (0, -1, 0),
            (0, 0, 1),
            (0, 0, -1),
        ];

        for (name, feat) in [
            ("acacia", &ACACIA),
            ("oak_young", &OAK_YOUNG),
            ("oak_small", &OAK_SMALL),
            ("oak_big", &OAK_BIG),
            ("spruce", &SPRUCE),
            ("redwood", &REDWOOD),
        ] {
            for seed in [1u32, 7, 42, 99, 1000, 31337] {
                let map = generate_into_map(feat, seed);
                let mut leaves = HashSet::new();
                let mut logs = HashSet::new();
                for (p, b) in &map {
                    if b.is_leaves() {
                        leaves.insert((p.x, p.y, p.z));
                    } else if b.is_log() {
                        logs.insert((p.x, p.y, p.z));
                    }
                }
                assert!(!leaves.is_empty(), "{name} seed {seed}: placed no leaves");

                for &start in &leaves {
                    let mut visited = HashSet::from([start]);
                    let mut frontier = VecDeque::from([(start, 0)]);
                    let mut supported = false;
                    'bfs: while let Some(((sx, sy, sz), dist)) = frontier.pop_front() {
                        for (dx, dy, dz) in FACES {
                            let n = (sx + dx, sy + dy, sz + dz);
                            if logs.contains(&n) {
                                supported = true;
                                break 'bfs;
                            }
                            if dist + 1 < MAX_LOG_DISTANCE
                                && leaves.contains(&n)
                                && visited.insert(n)
                            {
                                frontier.push_back((n, dist + 1));
                            }
                        }
                    }
                    assert!(
                        supported,
                        "{name} seed {seed}: leaf at {start:?} only diagonally attached — it would decay"
                    );
                }
            }
        }
    }

    /// Seen from straight above, an oak's trunk top must end in leaves, never
    /// a bare log end — the exposed-top-log artifact playtesting flagged
    /// (2026-07-12). The trunk centre wanders within ±1 of the origin, so the
    /// tallest log column in that window is the trunk top; its column must
    /// hold a leaf above the log.
    #[test]
    fn oak_crowns_bury_the_trunk_top() {
        use crate::worldgen::data::features::{OAK_BIG, OAK_SMALL, OAK_YOUNG};

        for (name, feat) in [
            ("oak_young", &OAK_YOUNG),
            ("oak_small", &OAK_SMALL),
            ("oak_big", &OAK_BIG),
        ] {
            for seed in [1u32, 7, 42, 99, 1000, 31337] {
                let map = generate_into_map(feat, seed);
                let top_log = |x: i32, z: i32| {
                    map.iter()
                        .filter(|(p, b)| p.x == x && p.z == z && b.is_log())
                        .map(|(p, _)| p.y)
                        .max()
                };
                let best = (-1..=1)
                    .flat_map(|dx| (-1..=1).map(move |dz| (dx, dz)))
                    .filter_map(|(x, z)| top_log(x, z).map(|y| (x, z, y)))
                    .max_by_key(|&(_, _, y)| y)
                    .expect("trunk has logs");
                let covered = map.iter().any(|(p, b)| {
                    p.x == best.0 && p.z == best.1 && p.y > best.2 && b.is_leaves()
                });
                assert!(
                    covered,
                    "{name} seed {seed}: bare trunk-top log exposed at column ({}, {}), y {}",
                    best.0, best.1, best.2
                );
            }
        }
    }

    /// The oak anchoring gate: flat ground accepts, a drop under the root
    /// splay rejects the whole tree — the floating-tree guard.
    #[test]
    fn oaks_refuse_sites_where_roots_would_hang() {
        use crate::mathh::IVec3;
        use crate::worldgen::data::features::{OAK_BIG, OAK_SMALL, OAK_YOUNG};
        use crate::worldgen::rng::FeatureRng;

        for (name, feat) in [
            ("oak_young", &OAK_YOUNG),
            ("oak_small", &OAK_SMALL),
            ("oak_big", &OAK_BIG),
        ] {
            for seed in [1u32, 7, 42, 99, 1000, 31337] {
                let origin = IVec3::new(0, 64, 0);
                let rng = FeatureRng::positional(seed, 0xACAC, 0, 0, 0);
                assert!(
                    feat.feature.is_anchored(&mut |_, _| 64, origin, rng),
                    "{name} seed {seed}: flat ground must anchor"
                );
                // Everything but the origin column drops far below: some base
                // cell always lies off-column, so the site must be refused.
                assert!(
                    !feat.feature.is_anchored(
                        &mut |x, z| if x == 0 && z == 0 { 64 } else { 40 },
                        origin,
                        rng,
                    ),
                    "{name} seed {seed}: a cliff under the roots must refuse the site"
                );
            }
        }
    }

    fn accepted_tree_origins(seed: u32, chunk_radius: i32) -> Vec<(i32, i32, i32)> {
        let mut origins = Vec::new();

        for cz in -chunk_radius..=chunk_radius {
            for cx in -chunk_radius..=chunk_radius {
                let ox = cx * CHUNK_SX as i32;
                let oz = cz * CHUNK_SZ as i32;
                let (x0, z0, w, h) = feature_region_bounds(ox, oz);
                let field = synthetic_tree_region(x0, z0, w, h);
                let mut field = &field;
                for wz in oz..(oz + CHUNK_SZ as i32) {
                    for wx in ox..(ox + CHUNK_SX as i32) {
                        let Some(candidate) = tree_candidate_at(&mut field, seed, wx, wz) else {
                            continue;
                        };
                        if tree_spacing_allows(candidate, &mut field, seed, wx, wz) {
                            origins.push((wx, wz, candidate.spacing_radius));
                        }
                    }
                }
            }
        }

        origins
    }

    #[test]
    fn tree_origin_spacing_rule_enforces_configured_radius() {
        for seed in [1u32, 7, 42, 0x1234_5678] {
            let origins = accepted_tree_origins(seed, 2);
            assert!(
                origins.len() > 10,
                "spacing test sampled too few tree origins for seed {seed:#x}"
            );

            for i in 0..origins.len() {
                for j in (i + 1)..origins.len() {
                    let (ax, az, ar) = origins[i];
                    let (bx, bz, br) = origins[j];
                    let dx = (ax - bx).abs();
                    let dz = (az - bz).abs();
                    let required = ar.max(br);
                    assert!(
                        dx > required || dz > required,
                        "tree origins ({ax},{az}) and ({bx},{bz}) are within {required} blocks"
                    );
                }
            }
        }
    }

    #[test]
    fn live_density_feature_region_covers_margin_and_spacing_queries() {
        let seed = 7u32;
        let surface = SurfaceDensitySystem::new(seed);

        for (cx, cz) in [(0, 0), (-2, 1), (4, -3)] {
            let ox = cx * CHUNK_SX as i32;
            let oz = cz * CHUNK_SZ as i32;
            let (x0, z0, w, h) = feature_region_bounds(ox, oz);
            let field = surface.region(x0, z0, w, h);
            let mut field = &field;

            for wz in (oz - MARGIN)..(oz + CHUNK_SZ as i32 + MARGIN) {
                for wx in (ox - MARGIN)..(ox + CHUNK_SX as i32 + MARGIN) {
                    if let Some(candidate) = tree_candidate_at(&mut field, seed, wx, wz) {
                        let _ = tree_spacing_allows(candidate, &mut field, seed, wx, wz);
                    }
                }
            }
        }
    }

    #[test]
    fn runtime_feature_field_matches_full_region_features() {
        let seed = 0x1234_5678;
        let surface = SurfaceDensitySystem::new(seed);
        let caves = crate::worldgen::noise::height::CaveField::new(seed);

        for (cx, cz) in [(0, 0), (-3, 5), (12, -7), (4, -3)] {
            let ox = cx * CHUNK_SX as i32;
            let oz = cz * CHUNK_SZ as i32;
            let (x0, z0, w, h) = feature_region_bounds(ox, oz);
            // The runtime field bakes the cave adjustment into its candidate
            // window, so the reference full-region field gets the same per-cell
            // adjustment before comparing.
            let mut full_region = surface.region(x0, z0, w, h);
            for (i, s) in full_region.surf.iter_mut().enumerate() {
                let wx = full_region.x0 + (i % full_region.w) as i32;
                let wz = full_region.z0 + (i / full_region.w) as i32;
                *s = caves.feature_surface_after_caves(wx, wz, *s);
            }
            let mut full_field = &full_region;

            let mut full_chunk = Chunk::new(cx, cz);
            place_features_with_field(&mut full_chunk, &mut full_field, seed);

            let mut runtime_chunk = Chunk::new(cx, cz);
            let mut field = RuntimeFeatureField::new(&surface, &caves, seed, ox, oz);
            place_features_with_field(&mut runtime_chunk, &mut field, seed);

            assert_eq!(
                full_chunk.blocks_slice(),
                runtime_chunk.blocks_slice(),
                "feature blocks differ at ({cx},{cz})"
            );
        }
    }

    #[test]
    fn generate_chunk_is_deterministic() {
        let seed = 0x1234_5678;
        for &(cx, cz) in &[(0, 0), (3, -2), (-5, 7), (12, 9)] {
            let a = generate_chunk(seed, cx, cz);
            let b = generate_chunk(seed, cx, cz);
            assert_eq!(
                a.blocks_slice(),
                b.blocks_slice(),
                "blocks differ at {cx},{cz}"
            );
            assert_eq!(
                a.biomes_slice(),
                b.biomes_slice(),
                "biomes differ at {cx},{cz}"
            );
        }
    }

    #[test]
    fn features_occupy_chunk_edges() {
        // P4 removed the chunk-edge skip: trees may now sit on the border.
        for seed in [1u32, 7, 42, 0x1234_5678] {
            let mut c = Chunk::new(0, 0);
            let (x0, z0, w, h) = feature_region_bounds(0, 0);
            let field = synthetic_tree_region(x0, z0, w, h);
            let mut field = &field;
            place_features_with_field(&mut c, &mut field, seed);

            for z in 0..CHUNK_SZ {
                for x in 0..CHUNK_SX {
                    let edge = x == 0 || x == CHUNK_SX - 1 || z == 0 || z == CHUNK_SZ - 1;
                    if !edge {
                        continue;
                    }
                    for y in 0..CHUNK_SY {
                        if is_tree(c.block_raw(x, y, z)) {
                            return;
                        }
                    }
                }
            }
        }
        panic!("no tree blocks on any chunk edge — edge-skip not removed?");
    }

    #[test]
    fn trees_span_chunk_seams() {
        // A trunk rooted on the west border of chunk (cx,cz) (world x = cx*16)
        // must have canopy reaching into the previous chunk's east column
        // (local x = 15). Any one confirmed seam-spanning tree proves the
        // cross-chunk feature mechanism (no bald seam, no gap).
        for seed in [1u32, 7, 13, 42, 0x1234_5678] {
            for cz in 0..6 {
                for cx in 1..6 {
                    let west = generate_chunk(seed, cx - 1, cz);
                    let east = generate_chunk(seed, cx, cz);
                    for z in 0..CHUNK_SZ {
                        for y in 2..CHUNK_SY - 2 {
                            if east.block_raw(0, y, z) != Block::OakLog.id() {
                                continue;
                            }
                            // Canopy of this trunk should reach the west chunk's
                            // x = 15 column near (y.., z..).
                            let z_lo = z.saturating_sub(2);
                            let z_hi = (z + 3).min(CHUNK_SZ);
                            for yy in y..(y + 8).min(CHUNK_SY) {
                                for zz in z_lo..z_hi {
                                    if is_tree(west.block_raw(15, yy, zz)) {
                                        return; // seam-spanning tree confirmed
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        panic!("no seam-spanning tree found in the sampled region");
    }
}
