//! `ChunkGenerator` — owns the worldgen subsystems and runs the ONE generation
//! pipeline: a column's shared 2D data ([`ColumnGen`]: climate, surfaces and
//! the tree windows), then each 16³ section's fixed stage order — terrain fill
//! and carve → underground scatter → vegetation → trees, with mod hooks after
//! each stage. A whole chunk is those sections assembled
//! ([`ChunkGenerator::generate_chunk`]), never a second implementation.
//!
//! The generator holds only immutable wiring built from `seed` (no interior
//! mutability). Output is therefore a pure function of `(seed, section)`,
//! independent of thread or call order.

use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use mod_api::WorldgenStage;

use crate::hooks::{FeatureOutcome, GenHookDispatch, GenInputs};
use petramond_world::chunk::{
    idx, Chunk, SectionPos, CHUNK_SX, CHUNK_SY, CHUNK_SZ, SEA_LEVEL, SECTION_SIZE,
};
use petramond_world::section::{Section, SectionSummary};

use super::density::surface::SurfaceDensitySystem;
use super::feature::{
    apply_gen_plan, cached_feature_region, feature_candidate_bounds, feature_region_bounds,
    scatter::{self, SCATTER_MAX_Y, SCATTER_MIN_Y},
    vegetation, ColumnFeatureField, FeaturePlan, SurfaceHeights, MAX_TREE_REACH_ABOVE, TREELINE,
};
use super::noise::cave_field::CaveField;
use super::region::RegionCells;

