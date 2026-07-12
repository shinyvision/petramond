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

use std::sync::Arc;

use mod_api::WorldgenStage;

use crate::chunk::{idx, Chunk, SectionPos, CHUNK_SX, CHUNK_SY, CHUNK_SZ, SEA_LEVEL, SECTION_SIZE};
use crate::modding::gen::{GenHooks, GenInputs};
use crate::section::{Section, SectionSummary};

use super::density::surface::SurfaceDensitySystem;
use super::feature::{
    apply_gen_writes, feature_candidate_bounds, feature_region_bounds, place_features_section,
    scatter::{self, SCATTER_MAX_Y, SCATTER_MIN_Y},
    cached_feature_region, vegetation, ColumnFeatureField, RuntimeFeatureField,
    SurfaceHeights, MAX_TREE_REACH_ABOVE,
    TREELINE,
};
use super::noise::height::CaveField;
use super::proto::ProtoChunk;
use super::region::RegionCells;

pub struct ChunkGenerator {
    seed: u32,
    surface_density: SurfaceDensitySystem,
    caves: CaveField,
    /// The session's mod worldgen hooks, captured at construction (see
    /// `modding::gen`). `None` — the common case — costs one branch per stage
    /// per section and nothing else; the engine pipeline is untouched.
    hooks: Option<Arc<GenHooks>>,
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
    /// The 20x20 tint halo for this column (two cells beyond each X/Z edge).
    /// Captured from the column-generation region so mesh submission never runs
    /// analytical biome generation on the owning thread.
    mesh_biome: Arc<[u8]>,
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
    /// Tree-placement windows (candidate region + redwood-support halo), consumed
    /// ONLY by tree-band section jobs. `None` once the streamer swaps in a
    /// [`slimmed`](Self::slimmed) clone after the column's gen burst — they are
    /// ~15 KB per column, dead weight while resident. A rare late tree-band job
    /// (vertical window re-entering the surface band) rebuilds them locally via
    /// [`ChunkGenerator::build_feature_windows`].
    feature_windows: Option<FeatureWindows>,
}

