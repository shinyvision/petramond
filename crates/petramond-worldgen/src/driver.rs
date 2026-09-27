use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use mod_api::WorldgenStage;

use crate::colgen::{ColumnCore, MESH_BIOME_RADIUS, MESH_BIOME_SIDE};
use crate::hooks::{FeatureOutcome, GenHookDispatch, GenInputs};
use petramond_world::chunk::{
    idx, Chunk, SectionPos, CHUNK_SX, CHUNK_SY, CHUNK_SZ, SEA_LEVEL, SECTION_SIZE,
};
use petramond_world::section::{Section, SectionSummary};

use super::density::surface::SurfaceDensitySystem;
use super::feature::{
    apply_gen_plan, cached_feature_region, feature_candidate_bounds, feature_region_bounds,
    scatter, vegetation, ColumnFeatureField, FeaturePlan, SurfaceHeights, MAX_TREE_REACH_ABOVE,
    TREELINE,
};
use super::noise::cave_field::CaveField;
use super::region::RegionCells;

pub struct ChunkGenerator {
    seed: u32,
    surface_density: SurfaceDensitySystem,
    caves: CaveField,
    hooks: Option<Arc<dyn GenHookDispatch>>,
}

pub struct ColumnGen {
    pub cx: i32,
    pub cz: i32,
    core: ColumnCore,
    feature_windows: Option<FeatureWindows>,
}

pub struct FeatureWindows {
    plan: OnceLock<FeaturePlan>,
    candidates: RegionCells,
    support: Option<SurfaceHeights>,
}

