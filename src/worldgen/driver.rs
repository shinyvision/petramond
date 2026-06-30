//! `ChunkGenerator` — owns the worldgen subsystems and runs the fixed stage
//! order for one chunk.
//!
//! Hot stages: Setup → SurfaceDensityFill → Caves → Underground → Vegetation →
//! Features.
//! The older full surface region path remains available for diagnostics and
//! tooling that needs a materialized feature/audit window.
//!
//! The generator holds only immutable wiring built from `seed` (no interior
//! mutability). Output is therefore a pure function of `(seed, cx, cz)`,
//! independent of thread or call order.

use crate::chunk::{Chunk, SectionPos, CHUNK_SX, CHUNK_SZ, SEA_LEVEL, SECTION_SIZE};
use crate::section::{Section, SectionSummary};

use super::density::surface::SurfaceDensitySystem;
use super::feature::{
    feature_candidate_bounds, feature_region_bounds, place_features_section,
    scatter::{self, SCATTER_MAX_Y, SCATTER_MIN_Y},
    vegetation, ColumnFeatureField, FeatureField, RuntimeFeatureField, SurfaceHeights,
    MAX_TREE_REACH_ABOVE, TREELINE,
};
use super::noise::height::CaveField;
use super::proto::ProtoChunk;
use super::region::RegionCells;

pub struct ChunkGenerator {
    seed: u32,
    surface_density: SurfaceDensitySystem,
    caves: CaveField,
}

/// Per-column data computed ONCE on the worker, then shared (via `Arc`) by every
/// per-section job of that column. Holds the column's biome + density surface
/// (`16×16`), plus the precomputed feature candidate region and redwood-support
/// surfaces so each section's tree pass does no lattice work. Pure function of
/// `(seed, cx, cz)`; `Send + Sync` (no interior mutability).
///
/// This is the seam between the cheap, inherently-2D part of worldgen (one heavy
/// job) and the per-16³ terrain/feature fill (many cheap jobs), so generation can
/// run closest to the player one section at a time — including below y=0 (room for
/// caves) without ever building a 256-tall column.
pub struct ColumnGen {
    pub cx: i32,
    pub cz: i32,
    /// Biome id per `(x,z)` in the column's 16×16, indexed `z*16 + x`.
    biome: Box<[u8]>,
    /// Density top-solid surface (world Y, or `-1` for a floorless column) per
    /// `(x,z)`, indexed `z*16 + x`.
    surf: Box<[i32]>,
    /// Post-cave bare top non-air surface for the column's local `(x,z)`, before
    /// vegetation/trees. This is lower than `surf` only at cave entrances.
    top_surf: Box<[i32]>,
    surf_min: i32,
    surf_max: i32,
    /// Surface min/max across the whole candidate window (chunk + spacing margin), so
    /// tree gating accounts for anchors at margin origins and content reaching in from
    /// neighbours, not just this 16×16.
    cand_surf_min: i32,
    cand_surf_max: i32,
    /// Highest world Y that can hold any generated block in this column — the candidate
    /// window's tallest surface plus the maximum tree reach. Sections whose floor is
    /// above this are provably all-air sky, so the streamer skips generating them.
    content_top: i32,
    /// Feature candidate window (chunk + spacing margin): surfaces + biomes for the
    /// tree density/spacing rolls.
    candidates: RegionCells,
    /// Redwood-support surface window (chunk + the larger support margin). `None` unless
    /// the candidate window actually contains a redwood-supporting biome — the only
    /// consumer is `redwood_trunk_is_supported`, so most columns never compute this
    /// (otherwise-eager) larger noise window.
    support: Option<SurfaceHeights>,
}

impl ColumnGen {
    #[inline]
    pub fn cx(&self) -> i32 {
        self.cx
    }
    #[inline]
    pub fn cz(&self) -> i32 {
        self.cz
    }
    /// Biome id at column-local `(x,z)`.
    #[inline]
    pub fn biome_at(&self, x: usize, z: usize) -> u8 {
        self.biome[z * SECTION_SIZE + x]
    }
    /// Density top-solid surface (world Y, or `-1`) at column-local `(x,z)`.
    #[inline]
    pub fn surface_y(&self, x: usize, z: usize) -> i32 {
        self.surf[z * SECTION_SIZE + x]
    }
    /// Generated column heightmap before vegetation/trees: waterline for submerged
    /// columns, otherwise the post-cave top surface so skylight can enter mouths.
    #[inline]
    pub fn heightmap_surface_y(&self, x: usize, z: usize) -> i32 {
        let i = z * SECTION_SIZE + x;
        if self.surf[i] < SEA_LEVEL {
            SEA_LEVEL
        } else {
            self.top_surf[i]
        }
    }
    /// Lowest / highest density surface across the column (for vertical-window sizing).
    #[inline]
    pub fn surf_range(&self) -> (i32, i32) {
        (self.surf_min, self.surf_max)
    }
    /// Highest world Y any generated block in this column can occupy (surface + tree
    /// reach). Sections whose floor exceeds this are all-air sky.
    #[inline]
    pub fn content_top(&self) -> i32 {
        self.content_top
    }