pub struct ChunkGenerator {
    seed: u32,
    surface_density: SurfaceDensitySystem,
    caves: CaveField,
    /// The session's mod worldgen hooks, captured at construction (see
    /// `modding::gen`). `None` — the common case — costs one branch per stage
    /// per section and nothing else; the engine pipeline is untouched.
    hooks: Option<Arc<dyn GenHookDispatch>>,
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
    plan: OnceLock<FeaturePlan>,
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
    /// Resident heap bytes of this column's gen data, for the memory census.
    pub fn memory_bytes(&self) -> u64 {
        let base = std::mem::size_of::<Self>()
            + self.biome.len()
            + self.mesh_biome.len()
            + self.surf.len() * 4
            + self.top_surf.len() * 4;
        let windows = self.feature_windows.as_ref().map_or(0, |w| {
            std::mem::size_of::<FeatureWindows>()
                + w.plan.get().map_or(0, FeaturePlan::memory_bytes)
                + w.candidates.surf.capacity() * 4
                + w.candidates.biomes.capacity()
                    * std::mem::size_of::<petramond_world::biome::Biome>()
                + w.support.as_ref().map_or(0, |s| s.capacity_bytes())
        });
        (base + windows) as u64
    }

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
    pub fn mesh_biome_slice(&self) -> &[u8] {
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
    pub fn cache_record(&self, seed: u32) -> crate::colgen::ColumnGenRecord {
        crate::colgen::ColumnGenRecord {
            pos: petramond_world::chunk::ChunkPos::new(self.cx, self.cz),
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
    pub fn from_cache_record(rec: crate::colgen::ColumnGenRecord) -> ColumnGen {
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

/// A hook deferred the section (see [`FeatureOutcome::Deferred`]); `feature`
/// is the position, among the stage's attached features, to resume at (`0`
/// for a deferred stage replacement, which reruns the whole stage).
struct Deferred {
    feature: usize,
}

/// The section stages, in pipeline order. Climate runs per column.
const SECTION_STAGES: [WorldgenStage; 4] = [
    WorldgenStage::Terrain,
    WorldgenStage::Underground,
    WorldgenStage::Vegetation,
    WorldgenStage::Trees,
];

/// Where a [`PendingSection`] picks up within its current stage.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Resume {
    /// The stage's own work (replacement or engine stage) has not run.
    Stage,
    /// The stage's own work is done; dispatch its attached features from
    /// this position on.
    Feature(usize),
}

/// A section a mod hook deferred, holding every stage and hook output
/// produced before the deferral so [`ChunkGenerator::resume_section`]
/// continues from the deferring hook instead of regenerating from scratch.
pub struct PendingSection {
    sp: SectionPos,
    section: Section,
    /// Index into [`SECTION_STAGES`] of the stage to continue.
    stage: usize,
    resume: Resume,
}

impl PendingSection {
    /// The section being generated.
    pub fn pos(&self) -> SectionPos {
        self.sp
    }
}

/// One non-blocking step of section generation.
pub enum SectionGen {
    /// The finished section.
    Ready(Section),
    /// A hook deferred the section; resume it once the fact it waits on is
    /// published.
    Deferred(PendingSection),
}

#[inline]
fn ranges_overlap(a_lo: i32, a_hi: i32, b_lo: i32, b_hi: i32) -> bool {
    a_lo <= b_hi && b_lo <= a_hi
}

impl ChunkGenerator {
    pub fn new(seed: u32) -> Self {
        Self::with_hooks(seed, crate::hooks::active())
    }

    /// [`new`](Self::new) with an explicit hook config instead of the
    /// process-installed one — how tests inject hooks without global state,
    /// and how `None` pins the pure engine pipeline.
    pub fn with_hooks(seed: u32, hooks: Option<Arc<dyn GenHookDispatch>>) -> Self {
        Self::with_caches(seed, hooks, crate::cache::installed())
    }

    /// [`with_hooks`](Self::with_hooks) over explicit memos instead of the
    /// installed ones.
    pub fn with_caches(
        seed: u32,
        hooks: Option<Arc<dyn GenHookDispatch>>,
        caches: Arc<crate::cache::GenCaches>,
    ) -> Self {
        Self {
            seed,
            surface_density: SurfaceDensitySystem::new(seed),
            caves: CaveField::new(seed).with_caches(caches),
            hooks,
        }
    }

    /// Identity of what [`new`](Self::new) captures besides the seed — the
    /// installed hook config and memos. A cached generator built under a
    /// different identity belongs to a previous session or world.
    pub fn installed_config() -> (u64, u64) {
        (
            crate::hooks::installed_epoch(),
            crate::cache::installed_epoch(),
        )
    }

    /// The process's shared generator for `seed` under the installed hook
    /// config and memos — built once (the two density graphs and the cave
    /// field are the expensive part) and handed out by `Arc` to every caller
    /// without a generator of its own: the positional host queries,
    /// whole-chunk tooling and on-demand section materialization. A seed or
    /// installed-config change replaces it; concurrent first callers wait for
    /// the one build.
    pub fn shared(seed: u32) -> Arc<ChunkGenerator> {
        type Slot = Option<((u32, (u64, u64)), Arc<ChunkGenerator>)>;
        static SLOT: Mutex<Slot> = Mutex::new(None);
        let key = (seed, Self::installed_config());
        let mut slot = SLOT.lock().unwrap_or_else(PoisonError::into_inner);
        match slot.as_ref() {
            Some((k, generator)) if *k == key => Arc::clone(generator),
            _ => {
                let generator = Arc::new(Self::new(seed));
                *slot = Some((key, Arc::clone(&generator)));
                generator
            }
        }
    }

    /// The pure engine pipeline over an explicit cave field — how a test
    /// generates from synthetic cave rows.
    #[cfg(all(test, feature = "worldgen-tests"))]
    pub(crate) fn with_caves(seed: u32, caves: CaveField) -> Self {
        Self {
            seed,
            surface_density: SurfaceDensitySystem::new(seed),
            caves,
            hooks: None,
        }
    }

    /// The generator's engine sources: the surface density graph and the
    /// cave field.
    pub(crate) fn sources(&self) -> (&SurfaceDensitySystem, &CaveField) {
        (&self.surface_density, &self.caves)
    }

    /// Whether any mod worldgen hooks are active on this generator.
    pub fn has_gen_hooks(&self) -> bool {
        self.hooks.is_some()
    }

    /// Compute the region for one chunk PLUS the feature margin in a single pass.
    /// Shared by terrain fill and feature placement, so terrain height and biomes
    /// are generated exactly once.
    pub fn region(&self, cx: i32, cz: i32) -> RegionCells {
        let (x0, z0, w, h) = super::feature::feature_region_bounds(cx * 16, cz * 16);
        self.surface_density.region(x0, z0, w, h)
    }

    /// Warm one 16×16 world tile of the shared feature-window memo — a pure
    /// cache fill (the memo is keyed by `(context, tile)`, so any thread's
    /// computation serves every later reader). Session bootstrap fans the
    /// spawn area's tiles across the pool with this so the first column jobs
    /// find them hot instead of computing them serially inside one job.
    pub fn warm_surface_tile(&self, tcx: i32, tcz: i32) {
        let _ = cached_feature_region(
            &self.surface_density,
            &self.caves,
            tcx * CHUNK_SX as i32,
            tcz * CHUNK_SZ as i32,
            CHUNK_SX,
            CHUNK_SZ,
        );
    }

    pub fn biome_at(&self, wx: i32, wz: i32) -> petramond_world::biome::Biome {
        self.surface_density.biome_at(wx, wz)
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

        top_surf.copy_from_slice(&self.caves.surfaces_after_caves(ox, oz, &surf));

        // Mod climate replacement: substitute the column's OWN biome map (what the
        // terrain skin, vegetation, and every later hook read). The candidate-window
        // biomes stay the engine's — the tree stage keeps engine climate unless it is
        // itself replaced.
        if let Some(hooks) = &self.hooks {
            if hooks.replaces(WorldgenStage::Climate) {
                let inputs = GenInputs {
                    seed: self.seed,
                    section_pos: [cx, 0, cz],
                    blocks: None,
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
    fn finish_feature_windows(&self, ox: i32, oz: i32, candidates: RegionCells) -> FeatureWindows {
        let needs_support = candidates.biomes.iter().any(|b| {
            super::biome::trees::profile(*b).support
                == super::biome::trees::TreeSupport::RedwoodBase
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
                sx0,
                sz0,
                sw,
                sh,
            );
            SurfaceHeights::new(sx0, sz0, sw, region.surf)
        });

        FeatureWindows {
            plan: OnceLock::new(),
            candidates,
            support,
        }
    }

    /// Generate one 16³ [`Section`] from its column's shared [`ColumnGen`], to
    /// completion. Runs the fixed stage order — terrain → underground scatter →
    /// vegetation → trees — but each stage clips to this section, and the
    /// deep/high stages are skipped when the section provably cannot hold their
    /// output. Works for any `cy` (incl. below y=0).
    ///
    /// For callers that need the finished section NOW — whole-chunk assembly,
    /// tooling, tests. When a mod hook defers (see [`FeatureOutcome::Deferred`])
    /// this waits for the fact it depends on to be published, woken by the
    /// publisher through [`GenHookDispatch::wait_deferred`], and resumes at the
    /// deferred hook. A streaming or simulation caller must never wait: it uses
    /// [`start_section`](Self::start_section) and re-queues the
    /// [`PendingSection`] instead.
    pub fn generate_section(&self, sp: SectionPos, col: &ColumnGen) -> Section {
        let mut attempt = self.start_section(sp, col);
        loop {
            match attempt {
                SectionGen::Ready(section) => return section,
                SectionGen::Deferred(pending) => {
                    if let Some(hooks) = &self.hooks {
                        hooks.wait_deferred();
                    }
                    attempt = self.resume_section(pending, col);
                }
            }
        }
    }

    /// Begin generating section `sp`; never waits. Mod worldgen hooks attach
    /// here (and ONLY here — every whole-chunk consumer assembles sections, so
    /// each hook sees identical inputs per `(seed, section)` on every path): a
    /// registered stage REPLACEMENT runs instead of the engine stage (falling
    /// back to the engine stage if it fails), and registered FEATURES run after
    /// their stage, unconditionally — mod content is not bounded by the engine
    /// stages' reach gates.
    ///
    /// Answers [`SectionGen::Deferred`] when a hook deferred the section: a
    /// positional fact it depends on is being derived by another worker right
    /// now. The caller hands the [`PendingSection`] to
    /// [`resume_section`](Self::resume_section) once that fact is published.
    pub fn start_section(&self, sp: SectionPos, col: &ColumnGen) -> SectionGen {
        debug_assert_eq!((sp.cx, sp.cz), (col.cx, col.cz));
        self.advance(
            PendingSection {
                sp,
                section: Section::new(sp.cx, sp.cy, sp.cz),
                stage: 0,
                resume: Resume::Stage,
            },
            col,
        )
    }

    /// Continue a deferred section from the hook that deferred it. The stages
    /// and hooks already applied are kept: each is a pure function of its
    /// inputs, and the facts a hook reads are positional, so running them
    /// again would reproduce the same writes — the section's content does not
    /// depend on when, or how often, it was deferred.
    pub fn resume_section(&self, pending: PendingSection, col: &ColumnGen) -> SectionGen {
        debug_assert_eq!((pending.sp.cx, pending.sp.cz), (col.cx, col.cz));
        self.advance(pending, col)
    }

    /// Run `pending` forward from its cursor until it finishes or a hook
    /// defers it again.
    fn advance(&self, mut pending: PendingSection, col: &ColumnGen) -> SectionGen {
        while let Some(&stage) = SECTION_STAGES.get(pending.stage) {
            let sp = pending.sp;
            let first_feature = match pending.resume {
                Resume::Stage => {
                    if self.run_stage(stage, sp, &mut pending.section, col).is_err() {
                        return SectionGen::Deferred(pending);
                    }
                    0
                }
                Resume::Feature(i) => i,
            };
            if let Err(Deferred { feature }) =
                self.run_gen_features(stage, sp, &mut pending.section, col, first_feature)
            {
                pending.resume = Resume::Feature(feature);
                return SectionGen::Deferred(pending);
            }
            pending.stage += 1;
            pending.resume = Resume::Stage;
        }
        pending.section.dirty = true;
        SectionGen::Ready(pending.section)
    }

    /// One stage's own work on `section`: the mod replacement when one is
    /// registered and succeeds, else the engine stage behind its reach gate.
    fn run_stage(
        &self,
        stage: WorldgenStage,
        sp: SectionPos,
        section: &mut Section,
        col: &ColumnGen,
    ) -> Result<(), Deferred> {
        let sec_lo = sp.cy * SECTION_SIZE as i32;
        let sec_hi = sec_lo + SECTION_SIZE as i32 - 1;
        match stage {
            WorldgenStage::Terrain => self.fill_terrain(sp, section, col),
            // Underground scatter: needs stone in the section AND overlap with the ore band.
            WorldgenStage::Underground => {
                if !self.run_stage_replacement(stage, sp, section, col)? {
                    let has_stone = sec_lo <= col.surf_max;
                    if has_stone && ranges_overlap(sec_lo, sec_hi, SCATTER_MIN_Y, SCATTER_MAX_Y) {
                        scatter::place_underground_section(section, self.seed);
                    }
                }
            }
            // Ground vegetation: the bare-ground plant cell (anchor+1) can fall here only
            // if some land column's surface (≥ sea level) sits within reach of the section.
            WorldgenStage::Vegetation => {
                if !self.run_stage_replacement(stage, sp, section, col)?
                    && col.surf_max >= SEA_LEVEL
                    && ranges_overlap(sec_lo, sec_hi, SEA_LEVEL + 1, col.surf_max + 1)
                {
                    vegetation::place_vegetation_section(
                        section,
                        &col.biome,
                        &col.surf,
                        &col.top_surf,
                        self.seed,
                    );
                }
            }
            WorldgenStage::Trees => {
                if !self.run_stage_replacement(stage, sp, section, col)? {
                    self.place_trees(sp, section, col);
                }
            }
            // Climate is a column stage (`generate_column_gen`), never a section one.
            WorldgenStage::Climate => {}
        }
        Ok(())
    }

    /// Terrain fill (always). It writes the block buffer in bulk (bypassing the
    /// setter bookkeeping), so the random-tick gate is recounted NOW — before
    /// the stages after it go through `set_block_raw`, whose incremental adjust
    /// would otherwise underflow when a feature overwrites a random-tickable
    /// skin block (e.g. a tree trunk replacing surface grass) while the count
    /// still read zero.
    fn fill_terrain(&self, sp: SectionPos, section: &mut Section, col: &ColumnGen) {
        let engine_terrain = match self.replaced_terrain_fill(sp, col) {
            Some(fill) => {
                *section.blocks_mut() = petramond_world::section::BlockCube::from_ids(&fill);
                false
            }
            // A replaced climate fills with the mod's biome map, which the
            // shared terrain memo (keyed on the engine's) must not carry.
            None if self
                .hooks
                .as_ref()
                .is_some_and(|h| h.replaces(WorldgenStage::Climate)) =>
            {
                self.surface_density
                    .fill_section(section, &col.biome, &col.surf);
                self.caves.carve_section(section, &col.surf);
                true
            }
            None => {
                *section.blocks_mut() =
                    crate::section_memo::terrain_cube(&self.surface_density, &self.caves, sp);
                true
            }
        };
        section.recompute_opaque_count();
        // After the recount, like every stage that writes through a setter. A
        // fall reads the engine's cave, so a replaced terrain carries none.
        if engine_terrain {
            crate::section_memo::stamp_falls(&self.caves, sp, section);
        }
    }

    /// Trees: a tree roots only where the surface is in (sea level, treeline] and
    /// reaches up to MAX_TREE_REACH_ABOVE. Anchors can sit at margin origins / in
    /// neighbours, so gate on the candidate-window surface range. Skip the section
    /// when no anchor can reach it.
    fn place_trees(&self, sp: SectionPos, section: &mut Section, col: &ColumnGen) {
        let sec_lo = sp.cy * SECTION_SIZE as i32;
        let sec_hi = sec_lo + SECTION_SIZE as i32 - 1;
        let anchor_lo = col.cand_surf_min.max(SEA_LEVEL + 1);
        let anchor_hi = col.cand_surf_max.min(TREELINE);
        if anchor_lo > anchor_hi
            || !ranges_overlap(sec_lo, sec_hi, anchor_lo, anchor_hi + MAX_TREE_REACH_ABOVE)
        {
            return;
        }
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
        let plan = windows.plan.get_or_init(|| {
            let mut field = ColumnFeatureField::new(&windows.candidates, windows.support.as_ref());
            let (ox, oz) = (sp.cx * SECTION_SIZE as i32, sp.cz * SECTION_SIZE as i32);
            FeaturePlan::record(sp.cx, sp.cz, |ctx| {
                super::feature::place_trees(ctx, &mut field, self.seed, ox, oz)
            })
        });
        plan.apply(section);
    }

    /// The mod terrain replacement's 4096-block fill, or `None` when no
    /// replacement is registered / it failed (the engine fill+carve runs).
    fn replaced_terrain_fill(&self, sp: SectionPos, col: &ColumnGen) -> Option<Vec<u16>> {
        let hooks = self.hooks.as_ref()?;
        if !hooks.replaces(WorldgenStage::Terrain) {
            return None;
        }
        hooks.replace_terrain(&GenInputs {
            seed: self.seed,
            section_pos: [sp.cx, sp.cy, sp.cz],
            blocks: None,
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
    ) -> Result<bool, Deferred> {
        let Some(hooks) = &self.hooks else {
            return Ok(false);
        };
        if !hooks.replaces(stage) {
            return Ok(false);
        }
        let outcome = hooks.replace_stage(
            stage,
            &GenInputs {
                seed: self.seed,
                section_pos: [sp.cx, sp.cy, sp.cz],
                blocks: Some(section.blocks()),
                surface_heights: &col.top_surf,
                biomes: &col.biome,
            },
        );
        match outcome {
            FeatureOutcome::Plan(writes) => {
                apply_gen_plan(section, &writes);
                Ok(true)
            }
            FeatureOutcome::Skipped => Ok(false),
            FeatureOutcome::Deferred => Err(Deferred { feature: 0 }),
        }
    }

    /// Dispatch every feature attached after `stage`, in registration order,
    /// from the `first`-th on, each seeing the section as of the previous
    /// one's writes. A deferral names the feature to resume at.
    fn run_gen_features(
        &self,
        stage: WorldgenStage,
        sp: SectionPos,
        section: &mut Section,
        col: &ColumnGen,
        first: usize,
    ) -> Result<(), Deferred> {
        let Some(hooks) = &self.hooks else {
            return Ok(());
        };
        if !hooks.any_features_after(stage) {
            return Ok(());
        }
        let attached = hooks.features_after(stage);
        for (feature, &idx) in attached.iter().enumerate().skip(first) {
            let outcome = hooks.dispatch_feature(
                idx,
                &GenInputs {
                    seed: self.seed,
                    section_pos: [sp.cx, sp.cy, sp.cz],
                    blocks: Some(section.blocks()),
                    surface_heights: &col.top_surf,
                    biomes: &col.biome,
                },
            );
            match outcome {
                FeatureOutcome::Plan(writes) => apply_gen_plan(section, &writes),
                FeatureOutcome::Skipped => {}
                FeatureOutcome::Deferred => return Err(Deferred { feature }),
            }
        }
        Ok(())
    }

    /// A whole chunk column (y 0..256) — the chunk's column data and its
    /// sections cy 0..16 from [`generate_section`](Self::generate_section),
    /// assembled. There is no separate whole-chunk pipeline: what tooling, the
    /// parity hash and the audits read is exactly what the streamer generates,
    /// hooks included. Content below y=0 exists only in the cubic world.
    pub fn generate_chunk(&self, cx: i32, cz: i32) -> Chunk {
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
        chunk.dirty = true;
        chunk
    }
}

#[cfg(test)]
mod tests;