impl ColumnGen {
    pub fn memory_bytes(&self) -> u64 {
        let core = &self.core;
        let base = std::mem::size_of::<Self>()
            + core.biome.len()
            + core.mesh_biome.len()
            + core.surf.len() * 4
            + core.top_surf.len() * 4;
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
    #[inline]
    pub fn biome_at(&self, x: usize, z: usize) -> u8 {
        self.core.biome[z * SECTION_SIZE + x]
    }
    #[inline]
    pub fn mesh_biome(&self) -> Arc<[u8]> {
        self.core.mesh_biome.clone()
    }
    #[inline]
    pub fn mesh_biome_slice(&self) -> &[u8] {
        &self.core.mesh_biome
    }
    #[inline]
    pub fn surface_y(&self, x: usize, z: usize) -> i32 {
        self.core.surf[z * SECTION_SIZE + x]
    }
    #[inline]
    pub fn heightmap_surface_y(&self, x: usize, z: usize) -> i32 {
        let i = z * SECTION_SIZE + x;
        if self.core.surf[i] < SEA_LEVEL {
            SEA_LEVEL
        } else {
            self.core.top_surf[i]
        }
    }
    #[inline]
    pub fn surf_range(&self) -> (i32, i32) {
        (self.core.surf_min, self.core.surf_max)
    }
    #[inline]
    pub fn content_top(&self) -> i32 {
        self.core.content_top
    }

    #[inline]
    pub fn has_feature_windows(&self) -> bool {
        self.feature_windows.is_some()
    }

    pub fn slimmed(&self) -> ColumnGen {
        ColumnGen {
            cx: self.cx,
            cz: self.cz,
            core: self.core.clone(),
            feature_windows: None,
        }
    }

    pub fn cache_record(&self, seed: u32) -> crate::colgen::ColumnGenRecord {
        crate::colgen::ColumnGenRecord {
            pos: petramond_world::chunk::ChunkPos::new(self.cx, self.cz),
            seed,
            core: self.core.clone(),
        }
    }

    pub fn from_cache_record(rec: crate::colgen::ColumnGenRecord) -> ColumnGen {
        ColumnGen {
            cx: rec.pos.cx,
            cz: rec.pos.cz,
            core: rec.core,
            feature_windows: None,
        }
    }

    #[inline]
    pub fn section_summary(&self, cy: i32) -> SectionSummary {
        if !SectionPos::cy_in_range(cy) {
            return SectionSummary::Unknown;
        }
        let core = &self.core;
        let y0 = cy * SECTION_SIZE as i32;
        let y1 = y0 + SECTION_SIZE as i32 - 1;
        if y0 > core.content_top {
            return SectionSummary::Empty;
        }
        if CaveField::section_may_carve(cy, core.surf_min, core.surf_max) {
            return SectionSummary::Mixed;
        }
        if y1 <= core.surf_min {
            return SectionSummary::FullOpaque;
        }
        if y0 > core.surf_max && y1 <= SEA_LEVEL {
            return SectionSummary::FullWater;
        }
        SectionSummary::Mixed
    }
}

struct Deferred {
    feature: usize,
}

const SECTION_STAGES: [WorldgenStage; 4] = [
    WorldgenStage::Terrain,
    WorldgenStage::Underground,
    WorldgenStage::Vegetation,
    WorldgenStage::Trees,
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Resume {
    Stage,
    Feature(usize),
}

pub struct PendingSection {
    sp: SectionPos,
    section: Section,
    stage: usize,
    resume: Resume,
}

impl PendingSection {
    pub fn pos(&self) -> SectionPos {
        self.sp
    }
}

pub enum SectionGen {
    Ready(Section),
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

    pub fn with_hooks(seed: u32, hooks: Option<Arc<dyn GenHookDispatch>>) -> Self {
        Self::with_caches(seed, hooks, crate::cache::installed())
    }

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

    pub fn installed_config() -> (u64, u64) {
        (
            crate::hooks::installed_epoch(),
            crate::cache::installed_epoch(),
        )
    }

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

    #[cfg(test)]
    pub(crate) fn with_caves(seed: u32, caves: CaveField) -> Self {
        Self {
            seed,
            surface_density: SurfaceDensitySystem::new(seed),
            caves,
            hooks: None,
        }
    }

    pub(crate) fn sources(&self) -> (&SurfaceDensitySystem, &CaveField) {
        (&self.surface_density, &self.caves)
    }

    pub fn has_gen_hooks(&self) -> bool {
        self.hooks.is_some()
    }

    pub fn region(&self, cx: i32, cz: i32) -> RegionCells {
        let (x0, z0, w, h) = super::feature::feature_region_bounds(cx * 16, cz * 16);
        self.surface_density.region(x0, z0, w, h)
    }

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

    pub fn missing_surface_tiles(&self, cx: i32, cz: i32) -> Vec<(i32, i32)> {
        const T: i32 = CHUNK_SX as i32;
        let (x0, z0, w, h) = feature_candidate_bounds(cx * T, cz * T);
        let tiles_x = x0.div_euclid(T)..=(x0 + w as i32 - 1).div_euclid(T);
        let tiles_z = z0.div_euclid(T)..=(z0 + h as i32 - 1).div_euclid(T);
        let memo = &self.caves.caches().terrain.surface_tiles;
        let context = self.caves.context();
        tiles_z
            .flat_map(|tcz| tiles_x.clone().map(move |tcx| (tcx, tcz)))
            .filter(|&(tcx, tcz)| !memo.contains(&(context, [tcx, tcz])))
            .collect()
    }

    pub fn biome_at(&self, wx: i32, wz: i32) -> petramond_world::biome::Biome {
        self.surface_density.biome_at(wx, wz)
    }

    pub fn generate_column_gen(&self, cx: i32, cz: i32) -> ColumnGen {
        let (ox, oz) = (cx * CHUNK_SX as i32, cz * CHUNK_SZ as i32);

        let (cx0, cz0, cw, ch) = feature_candidate_bounds(ox, oz);
        let (candidates, raw_surf) =
            cached_feature_region(&self.surface_density, &self.caves, cx0, cz0, cw, ch);

        let radius = MESH_BIOME_RADIUS as i32;
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
                let (_, b) = candidates.at(ox - radius + x as i32, oz - radius + z as i32);
                mesh_biome[z * MESH_BIOME_SIDE + x] = b.id();
            }
        }

        let windows = self.finish_feature_windows(ox, oz, candidates);

        let (mut cand_surf_min, mut cand_surf_max) = (i32::MAX, i32::MIN);
        for &s in &windows.candidates.surf {
            cand_surf_min = cand_surf_min.min(s);
            cand_surf_max = cand_surf_max.max(s);
        }

        top_surf.copy_from_slice(&self.caves.surfaces_after_caves(ox, oz, &surf));

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
                        let dst = (z + MESH_BIOME_RADIUS) * MESH_BIOME_SIDE + MESH_BIOME_RADIUS;
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
            core: ColumnCore {
                biome,
                mesh_biome: Arc::from(mesh_biome),
                surf,
                top_surf,
                surf_min,
                surf_max,
                cand_surf_min,
                cand_surf_max,
                content_top: cand_surf_max + MAX_TREE_REACH_ABOVE,
            },
            feature_windows: Some(windows),
        }
    }

    fn build_feature_windows(&self, cx: i32, cz: i32) -> FeatureWindows {
        let (ox, oz) = (cx * CHUNK_SX as i32, cz * CHUNK_SZ as i32);
        let (cx0, cz0, cw, ch) = feature_candidate_bounds(ox, oz);
        let (candidates, _raw) =
            cached_feature_region(&self.surface_density, &self.caves, cx0, cz0, cw, ch);
        self.finish_feature_windows(ox, oz, candidates)
    }

    fn finish_feature_windows(&self, ox: i32, oz: i32, candidates: RegionCells) -> FeatureWindows {
        let needs_support = candidates.biomes.iter().any(|b| {
            super::biome::trees::profile(*b).support
                == super::biome::trees::TreeSupport::RedwoodBase
        });

        let support = needs_support.then(|| {
            let (sx0, sz0, sw, sh) = feature_region_bounds(ox, oz);
            debug_assert_eq!(sw, sh);
            let (region, _raw) =
                cached_feature_region(&self.surface_density, &self.caves, sx0, sz0, sw, sh);
            SurfaceHeights::new(sx0, sz0, sw, region.surf)
        });

        FeatureWindows {
            plan: OnceLock::new(),
            candidates,
            support,
        }
    }

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

    pub fn resume_section(&self, pending: PendingSection, col: &ColumnGen) -> SectionGen {
        debug_assert_eq!((pending.sp.cx, pending.sp.cz), (col.cx, col.cz));
        self.advance(pending, col)
    }

    fn advance(&self, mut pending: PendingSection, col: &ColumnGen) -> SectionGen {
        while let Some(&stage) = SECTION_STAGES.get(pending.stage) {
            let sp = pending.sp;
            let first_feature = match pending.resume {
                Resume::Stage => {
                    if self
                        .run_stage(stage, sp, &mut pending.section, col)
                        .is_err()
                    {
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
            WorldgenStage::Underground => {
                if !self.run_stage_replacement(stage, sp, section, col)? {
                    let has_stone = sec_lo <= col.core.surf_max;
                    let (scatter_lo, scatter_hi) = scatter::y_span();
                    if has_stone && ranges_overlap(sec_lo, sec_hi, scatter_lo, scatter_hi) {
                        scatter::place_underground_section(section, self.seed);
                    }
                }
            }
            WorldgenStage::Vegetation => {
                if !self.run_stage_replacement(stage, sp, section, col)?
                    && col.core.surf_max >= SEA_LEVEL
                    && ranges_overlap(sec_lo, sec_hi, SEA_LEVEL + 1, col.core.surf_max + 1)
                {
                    vegetation::place_vegetation_section(
                        section,
                        &col.core.biome,
                        &col.core.surf,
                        &col.core.top_surf,
                        self.seed,
                    );
                }
            }
            WorldgenStage::Trees => {
                if !self.run_stage_replacement(stage, sp, section, col)? {
                    self.place_trees(sp, section, col);
                }
            }
            WorldgenStage::Climate => {}
        }
        Ok(())
    }

    /// Terrain fill (always), writes the block buffer in bulk and skips setter bookkeeping.
    /// So we recount the random-tick gate right here, before later stages use
    /// `set_block_raw`. Otherwise its incremental adjust underflows when a feature
    /// overwrites a random-tickable skin block (tree trunk over grass) while the
    /// count still reads zero.
    fn fill_terrain(&self, sp: SectionPos, section: &mut Section, col: &ColumnGen) {
        let engine_terrain = match self.replaced_terrain_fill(sp, col) {
            Some(fill) => {
                *section.blocks_mut() = petramond_world::section::BlockCube::from_ids(&fill);
                false
            }
            None if self
                .hooks
                .as_ref()
                .is_some_and(|h| h.replaces(WorldgenStage::Climate)) =>
            {
                self.surface_density
                    .fill_section(section, &col.core.biome, &col.core.surf);
                self.caves.carve_section(section, &col.core.surf);
                true
            }
            None => {
                *section.blocks_mut() =
                    crate::section_memo::terrain_cube(&self.surface_density, &self.caves, sp);
                true
            }
        };
        section.recompute_opaque_count();
        if engine_terrain {
            crate::section_memo::stamp_falls(&self.caves, sp, section);
        }
    }

    fn place_trees(&self, sp: SectionPos, section: &mut Section, col: &ColumnGen) {
        let sec_lo = sp.cy * SECTION_SIZE as i32;
        let sec_hi = sec_lo + SECTION_SIZE as i32 - 1;
        let anchor_lo = col.core.cand_surf_min.max(SEA_LEVEL + 1);
        let anchor_hi = col.core.cand_surf_max.min(TREELINE);
        if anchor_lo > anchor_hi
            || !ranges_overlap(sec_lo, sec_hi, anchor_lo, anchor_hi + MAX_TREE_REACH_ABOVE)
        {
            return;
        }
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

    fn replaced_terrain_fill(&self, sp: SectionPos, col: &ColumnGen) -> Option<Vec<u16>> {
        let hooks = self.hooks.as_ref()?;
        if !hooks.replaces(WorldgenStage::Terrain) {
            return None;
        }
        hooks.replace_terrain(&GenInputs {
            seed: self.seed,
            section_pos: [sp.cx, sp.cy, sp.cz],
            blocks: None,
            surface_heights: &col.core.top_surf,
            biomes: &col.core.biome,
        })
    }

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
                surface_heights: &col.core.top_surf,
                biomes: &col.core.biome,
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
                    surface_heights: &col.core.top_surf,
                    biomes: &col.core.biome,
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
