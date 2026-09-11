//! The mod-gen hook seam, CONSUMER side.
//!
//! Worldgen owns the surface it consumes: the per-dispatch input contract
//! ([`GenInputs`]), the dispatch trait ([`GenHookDispatch`]), and the
//! process-wide installed-config registry. The modding host implements the
//! trait over its WASM instances and calls [`install`] at session init —
//! worldgen never links the WASM machinery.
//!
//! Dispatch granularity is per SECTION/STAGE (batched), never per block: the
//! one dynamic call per stage is noise next to the guest call it fronts.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};

use mod_api::WorldgenStage;
use petramond_world::section::BlockCube;

/// Validated generation layers. Template references share their compiled
/// storage across workers; no voxel expansion crosses the mod ABI.
#[derive(Default)]
pub struct GenerationPlan {
    pub blocks: Vec<([i32; 3], u16)>,
    pub structures: Vec<petramond_world::structure::Placement<'static>>,
    pub features: Vec<std::sync::Arc<crate::feature::placement::PlacedFeature>>,
}

/// What one feature or stage-replacement dispatch produced.
pub enum FeatureOutcome {
    /// Writes to apply.
    Plan(GenerationPlan),
    /// Nothing registered, or the hook failed and was skipped.
    Skipped,
    /// The hook could not settle a positional fact its writes depend on
    /// because another worker is deriving it: the whole section must be
    /// generated again later, and its eventual content does not depend on
    /// when.
    Deferred,
}

/// Borrowed inputs of one per-section hook dispatch (copied into the guest
/// call only when a hook actually fires).
pub struct GenInputs<'a> {
    pub seed: u32,
    pub section_pos: [i32; 3],
    /// Section snapshot at this attach point (`None` for climate/terrain,
    /// which run before the section has content).
    pub blocks: Option<&'a BlockCube>,
    /// Post-cave bare-ground top per column (`z*16 + x`).
    pub surface_heights: &'a [i32],
    /// Biome id per column (`z*16 + x`).
    pub biomes: &'a [u8],
}

/// What the worldgen driver asks of an installed hook config. Implemented by
/// the modding host's `GenHooks` (per-thread WASM instances behind it); every
/// method mirrors a driver call site, nothing more.
pub trait GenHookDispatch: Send + Sync {
    /// Identity of this config for per-thread generator cache keys.
    fn epoch(&self) -> u64;
    /// Whether `stage` has a registered replacement.
    fn replaces(&self, stage: WorldgenStage) -> bool;
    /// Climate replacement: the column's 256-entry biome map, or `None` when
    /// unregistered/failed (caller keeps the engine map).
    fn replace_climate(&self, inputs: &GenInputs) -> Option<Vec<u8>>;
    /// Terrain replacement: a full section fill, or `None`.
    fn replace_terrain(&self, inputs: &GenInputs) -> Option<Vec<u16>>;
    /// Stage replacement writes; `Skipped` = unregistered or failed (the
    /// caller runs the engine stage).
    fn replace_stage(&self, stage: WorldgenStage, inputs: &GenInputs) -> FeatureOutcome;
    /// Whether any feature attaches after `stage` (the driver's cheap gate).
    fn any_features_after(&self, stage: WorldgenStage) -> bool;
    /// Indices (dispatch order) of the features attached after `stage`.
    fn features_after(&self, stage: WorldgenStage) -> Vec<usize>;
    /// Dispatch feature `idx` for one section. `Skipped` = the feature
    /// failed (instance disabled with a logged error) or declined.
    fn dispatch_feature(&self, idx: usize, inputs: &GenInputs) -> FeatureOutcome;
}

static INSTALLED: RwLock<Option<Arc<dyn GenHookDispatch>>> = RwLock::new(None);
static INSTALLED_EPOCH: AtomicU64 = AtomicU64::new(0);
static NEXT_EPOCH: AtomicU64 = AtomicU64::new(1);

/// A fresh nonzero epoch for a new hook config (the implementor stamps it on
/// the config it is about to [`install`]).
pub fn next_epoch() -> u64 {
    NEXT_EPOCH.fetch_add(1, Ordering::Relaxed)
}

/// Install the session's hook config (or `None` for a hookless session).
/// Always bumps the epoch, so cached per-thread generators rebuild and capture
/// the new config. Called from the mod host's initialize, BEFORE any
/// generation for the new session is submitted.
pub fn install(hooks: Option<Arc<dyn GenHookDispatch>>) {
    let epoch = match &hooks {
        Some(h) => h.epoch(),
        None => next_epoch(),
    };
    *INSTALLED.write().unwrap() = hooks;
    INSTALLED_EPOCH.store(epoch, Ordering::Release);
}

/// The installed config, if any. Read at `ChunkGenerator` construction — never
/// per section.
pub fn active() -> Option<Arc<dyn GenHookDispatch>> {
    // Cheap out before touching the lock: 0 = nothing was ever installed
    // (tooling binaries and hookless test processes never pay the lock).
    if INSTALLED_EPOCH.load(Ordering::Acquire) == 0 {
        return None;
    }
    INSTALLED.read().unwrap().clone()
}

/// Identity of the installed config for per-thread generator cache keys
/// (`(seed, installed_epoch)`): one atomic load on the job hot path.
pub fn installed_epoch() -> u64 {
    INSTALLED_EPOCH.load(Ordering::Acquire)
}