    /// Conservative occupancy summary for a generated section that may not be
    /// materialized yet. This is cheap enough for streaming/meshing decisions and avoids
    /// generating deep stone just to learn that it is fully solid.
    #[inline]
    pub fn section_summary(&self, cy: i32) -> SectionSummary {
        if !SectionPos::cy_in_range(cy) {
            return SectionSummary::Unknown;
        }
        let y0 = cy * SECTION_SIZE as i32;
        let y1 = y0 + SECTION_SIZE as i32 - 1;
        if y0 > self.content_top {
            return SectionSummary::Empty;
        }
        if CaveField::section_may_carve(cy, self.surf_min, self.surf_max) {
            return SectionSummary::Mixed;
        }
        if y1 <= self.surf_min {
            return SectionSummary::FullOpaque;
        }
        if y0 > self.surf_max && y1 <= SEA_LEVEL {
            return SectionSummary::FullWater;
        }
        SectionSummary::Mixed
    }
}

#[inline]
fn ranges_overlap(a_lo: i32, a_hi: i32, b_lo: i32, b_hi: i32) -> bool {
    a_lo <= b_hi && b_lo <= a_hi
}

impl ChunkGenerator {
    pub fn new(seed: u32) -> Self {
        Self {
            seed,
            surface_density: SurfaceDensitySystem::new(seed),
            caves: CaveField::new(seed),
        }
    }

    /// Compute the region for one chunk PLUS the feature margin in a single pass.
    /// Shared by terrain fill and feature placement, so terrain height and biomes
    /// are generated exactly once.
    pub fn region(&self, cx: i32, cz: i32) -> RegionCells {
        let (x0, z0, w, h) = super::feature::feature_region_bounds(cx * 16, cz * 16);
        self.surface_density.region(x0, z0, w, h)
    }

    pub fn biome_at(&self, wx: i32, wz: i32) -> crate::biome::Biome {
        self.surface_density.biome_at(wx, wz)
    }

    /// Run terrain generation (everything except features) for one chunk, reading
    /// the precomputed region. Kept for staged tooling and diagnostics.
    pub fn generate(&self, region: &RegionCells, cx: i32, cz: i32) -> Chunk {
        let mut proto = ProtoChunk::new(cx, cz);
        self.surface_density.fill_chunk(&mut proto, region);
        proto.into_chunk()
    }

    /// Run hot-path terrain generation for one chunk without materializing a
    /// padded feature region.
    pub fn generate_surface(&self, cx: i32, cz: i32) -> Chunk {
        let mut proto = ProtoChunk::new(cx, cz);
        self.surface_density.fill_chunk_direct(&mut proto);
        proto.into_chunk()
    }

    /// Cave carving stage: removes solid cells from the surface-filled chunk using
    /// the same original density surfaces the cubic section path receives through
    /// [`ColumnGen`].
    pub fn carve_caves(&self, chunk: &mut Chunk) {
        let (ox, oz) = chunk.chunk_origin_world();
        let surf = self
            .surface_density
            .surface_heights(ox, oz, CHUNK_SX, CHUNK_SZ);
        self.caves.carve_chunk(chunk, &surf);
    }

    /// Underground scatter stage: ore veins + stone / dirt / gravel blobs that
    /// overwrite Stone below the surface. Runs before features (vegetation) and is
    /// a pure function of `(seed, cx, cz)`.
    pub fn place_underground(&self, chunk: &mut Chunk) {
        super::feature::scatter::place_underground(chunk, self.seed);
    }

    /// Ground-vegetation stage: single-block plants (grass, flowers, ferns,
    /// mushrooms, dead bushes) keyed to biome + surface material. Runs after the
    /// underground pass and BEFORE trees so it reads bare ground.
    pub fn place_vegetation(&self, chunk: &mut Chunk) {
        super::feature::vegetation::place_vegetation(chunk, self.seed);
    }