/// The tree stage's lattice windows: the feature candidate region (chunk +
/// spacing margin, cave-adjusted) and the redwood-support surface halo. A pure
/// function of `(seed, cx, cz)` — droppable and rebuildable at will.
pub struct FeatureWindows {
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
    #[inline]
    pub fn mesh_biome(&self) -> Arc<[u8]> {
        self.mesh_biome.clone()
    }
    #[inline]
    pub(crate) fn mesh_biome_slice(&self) -> &[u8] {
        &self.mesh_biome
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

    /// Whether the tree-placement windows are still resident (see `feature_windows`).
    #[inline]
    pub fn has_feature_windows(&self) -> bool {
        self.feature_windows.is_some()
    }

    /// A copy of this column's resident data WITHOUT the tree-placement windows —
    /// what the world retains once the column's gen burst is done. ~2 KB of 16×16
    /// arrays instead of ~17 KB.
    pub fn slimmed(&self) -> ColumnGen {
        ColumnGen {
            cx: self.cx,
            cz: self.cz,
            biome: self.biome.clone(),
            mesh_biome: self.mesh_biome.clone(),
            surf: self.surf.clone(),
            top_surf: self.top_surf.clone(),
            surf_min: self.surf_min,
            surf_max: self.surf_max,
            cand_surf_min: self.cand_surf_min,
            cand_surf_max: self.cand_surf_max,
            content_top: self.content_top,
            feature_windows: None,
        }
    }

    /// This column's resident data as a column-gen cache record ("Optimize
    /// explored terrain"). The record IS the slimmed column: a load through
    /// [`from_cache_record`](Self::from_cache_record) reproduces exactly what
    /// [`slimmed`](Self::slimmed) retains.
    pub fn cache_record(&self, seed: u32) -> crate::save::colgen::ColumnGenRecord {
        crate::save::colgen::ColumnGenRecord {
            pos: crate::chunk::ChunkPos::new(self.cx, self.cz),
            seed,
            biome: self.biome.clone(),
            mesh_biome: self.mesh_biome.clone(),
            surf: self.surf.clone(),
            top_surf: self.top_surf.clone(),
            surf_min: self.surf_min,
            surf_max: self.surf_max,
            cand_surf_min: self.cand_surf_min,
            cand_surf_max: self.cand_surf_max,
            content_top: self.content_top,
        }
    }

    /// Rebuild a (slim) column from its cache record — no feature windows; a
    /// rare late tree-band section job rebuilds them locally, exactly like a
    /// column slimmed after its gen burst.
    pub fn from_cache_record(rec: crate::save::colgen::ColumnGenRecord) -> ColumnGen {
        ColumnGen {
            cx: rec.pos.cx,
            cz: rec.pos.cz,
            biome: rec.biome,
            mesh_biome: rec.mesh_biome,
            surf: rec.surf,
            top_surf: rec.top_surf,
            surf_min: rec.surf_min,
            surf_max: rec.surf_max,
            cand_surf_min: rec.cand_surf_min,
            cand_surf_max: rec.cand_surf_max,
            content_top: rec.content_top,
            feature_windows: None,
        }
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
        Self::with_hooks(seed, crate::modding::gen::active())
    }

    /// [`new`](Self::new) with an explicit hook config instead of the
    /// process-installed one — how tests inject hooks without global state,
    /// and how `None` pins the pure engine pipeline.
    pub(crate) fn with_hooks(seed: u32, hooks: Option<Arc<GenHooks>>) -> Self {
        Self {
            seed,
            surface_density: SurfaceDensitySystem::new(seed),
            caves: CaveField::new(seed),
            hooks,
        }
    }

    /// Whether any mod worldgen hooks are active on this generator.
    pub(crate) fn has_gen_hooks(&self) -> bool {
        self.hooks.is_some()
    }

    /// Compute the region for one chunk PLUS the feature margin in a single pass.
    /// Shared by terrain fill and feature placement, so terrain height and biomes
    /// are generated exactly once.
    pub(crate) fn region(&self, cx: i32, cz: i32) -> RegionCells {
        let (x0, z0, w, h) = super::feature::feature_region_bounds(cx * 16, cz * 16);
        self.surface_density.region(x0, z0, w, h)
    }

    pub fn biome_at(&self, wx: i32, wz: i32) -> crate::biome::Biome {
        self.surface_density.biome_at(wx, wz)
    }

    /// Run hot-path terrain generation for one chunk without materializing a
    /// padded feature region.
    pub fn generate_surface(&self, cx: i32, cz: i32) -> Chunk {
        let mut proto = ProtoChunk::new(cx, cz);
        // Per-column biome + surface from the shared window tile memo — the
        // same tile the carve and feature stages read — so terrain fill no
        // longer builds its own lattice or re-classifies climate.
        let (ox, oz) = (cx * CHUNK_SX as i32, cz * CHUNK_SZ as i32);
        let (region, raw) = cached_feature_region(
            &self.surface_density,
            &self.caves,
            self.seed,
            ox,
            oz,
            CHUNK_SX,
            CHUNK_SZ,
        );
        self.surface_density
            .fill_chunk_from(&mut proto, &region.biomes, &raw);
        proto.into_chunk()
    }

    /// Cave carving stage: removes solid cells from the surface-filled chunk using
    /// the same original density surfaces the cubic section path receives through
    /// [`ColumnGen`].
    pub fn carve_caves(&self, chunk: &mut Chunk) {
        let (ox, oz) = chunk.chunk_origin_world();
        // Raw surfaces via the shared window tile memo — the feature stage
        // needs this chunk's tile anyway, so the carve query is free on hit
        // and pre-warms it on miss.
        let (_region, surf) = cached_feature_region(
            &self.surface_density,
            &self.caves,
            self.seed,
            ox,
            oz,
            CHUNK_SX,
            CHUNK_SZ,
        );
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

    /// Hot-path feature placement. Builds only the feature candidate/support
    /// windows needed by tree placement instead of a full surf+biome audit
    /// region; the windows come out cave-adjusted (mouths are not tree roots).
    pub fn place_features_runtime(&self, chunk: &mut Chunk) {
        let (ox, oz) = chunk.chunk_origin_world();
        let mut field =
            RuntimeFeatureField::new(&self.surface_density, &self.caves, self.seed, ox, oz);
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
        // Served by the per-thread window memo; `raw_surf` carries the
        // pre-cave-adjustment surfaces the column core stores.
        let (cx0, cz0, cw, ch) = feature_candidate_bounds(ox, oz);
        let (candidates, raw_surf) = cached_feature_region(
            &self.surface_density,
            &self.caves,
            self.seed,
            cx0,
            cz0,
            cw,
            ch,
        );

        const MESH_BIOME_RADIUS: i32 = 2;
        const MESH_BIOME_SIDE: usize = SECTION_SIZE + MESH_BIOME_RADIUS as usize * 2;
        let mut biome = vec![0u8; SECTION_SIZE * SECTION_SIZE].into_boxed_slice();
        let mut mesh_biome = vec![0u8; MESH_BIOME_SIDE * MESH_BIOME_SIDE].into_boxed_slice();
        let mut surf = vec![0i32; SECTION_SIZE * SECTION_SIZE].into_boxed_slice();
        let mut top_surf = vec![0i32; SECTION_SIZE * SECTION_SIZE].into_boxed_slice();
        let (mut surf_min, mut surf_max) = (i32::MAX, i32::MIN);
        for z in 0..SECTION_SIZE {
            for x in 0..SECTION_SIZE {
                let wx = ox + x as i32;
                let wz = oz + z as i32;
                let (_, b) = candidates.at(wx, wz);
                let s = raw_surf[(wz - cz0) as usize * cw + (wx - cx0) as usize];
                let i = z * SECTION_SIZE + x;
                biome[i] = b.id();
                surf[i] = s;
                surf_min = surf_min.min(s);
                surf_max = surf_max.max(s);
            }
        }
        for z in 0..MESH_BIOME_SIDE {
            for x in 0..MESH_BIOME_SIDE {
                let (_, b) = candidates.at(
                    ox - MESH_BIOME_RADIUS + x as i32,
                    oz - MESH_BIOME_RADIUS + z as i32,
                );
                mesh_biome[z * MESH_BIOME_SIDE + x] = b.id();
            }
        }

        let windows = self.finish_feature_windows(ox, oz, candidates);

        // Candidate-window surface range. Feature surfaces are cave-aware, so tree
        // gating does not root on cave-mouth columns.
        let (mut cand_surf_min, mut cand_surf_max) = (i32::MAX, i32::MIN);
        for &s in &windows.candidates.surf {
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

        // Mod climate replacement: substitute the column's OWN biome map (what the
        // terrain skin, vegetation, and every later hook read). The candidate-window
        // biomes stay the engine's — the tree stage keeps engine climate unless it is
        // itself replaced (documented, Phase 4 notes).
        if let Some(hooks) = &self.hooks {
            if hooks.replaces(WorldgenStage::Climate) {
                let inputs = GenInputs {
                    seed: self.seed,
                    section_pos: [cx, 0, cz],
                    blocks: &[],
                    surface_heights: &top_surf,
                    biomes: &biome,
                };
                if let Some(map) = hooks.replace_climate(&inputs) {
                    biome.copy_from_slice(&map);
                    for z in 0..SECTION_SIZE {
                        let dst = (z + MESH_BIOME_RADIUS as usize) * MESH_BIOME_SIDE
                            + MESH_BIOME_RADIUS as usize;
                        let src = z * SECTION_SIZE;
                        mesh_biome[dst..dst + SECTION_SIZE]
                            .copy_from_slice(&biome[src..src + SECTION_SIZE]);
                    }
                }
            }
        }

        ColumnGen {
            cx,
            cz,
            biome,
            mesh_biome: Arc::from(mesh_biome),
            surf,
            top_surf,
            surf_min,
            surf_max,
            cand_surf_min,
            cand_surf_max,
            content_top: cand_surf_max + MAX_TREE_REACH_ABOVE,
            feature_windows: Some(windows),
        }
    }

    /// Rebuild a column's tree-placement windows from scratch — for a section job
    /// that received a [`ColumnGen::slimmed`] column. Byte-identical to the windows
    /// the original column build produced (pure function of `(seed, cx, cz)`).
    fn build_feature_windows(&self, cx: i32, cz: i32) -> FeatureWindows {
        let (ox, oz) = (cx * CHUNK_SX as i32, cz * CHUNK_SZ as i32);
        let (cx0, cz0, cw, ch) = feature_candidate_bounds(ox, oz);
        let (candidates, _raw) = cached_feature_region(
            &self.surface_density,
            &self.caves,
            self.seed,
            cx0,
            cz0,
            cw,
            ch,
        );
        self.finish_feature_windows(ox, oz, candidates)
    }

    /// Build the redwood-support halo over the (already cave-adjusted)
    /// candidate region: the shared tail of [`generate_column_gen`] and
    /// [`build_feature_windows`]. `candidates` must come from
    /// `cached_feature_region`.
    fn finish_feature_windows(
        &self,
        ox: i32,
        oz: i32,
        candidates: RegionCells,
    ) -> FeatureWindows {
        let needs_support = candidates.biomes.iter().any(|b| {
            matches!(
                super::biome::spec(*b).trees.support,
                super::biome::TreeSupport::RedwoodBase
            )
        });

        // Support window (the larger redwood-support halo): surfaces only, and ONLY when
        // a redwood-supporting biome is actually in range — `redwood_trunk_is_supported`
        // is its sole reader, so the common (redwood-free) column skips this big window.
        let support = needs_support.then(|| {
            let (sx0, sz0, sw, sh) = feature_region_bounds(ox, oz);
            debug_assert_eq!(sw, sh);
            let (region, _raw) = cached_feature_region(
                &self.surface_density,
                &self.caves,
                self.seed,
                sx0,
                sz0,
                sw,
                sh,
            );
            SurfaceHeights::new(sx0, sz0, sw, region.surf)
        });

        FeatureWindows {
            candidates,
            support,
        }
    }

    /// Generate one 16³ [`Section`] from its column's shared [`ColumnGen`]. Runs the
    /// fixed stage order — terrain → underground scatter → vegetation → trees — but
    /// each stage clips to this section, and the deep/high stages are skipped when the
    /// section provably cannot hold their output. Byte-identical, above ground, to the
    /// same slab of [`generate_chunk_with`]; works for any `cy` (incl. below y=0).
    ///
    /// Mod worldgen hooks attach here (and ONLY here — the whole-chunk path routes
    /// through this function when hooks are active, so both paths dispatch every hook
    /// with identical inputs by construction): a registered stage REPLACEMENT runs
    /// instead of the engine stage (falling back to the engine stage if it fails),
    /// and registered FEATURES run after their stage, unconditionally — mod content
    /// is not bounded by the engine stages' reach gates.
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
        match self.replaced_terrain_fill(sp, col) {
            Some(fill) => section.blocks_slice_mut().copy_from_slice(&fill),
            None => {
                self.surface_density
                    .fill_section(&mut section, &col.biome, &col.surf);
                self.caves.carve_section(&mut section, &col.surf);
            }
        }
        section.recompute_opaque_count();
        self.run_gen_features(WorldgenStage::Terrain, sp, &mut section, col);

        // 2. Underground scatter: needs stone in the section AND overlap with the ore band.
        if !self.run_stage_replacement(WorldgenStage::Underground, sp, &mut section, col) {
            let has_stone = sec_lo <= col.surf_max;
            if has_stone && ranges_overlap(sec_lo, sec_hi, SCATTER_MIN_Y, SCATTER_MAX_Y) {
                scatter::place_underground_section(&mut section, self.seed);
            }
        }
        self.run_gen_features(WorldgenStage::Underground, sp, &mut section, col);

        // 3. Ground vegetation: the bare-ground plant cell (anchor+1) can fall here only
        //    if some land column's surface (≥ sea level) sits within reach of the section.
        if !self.run_stage_replacement(WorldgenStage::Vegetation, sp, &mut section, col)
            && col.surf_max >= SEA_LEVEL
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
        self.run_gen_features(WorldgenStage::Vegetation, sp, &mut section, col);

        // 4. Trees: a tree roots only where the surface is in (sea level, treeline] and
        //    reaches up to MAX_TREE_REACH_ABOVE. Anchors can sit at margin origins / in
        //    neighbours, so gate on the candidate-window surface range. Skip the section
        //    when no anchor can reach it.
        if !self.run_stage_replacement(WorldgenStage::Trees, sp, &mut section, col) {
            let anchor_lo = col.cand_surf_min.max(SEA_LEVEL + 1);
            let anchor_hi = col.cand_surf_max.min(TREELINE);
            if anchor_lo <= anchor_hi
                && ranges_overlap(sec_lo, sec_hi, anchor_lo, anchor_hi + MAX_TREE_REACH_ABOVE)
            {
                // A slimmed column (gen burst long done) rebuilds its windows
                // locally — rare, and byte-identical by construction.
                let rebuilt;
                let windows = match &col.feature_windows {
                    Some(w) => w,
                    None => {
                        rebuilt = self.build_feature_windows(col.cx, col.cz);
                        &rebuilt
                    }
                };
                let mut field =
                    ColumnFeatureField::new(&windows.candidates, windows.support.as_ref());
                place_features_section(&mut section, &mut field, self.seed);
            }
        }
        self.run_gen_features(WorldgenStage::Trees, sp, &mut section, col);

        section.dirty = true;
        section
    }

    /// The mod terrain replacement's 4096-block fill, or `None` when no
    /// replacement is registered / it failed (the engine fill+carve runs).
    fn replaced_terrain_fill(&self, sp: SectionPos, col: &ColumnGen) -> Option<Vec<u8>> {
        let hooks = self.hooks.as_ref()?;
        if !hooks.replaces(WorldgenStage::Terrain) {
            return None;
        }
        hooks.replace_terrain(&GenInputs {
            seed: self.seed,
            section_pos: [sp.cx, sp.cy, sp.cz],
            blocks: &[],
            surface_heights: &col.top_surf,
            biomes: &col.biome,
        })
    }

    /// Run `stage`'s registered replacement into `section`. `false` = no
    /// replacement / it failed — the caller runs the engine stage.
    fn run_stage_replacement(
        &self,
        stage: WorldgenStage,
        sp: SectionPos,
        section: &mut Section,
        col: &ColumnGen,
    ) -> bool {
        let Some(hooks) = &self.hooks else {
            return false;
        };
        if !hooks.replaces(stage) {
            return false;
        }
        let writes = hooks.replace_stage(
            stage,
            &GenInputs {
                seed: self.seed,
                section_pos: [sp.cx, sp.cy, sp.cz],
                blocks: section.blocks_slice(),
                surface_heights: &col.top_surf,
                biomes: &col.biome,
            },
        );
        match writes {
            Some(writes) => {
                apply_gen_writes(section, &writes);
                true
            }
            None => false,
        }
    }

    /// Dispatch every feature attached after `stage`, in registration order,
    /// each seeing the section as of the previous one's writes.
    fn run_gen_features(
        &self,
        stage: WorldgenStage,
        sp: SectionPos,
        section: &mut Section,
        col: &ColumnGen,
    ) {
        let Some(hooks) = &self.hooks else {
            return;
        };
        if !hooks.any_features_after(stage) {
            return;
        }
        for idx in hooks.features_after(stage) {
            let writes = hooks.dispatch_feature(
                idx,
                &GenInputs {
                    seed: self.seed,
                    section_pos: [sp.cx, sp.cy, sp.cz],
                    blocks: section.blocks_slice(),
                    surface_heights: &col.top_surf,
                    biomes: &col.biome,
                },
            );
            if let Some(writes) = writes {
                apply_gen_writes(section, &writes);
            }
        }
    }

    /// Whole-chunk generation ASSEMBLED from the cubic per-section path — the
    /// hook-active variant of `generate_chunk_with`: because every stage and
    /// hook runs through [`generate_section`](Self::generate_section), the two
    /// paths dispatch identical calls per `(seed, section)` by construction
    /// (parity is structural, not mirrored). Covers the chunk's own vertical
    /// range (cy 0..16); mod writes below y=0 exist only in the cubic world.
    pub(crate) fn generate_chunk_via_sections(&self, cx: i32, cz: i32) -> Chunk {
        let col = self.generate_column_gen(cx, cz);
        let mut chunk = Chunk::new(cx, cz);
        for z in 0..CHUNK_SZ {
            for x in 0..CHUNK_SX {
                chunk.set_biome(x, z, col.biome_at(x, z));
            }
        }
        for cy in 0..(CHUNK_SY / SECTION_SIZE) as i32 {
            let section = self.generate_section(SectionPos::new(cx, cy, cz), &col);
            let blocks = chunk.blocks_slice_mut();
            for ly in 0..SECTION_SIZE {
                let wy = cy as usize * SECTION_SIZE + ly;
                for z in 0..CHUNK_SZ {
                    for x in 0..CHUNK_SX {
                        blocks[idx(x, wy, z)] = section.block_raw(x, ly, z);
                    }
                }
            }
        }
        chunk.recompute_heightmap();
        chunk.recompute_random_tick_count();
        chunk
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A slimmed column must regenerate any section byte-identically: the tree
    /// windows it drops are a pure function of `(seed, cx, cz)`, and the rebuild
    /// path in `generate_section` must reproduce them exactly.
    #[test]
    fn slimmed_column_regenerates_sections_byte_identically() {
        let generator = ChunkGenerator::new(0xDEAD_BEEF);
        for (cx, cz) in [(0, 0), (3, -2)] {
            let full = generator.generate_column_gen(cx, cz);
            let slim = full.slimmed();
            assert!(full.has_feature_windows() && !slim.has_feature_windows());
            // Cover the surface/tree band and a deep section.
            let (lo, hi) = full.surf_range();
            for cy in [
                lo.div_euclid(16) - 1,
                hi.div_euclid(16),
                hi.div_euclid(16) + 1,
            ] {
                let sp = SectionPos::new(cx, cy, cz);
                let a = generator.generate_section(sp, &full);
                let b = generator.generate_section(sp, &slim);
                assert_eq!(
                    a.blocks_slice(),
                    b.blocks_slice(),
                    "slimmed rebuild diverged at cy {cy} of column ({cx},{cz})"
                );
            }
        }
    }

    /// A column restored from its encoded cache record ("Optimize explored
    /// terrain") must be indistinguishable from a slimmed live column: same
    /// resident data, byte-identical section regeneration.
    #[test]
    fn cache_record_roundtrip_matches_the_live_column() {
        let seed = 0xDEAD_BEEF;
        let generator = ChunkGenerator::new(seed);
        let full = generator.generate_column_gen(2, -5);
        let blob = crate::save::colgen::encode_record(&full.cache_record(seed));
        let rec =
            crate::save::colgen::decode_record(crate::chunk::ChunkPos::new(2, -5), seed, &blob)
                .expect("cache record decodes");
        let cached = ColumnGen::from_cache_record(rec);

        for x in 0..SECTION_SIZE {
            for z in 0..SECTION_SIZE {
                assert_eq!(cached.biome_at(x, z), full.biome_at(x, z));
                assert_eq!(cached.surface_y(x, z), full.surface_y(x, z));
                assert_eq!(
                    cached.heightmap_surface_y(x, z),
                    full.heightmap_surface_y(x, z)
                );
            }
        }
        assert_eq!(cached.surf_range(), full.surf_range());
        assert_eq!(cached.content_top(), full.content_top());
        let (lo, hi) = full.surf_range();
        for cy in [lo.div_euclid(16), hi.div_euclid(16) + 1] {
            let sp = SectionPos::new(2, cy, -5);
            assert_eq!(
                generator.generate_section(sp, &full).blocks_slice(),
                generator.generate_section(sp, &cached).blocks_slice(),
                "cached column diverged at cy {cy}"
            );
        }
    }
}
