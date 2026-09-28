use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};

use mod_api::WorldgenStage;
use petramond_world::section::BlockCube;

/// A validated generation reply, applied in field order: box fills, plain writes, authored
/// cells, placed features, then structure templates.
#[derive(Default)]
pub struct GenerationPlan {
    pub fills: Vec<mod_api::GenFill>,
    pub blocks: Vec<([i32; 3], u16)>,
    pub authored: Vec<petramond_world::world::placement::authored::Cell>,
    pub structures: Vec<petramond_world::structure::Placement<'static>>,
    pub features: Vec<std::sync::Arc<crate::feature::placement::PlacedFeature>>,
}

impl GenerationPlan {
    pub fn is_empty(&self) -> bool {
        self.fills.is_empty()
            && self.blocks.is_empty()
            && self.authored.is_empty()
            && self.structures.is_empty()
            && self.features.is_empty()
    }
}

pub enum FeatureOutcome {
    Plan(GenerationPlan),
    Skipped,
    Deferred,
}

/// Columns generation features keep for themselves: the engine's trees stay out of them.
#[derive(Clone, Default)]
pub struct Claims(Vec<Arc<mod_api::ColumnMask>>);

impl Claims {
    pub fn new(masks: Vec<Arc<mod_api::ColumnMask>>) -> Claims {
        Claims(masks)
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    #[inline]
    pub fn claimed(&self, column: [i32; 2]) -> bool {
        self.0.iter().any(|m| m.has(column))
    }

    /// Whether a claim's bounds come within `reach` columns of `column`.
    pub fn near(&self, [x, z]: [i32; 2], reach: i32) -> bool {
        self.0.iter().any(|m| {
            let b = &m.bounds;
            b.min[0] - reach <= x
                && x <= b.max[0] + reach
                && b.min[1] - reach <= z
                && z <= b.max[1] + reach
        })
    }
}

pub struct GenInputs<'a> {
    pub seed: u32,
    pub section_pos: [i32; 3],
    pub blocks: Option<&'a BlockCube>,
    pub surface_heights: &'a [i32],
    pub biomes: &'a [u8],
}

pub trait GenHookDispatch: Send + Sync {
    fn epoch(&self) -> u64;
    fn replaces(&self, stage: WorldgenStage) -> bool;
    fn replace_climate(&self, inputs: &GenInputs) -> Option<Vec<u8>>;
    fn replace_terrain(&self, inputs: &GenInputs) -> Option<Vec<u16>>;
    fn replace_stage(&self, stage: WorldgenStage, inputs: &GenInputs) -> FeatureOutcome;
    fn any_features_after(&self, stage: WorldgenStage) -> bool;
    fn features_after(&self, stage: WorldgenStage) -> Vec<usize>;
    fn dispatch_feature(&self, idx: usize, inputs: &GenInputs) -> FeatureOutcome;
    fn wait_deferred(&self);
    /// Every claim touching the columns `min..=max`; `None` when one is still being worked out
    /// on another worker, which [`wait_deferred`](GenHookDispatch::wait_deferred) waits for.
    fn claims(&self, _min: [i32; 2], _max: [i32; 2]) -> Option<Claims> {
        Some(Claims::default())
    }
}

static INSTALLED: RwLock<Option<Arc<dyn GenHookDispatch>>> = RwLock::new(None);
static INSTALLED_EPOCH: AtomicU64 = AtomicU64::new(0);
static NEXT_EPOCH: AtomicU64 = AtomicU64::new(1);

pub fn next_epoch() -> u64 {
    NEXT_EPOCH.fetch_add(1, Ordering::Relaxed)
}

pub fn install(hooks: Option<Arc<dyn GenHookDispatch>>) {
    let epoch = match &hooks {
        Some(h) => h.epoch(),
        None => next_epoch(),
    };
    *INSTALLED.write().unwrap() = hooks;
    INSTALLED_EPOCH.store(epoch, Ordering::Release);
}

pub fn active() -> Option<Arc<dyn GenHookDispatch>> {
    if INSTALLED_EPOCH.load(Ordering::Acquire) == 0 {
        return None;
    }
    INSTALLED.read().unwrap().clone()
}

pub fn installed_epoch() -> u64 {
    INSTALLED_EPOCH.load(Ordering::Acquire)
}