    /// Feature placement stage. Reads biome + biome-driven surface from the shared
    /// region (incl. the cross-chunk margin) so trees land in the right biome at the
    /// right height. Kept for staged tooling and diagnostics.
    pub fn place_features(&self, chunk: &mut Chunk, region: &RegionCells) {
        super::feature::place_features(chunk, region, self.seed);
    }

    /// Hot-path feature placement. Builds only the feature candidate/support
    /// windows needed by tree placement instead of a full surf+biome audit region.
    pub fn place_features_runtime(&self, chunk: &mut Chunk) {
        let (ox, oz) = chunk.chunk_origin_world();
        let field = RuntimeFeatureField::new(&self.surface_density, ox, oz);
        let mut field = CaveAdjustedFeatureField {
            inner: field,
            caves: &self.caves,
        };
        super::feature::place_features_with_field(chunk, &mut field, self.seed);
    }

    // --- Cubic per-section generation -------------------------------------------

    /// Compute the shared per-column data for `(cx,cz)`: biome + density surface, the
    /// feature candidate region, and the redwood-support surfaces. This is the heavy,
    /// inherently-2D part of worldgen; it runs once and is shared by all of the
    /// column's [`generate_section`](Self::generate_section) jobs.
    pub fn generate_column_gen(&self, cx: i32, cz: i32) -> ColumnGen {
        let (ox, oz) = (cx * CHUNK_SX as i32, cz * CHUNK_SZ as i32);

        // Candidate window (chunk + spacing margin): full biome + surface. The chunk's
        // own 16×16 biome/surface is the centre of this region, so no separate query.
        let (cx0, cz0, cw, ch) = feature_candidate_bounds(ox, oz);
        let mut candidates = self.surface_density.region(cx0, cz0, cw, ch);

        let mut biome = vec![0u8; SECTION_SIZE * SECTION_SIZE].into_boxed_slice();
        let mut surf = vec![0i32; SECTION_SIZE * SECTION_SIZE].into_boxed_slice();
        let mut top_surf = vec![0i32; SECTION_SIZE * SECTION_SIZE].into_boxed_slice();
        let (mut surf_min, mut surf_max) = (i32::MAX, i32::MIN);
        for z in 0..SECTION_SIZE {
            for x in 0..SECTION_SIZE {
                let (s, b) = candidates.at(ox + x as i32, oz + z as i32);
                let i = z * SECTION_SIZE + x;
                biome[i] = b.id();
                surf[i] = s;
                surf_min = surf_min.min(s);
                surf_max = surf_max.max(s);
            }
        }

        let mut needs_support = false;
        for (i, s) in candidates.surf.iter_mut().enumerate() {
            let wx = candidates.x0 + (i % candidates.w) as i32;
            let wz = candidates.z0 + (i / candidates.w) as i32;
            *s = self.caves.feature_surface_after_caves(wx, wz, *s);
            if matches!(
                super::biome::spec(candidates.biomes[i]).trees.support,
                super::biome::TreeSupport::RedwoodBase
            ) {
                needs_support = true;
            }
        }

        // Candidate-window surface range + whether any cell wants a redwood support
        // check. Feature surfaces are cave-aware, so tree gating does not root on
        // cave-mouth columns.
        let (mut cand_surf_min, mut cand_surf_max) = (i32::MAX, i32::MIN);
        for &s in &candidates.surf {
            cand_surf_min = cand_surf_min.min(s);
            cand_surf_max = cand_surf_max.max(s);
        }

        for z in 0..SECTION_SIZE {
            for x in 0..SECTION_SIZE {
                let i = z * SECTION_SIZE + x;
                let wx = ox + x as i32;
                let wz = oz + z as i32;
                top_surf[i] = self.caves.surface_after_caves(wx, wz, surf[i]);
            }
        }

        // Support window (the larger redwood-support halo): surfaces only, and ONLY when
        // a redwood-supporting biome is actually in range — `redwood_trunk_is_supported`
        // is its sole reader, so the common (redwood-free) column skips this big window.
        let support = needs_support.then(|| {
            let (sx0, sz0, sw, sh) = feature_region_bounds(ox, oz);
            debug_assert_eq!(sw, sh);
            let mut support_surf = self.surface_density.surface_heights(sx0, sz0, sw, sh);
            for (i, s) in support_surf.iter_mut().enumerate() {
                let wx = sx0 + (i % sw) as i32;
                let wz = sz0 + (i / sw) as i32;
                *s = self.caves.feature_surface_after_caves(wx, wz, *s);
            }
            SurfaceHeights::new(sx0, sz0, sw, support_surf)
        });

        ColumnGen {
            cx,
            cz,
            biome,
            surf,
            top_surf,
            surf_min,
            surf_max,
            cand_surf_min,
            cand_surf_max,
            content_top: cand_surf_max + MAX_TREE_REACH_ABOVE,
            candidates,
            support,
        }
    }

