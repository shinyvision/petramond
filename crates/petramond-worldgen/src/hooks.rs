use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};

use mod_api::WorldgenStage;
use petramond_world::section::BlockCube;

#[derive(Default)]
pub struct GenerationPlan {
    pub blocks: Vec<([i32; 3], u16)>,
    pub structures: Vec<petramond_world::structure::Placement<'static>>,
    pub features: Vec<std::sync::Arc<crate::feature::placement::PlacedFeature>>,
}

pub enum FeatureOutcome {
    Plan(GenerationPlan),
    Skipped,
    Deferred,
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