    /// Generate one 16³ [`Section`] from its column's shared [`ColumnGen`]. Runs the
    /// fixed stage order — terrain → underground scatter → vegetation → trees — but
    /// each stage clips to this section, and the deep/high stages are skipped when the
    /// section provably cannot hold their output. Byte-identical, above ground, to the
    /// same slab of [`generate_chunk_with`]; works for any `cy` (incl. below y=0).
    pub fn generate_section(&self, sp: SectionPos, col: &ColumnGen) -> Section {
        debug_assert_eq!((sp.cx, sp.cz), (col.cx, col.cz));
        let mut section = Section::new(sp.cx, sp.cy, sp.cz);
        let (_ox, oy, _oz) = sp.origin_world();
        let sec_lo = oy;
        let sec_hi = oy + SECTION_SIZE as i32 - 1;

        // 1. Terrain fill (always). It writes the block buffer in bulk (bypassing the
        //    setter bookkeeping), so recount the random-tick gate NOW — before the stages
        //    below go through `set_block_raw`, whose incremental adjust would otherwise
        //    underflow when a feature overwrites a random-tickable skin block (e.g. a tree
        //    trunk replacing surface grass) while the count still read zero.
        self.surface_density
            .fill_section(&mut section, &col.biome, &col.surf);
        self.caves.carve_section(&mut section, &col.surf);
        section.recompute_random_tick_count();
        section.recompute_opaque_count();

        // 2. Underground scatter: needs stone in the section AND overlap with the ore band.
        let has_stone = sec_lo <= col.surf_max;
        if has_stone && ranges_overlap(sec_lo, sec_hi, SCATTER_MIN_Y, SCATTER_MAX_Y) {
            scatter::place_underground_section(&mut section, self.seed);
        }

        // 3. Ground vegetation: the bare-ground plant cell (anchor+1) can fall here only
        //    if some land column's surface (≥ sea level) sits within reach of the section.
        if col.surf_max >= SEA_LEVEL
            && ranges_overlap(sec_lo, sec_hi, SEA_LEVEL + 1, col.surf_max + 1)
        {
            vegetation::place_vegetation_section(
                &mut section,
                &col.biome,
                &col.surf,
                &col.top_surf,
                self.seed,
            );
        }

        // 4. Trees: a tree roots only where the surface is in (sea level, treeline] and
        //    reaches up to MAX_TREE_REACH_ABOVE. Anchors can sit at margin origins / in
        //    neighbours, so gate on the candidate-window surface range. Skip the section
        //    when no anchor can reach it.
        let anchor_lo = col.cand_surf_min.max(SEA_LEVEL + 1);
        let anchor_hi = col.cand_surf_max.min(TREELINE);
        if anchor_lo <= anchor_hi
            && ranges_overlap(sec_lo, sec_hi, anchor_lo, anchor_hi + MAX_TREE_REACH_ABOVE)
        {
            let mut field = ColumnFeatureField::new(&col.candidates, col.support.as_ref());
            place_features_section(&mut section, &mut field, self.seed);
        }

        section.dirty = true;
        section
    }
}

struct CaveAdjustedFeatureField<'a> {
    inner: RuntimeFeatureField<'a>,
    caves: &'a CaveField,
}

impl FeatureField for CaveAdjustedFeatureField<'_> {
    fn column_at(&mut self, wx: i32, wz: i32) -> (i32, crate::biome::Biome) {
        let (surf, biome) = self.inner.column_at(wx, wz);
        (self.caves.feature_surface_after_caves(wx, wz, surf), biome)
    }

    fn surf_at(&mut self, wx: i32, wz: i32) -> i32 {
        let surf = self.inner.surf_at(wx, wz);
        self.caves.feature_surface_after_caves(wx, wz, surf)
    }
}
