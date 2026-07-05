//! Worldgen hooks (modding Phase 4): the registered feature/stage-replacement
//! config, its process-wide installation, and the per-thread guest dispatch.
//!
//! # Shape
//!
//! `mod_init` on the MAIN load records worldgen registrations; `ModHost::
//! initialize` folds them into one immutable [`GenHooks`] and [`install`]s it
//! process-wide. `ChunkGenerator::new` captures the installed config (an `Arc`
//! clone), so every generator built for a session — worker threads, the main
//! thread, tooling — agrees on the hook set; the [`installed_epoch`] rides the
//! per-thread generator cache keys so a new session's config replaces stale
//! cached generators.
//!
//! # Per-thread instances
//!
//! `wasmtime::Store` is not `Sync`, so each thread that dispatches a gen hook
//! lazily instantiates its own guest from the 2b compiled-module cache
//! (mirroring the thread-local `GENERATOR` in `src/worker.rs`). Gen instances
//! share NOTHING with the tick instance: separate wasm memories, `mod_init`
//! run detached (no `SimCtx` — registrations are accepted-and-ignored,
//! sim-scoped calls error). Hook replies must therefore be pure functions of
//! the dispatched inputs — the determinism contract in `mod-api`.
//!
//! # Failure policy
//!
//! Trap / deadline / protocol break / invalid ids disable that THREAD's
//! instance with a visible error; a failed FEATURE is skipped, a failed stage
//! REPLACEMENT falls back to the ENGINE stage (logged loudly, once per stage).
//!
//! # Empty-hook cost
//!
//! With no hooks installed the whole system is one `Option` on the generator
//! (checked per stage) plus one atomic epoch load per cached-generator lookup
//! — no snapshots, no allocation, byte-identical output (the genparity pin).

use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, RwLock};

use mod_api::{GuestCall, GuestRet, WorldgenStage};
use wasmtime::Module;

use crate::biome::BIOME_COUNT;
use crate::block::Block;
use crate::chunk::{SEA_LEVEL, SECTION_VOLUME};

use super::host::Registration;
use super::instance::ModInstance;

/// The addressable stages, in pipeline order (indexes [`GenHooks`] arrays).
const STAGE_COUNT: usize = 5;

fn stage_index(stage: WorldgenStage) -> usize {
    match stage {
        WorldgenStage::Climate => 0,
        WorldgenStage::Terrain => 1,
        WorldgenStage::Underground => 2,
        WorldgenStage::Vegetation => 3,
        WorldgenStage::Trees => 4,
    }
}

const ALL_STAGES: [WorldgenStage; STAGE_COUNT] = [
    WorldgenStage::Climate,
    WorldgenStage::Terrain,
    WorldgenStage::Underground,
    WorldgenStage::Vegetation,
    WorldgenStage::Trees,
];

// ---------------------------------------------------------------------------
// The immutable hook config.
// ---------------------------------------------------------------------------

struct GenModule {
    id: String,
    module: Module,
    /// Gen registrations the MAIN load recorded — per-thread inits are
    /// validated (cheaply, by count) against this.
    expected_gen_regs: usize,
}

struct FeatureHook {
    mod_idx: usize,
    feature_id: u32,
    stage_idx: usize,
}

struct StageHook {
    mod_idx: usize,
    callback_id: u32,
}

/// One session's worldgen hook set. Immutable after build; shared by `Arc`
/// with every `ChunkGenerator` of the session.
pub(crate) struct GenHooks {
    epoch: u64,
    seed: u32,
    mods: Vec<GenModule>,
    /// Registration order == (load order, per-mod order) — the dispatch order.
    features: Vec<FeatureHook>,
    replacements: [Option<StageHook>; STAGE_COUNT],
    /// One loud engine-fallback line per stage per session, not per section.
    fallback_logged: [AtomicBool; STAGE_COUNT],
}

/// Borrowed inputs of one per-section hook dispatch (copied into the guest
/// call only when a hook actually fires).
pub(crate) struct GenInputs<'a> {
    pub seed: u32,
    pub section_pos: [i32; 3],
    /// Section snapshot at this attach point (empty for climate/terrain).
    pub blocks: &'a [u8],
    /// Post-cave bare-ground top per column (`z*16 + x`).
    pub surface_heights: &'a [i32],
    /// Biome id per column (`z*16 + x`).
    pub biomes: &'a [u8],
}

impl GenHooks {
    /// Whether `stage` has a registered replacement.
    pub(crate) fn replaces(&self, stage: WorldgenStage) -> bool {
        self.replacements[stage_index(stage)].is_some()
    }

    /// Whether any feature attaches after `stage` (the driver's cheap gate).
    pub(crate) fn any_features_after(&self, stage: WorldgenStage) -> bool {
        let i = stage_index(stage);
        self.features.iter().any(|f| f.stage_idx == i)
    }

    /// Indices (dispatch order) of the features attached after `stage`.
    pub(crate) fn features_after(&self, stage: WorldgenStage) -> Vec<usize> {
        let i = stage_index(stage);
        (0..self.features.len())
            .filter(|&f| self.features[f].stage_idx == i)
            .collect()
    }

    /// Dispatch feature `idx` for one section. `None` = the feature failed
    /// (instance disabled with a logged error) and is skipped.
    pub(crate) fn dispatch_feature(
        &self,
        idx: usize,
        inputs: &GenInputs,
    ) -> Option<Vec<([i32; 3], u8)>> {
        let hook = &self.features[idx];
        let call = GuestCall::GenFeature {
            feature_id: hook.feature_id,
            section_pos: inputs.section_pos,
            seed: inputs.seed,
            blocks: inputs.blocks.to_vec(),
            surface_heights: inputs.surface_heights.to_vec(),
            biomes: inputs.biomes.to_vec(),
            sea_level: SEA_LEVEL,
        };
        self.dispatch(hook.mod_idx, &call, |ret| match ret {
            GuestRet::GenWrites(w) => validated_writes(w),
            other => Err(reply_shape("GenFeature", "GenWrites", &other)),
        })
    }

    /// Run the registered replacement of a write-list stage
    /// (underground/vegetation/trees). `None` = no replacement registered OR
    /// it failed — either way the caller runs the ENGINE stage.
    pub(crate) fn replace_stage(
        &self,
        stage: WorldgenStage,
        inputs: &GenInputs,
    ) -> Option<Vec<([i32; 3], u8)>> {
        let hook = self.replacements[stage_index(stage)].as_ref()?;
        let call = self.stage_call(hook, stage, inputs);
        let res = self.dispatch(hook.mod_idx, &call, |ret| match ret {
            GuestRet::GenWrites(w) => validated_writes(w),
            other => Err(reply_shape("GenStage", "GenWrites", &other)),
        });
        if res.is_none() {
            self.log_fallback(stage, hook.mod_idx);
        }
        res
    }

    /// Run the registered terrain replacement: the full 4096-block fill.
    /// `None` = unregistered or failed (engine fill+carve runs).
    pub(crate) fn replace_terrain(&self, inputs: &GenInputs) -> Option<Vec<u8>> {
        let stage = WorldgenStage::Terrain;
        let hook = self.replacements[stage_index(stage)].as_ref()?;
        let call = self.stage_call(hook, stage, inputs);
        let res = self.dispatch(hook.mod_idx, &call, |ret| match ret {
            GuestRet::GenBlocks(fill) => {
                if fill.len() != SECTION_VOLUME {
                    return Err(format!(
                        "terrain replacement returned {} bytes; a section fill is exactly {}",
                        fill.len(),
                        SECTION_VOLUME
                    ));
                }
                let registered = Block::all().len();
                if let Some(&bad) = fill.iter().find(|&&id| id as usize >= registered) {
                    return Err(format!(
                        "terrain replacement wrote unregistered block id {bad}"
                    ));
                }
                Ok(fill)
            }
            other => Err(reply_shape("GenStage", "GenBlocks", &other)),
        });
        if res.is_none() {
            self.log_fallback(stage, hook.mod_idx);
        }
        res
    }

    /// Run the registered climate replacement: the 256-entry column biome map.
    /// `None` = unregistered or failed (the engine map stands).
    pub(crate) fn replace_climate(&self, inputs: &GenInputs) -> Option<Vec<u8>> {
        let stage = WorldgenStage::Climate;
        let hook = self.replacements[stage_index(stage)].as_ref()?;
        let call = self.stage_call(hook, stage, inputs);
        let res = self.dispatch(hook.mod_idx, &call, |ret| match ret {
            GuestRet::GenBiomes(map) => {
                if map.len() != 256 {
                    return Err(format!(
                        "climate replacement returned {} biomes; a column map is exactly 256",
                        map.len()
                    ));
                }
                if let Some(&bad) = map.iter().find(|&&id| id == 0 || id as usize > BIOME_COUNT) {
                    return Err(format!("climate replacement wrote invalid biome id {bad}"));
                }
                Ok(map)
            }
            other => Err(reply_shape("GenStage", "GenBiomes", &other)),
        });
        if res.is_none() {
            self.log_fallback(stage, hook.mod_idx);
        }
        res
    }

    fn stage_call(&self, hook: &StageHook, stage: WorldgenStage, inputs: &GenInputs) -> GuestCall {
        GuestCall::GenStage {
            callback_id: hook.callback_id,
            stage,
            section_pos: inputs.section_pos,
            seed: inputs.seed,
            blocks: inputs.blocks.to_vec(),
            surface_heights: inputs.surface_heights.to_vec(),
            biomes: inputs.biomes.to_vec(),
            sea_level: SEA_LEVEL,
        }
    }

    fn log_fallback(&self, stage: WorldgenStage, mod_idx: usize) {
        if !self.fallback_logged[stage_index(stage)].swap(true, Ordering::Relaxed) {
            log::error!(
                "worldgen stage {stage:?}: replacement by mod '{}' failed; the ENGINE stage \
                 is generating instead (see the disable error above for the cause)",
                self.mods[mod_idx].id
            );
        }
    }

    /// Dispatch one call into this thread's instance of `mod_idx`, validating
    /// the reply. Any failure (instantiation, trap, deadline, shape, ids)
    /// disables that thread's instance and yields `None`.
    fn dispatch<T>(
        &self,
        mod_idx: usize,
        call: &GuestCall,
        validate: impl FnOnce(GuestRet) -> Result<T, String>,
    ) -> Option<T> {
        THREAD_SLOTS.with(|cell| {
            let mut t = cell.borrow_mut();
            if t.epoch != self.epoch {
                t.epoch = self.epoch;
                t.slots.clear();
            }
            if t.slots.len() < self.mods.len() {
                t.slots.resize_with(self.mods.len(), || Slot::Empty);
            }
            if matches!(t.slots[mod_idx], Slot::Empty) {
                t.slots[mod_idx] = match self.instantiate(mod_idx) {
                    Some(inst) => Slot::Live(Box::new(inst)),
                    None => Slot::Failed,
                };
            }
            let Slot::Live(inst) = &mut t.slots[mod_idx] else {
                return None;
            };
            let ret = inst.call_guest_detached(call)?;
            match validate(ret) {
                Ok(v) => Some(v),
                Err(why) => {
                    inst.disable(&why);
                    None
                }
            }
        })
    }

    /// Build this thread's instance of `mod_idx` and run its detached init.
    fn instantiate(&self, mod_idx: usize) -> Option<ModInstance> {
        let m = &self.mods[mod_idx];
        let mut inst = match ModInstance::from_module(&m.id, &m.module, self.seed) {
            Ok(inst) => inst,
            Err(e) => {
                log::error!(
                    "mod '{}': worldgen instance failed to instantiate: {e}",
                    m.id
                );
                return None;
            }
        };
        inst.call_init_detached();
        if inst.disabled() {
            return None;
        }
        // Registrations from THIS init are accepted-and-ignored (the main load
        // already recorded them); validate the cheap invariant only.
        let gen_regs = inst
            .take_registrations()
            .iter()
            .filter(|r| r.is_gen())
            .count();
        if gen_regs != m.expected_gen_regs {
            log::warn!(
                "mod '{}': a worldgen instance registered {gen_regs} gen hook(s) but the main \
                 load recorded {}; mod_init must register deterministically",
                m.id,
                m.expected_gen_regs
            );
        }
        Some(inst)
    }
}

fn reply_shape(call: &str, expected: &str, got: &GuestRet) -> String {
    let got = match got {
        GuestRet::Unit => "Unit",
        GuestRet::Event { .. } => "Event",
        GuestRet::GenWrites(_) => "GenWrites",
        GuestRet::GenBlocks(_) => "GenBlocks",
        GuestRet::GenBiomes(_) => "GenBiomes",
    };
    format!("{call} expected a {expected} reply, got {got}")
}

/// Validate a write list's block ids against the loaded registry — an
/// unregistered id must never reach a section buffer.
fn validated_writes(w: Vec<mod_api::GenWrite>) -> Result<Vec<([i32; 3], u8)>, String> {
    let registered = Block::all().len();
    if let Some((_, bad)) = w.iter().find(|(_, id)| id.0 as usize >= registered) {
        return Err(format!(
            "worldgen write with unregistered block id {}",
            bad.0
        ));
    }
    Ok(w.into_iter().map(|(p, id)| (p, id.0)).collect())
}

enum Slot {
    Empty,
    Failed,
    Live(Box<ModInstance>),
}

struct ThreadSlots {
    epoch: u64,
    slots: Vec<Slot>,
}

thread_local! {
    /// This thread's gen instances, keyed by the config epoch (a new session's
    /// config drops the previous session's instances lazily).
    static THREAD_SLOTS: RefCell<ThreadSlots> = RefCell::new(ThreadSlots {
        epoch: 0,
        slots: Vec::new(),
    });
}

// ---------------------------------------------------------------------------
// Builder (fed by ModHost::initialize from the main-load registrations).
// ---------------------------------------------------------------------------

pub(crate) struct GenHooksBuilder {
    seed: u32,
    mods: Vec<GenModule>,
    features: Vec<FeatureHook>,
    replacements: [Option<StageHook>; STAGE_COUNT],
}

impl GenHooksBuilder {
    pub(crate) fn new(seed: u32) -> Self {
        Self {
            seed,
            mods: Vec::new(),
            features: Vec::new(),
            replacements: Default::default(),
        }
    }

    /// Fold one main-load registration in (no-op for non-gen registrations).
    pub(super) fn add_registration(&mut self, mod_id: &str, module: &Module, reg: &Registration) {
        match *reg {
            Registration::WorldgenFeature { stage, feature_id } => {
                self.add_feature(mod_id, module, stage, feature_id)
            }
            Registration::StageReplacement { stage, callback_id } => {
                self.add_stage_replacement(mod_id, module, stage, callback_id)
            }
            Registration::Generator { callback_id } => {
                self.add_generator(mod_id, module, callback_id)
            }
            Registration::TickSystem { .. } | Registration::EventHandler { .. } => {}
        }
    }

    pub(crate) fn add_feature(
        &mut self,
        mod_id: &str,
        module: &Module,
        stage: WorldgenStage,
        feature_id: u32,
    ) {
        let mod_idx = self.mod_index(mod_id, module);
        self.features.push(FeatureHook {
            mod_idx,
            feature_id,
            stage_idx: stage_index(stage),
        });
    }

    pub(crate) fn add_stage_replacement(
        &mut self,
        mod_id: &str,
        module: &Module,
        stage: WorldgenStage,
        callback_id: u32,
    ) {
        let mod_idx = self.mod_index(mod_id, module);
        let slot = &mut self.replacements[stage_index(stage)];
        if let Some(prev) = slot.as_ref() {
            log::warn!(
                "worldgen stage {stage:?}: mod '{}' already registered a replacement; \
                 mod '{}' is later in load order and wins",
                self.mods[prev.mod_idx].id,
                mod_id
            );
        }
        *slot = Some(StageHook {
            mod_idx,
            callback_id,
        });
    }

    /// Whole-generator replacement == every stage replaced by `callback_id`
    /// (the guest switches on the dispatched stage).
    pub(crate) fn add_generator(&mut self, mod_id: &str, module: &Module, callback_id: u32) {
        for stage in ALL_STAGES {
            self.add_stage_replacement(mod_id, module, stage, callback_id);
        }
    }

    fn mod_index(&mut self, mod_id: &str, module: &Module) -> usize {
        let idx = match self.mods.iter().position(|m| m.id == mod_id) {
            Some(idx) => idx,
            None => {
                self.mods.push(GenModule {
                    id: mod_id.to_owned(),
                    module: module.clone(),
                    expected_gen_regs: 0,
                });
                self.mods.len() - 1
            }
        };
        self.mods[idx].expected_gen_regs += 1;
        idx
    }

    /// `None` when nothing registered — the empty-hook fast path.
    pub(crate) fn build(self) -> Option<Arc<GenHooks>> {
        if self.features.is_empty() && self.replacements.iter().all(Option::is_none) {
            return None;
        }
        Some(Arc::new(GenHooks {
            epoch: NEXT_EPOCH.fetch_add(1, Ordering::Relaxed),
            seed: self.seed,
            mods: self.mods,
            features: self.features,
            replacements: self.replacements,
            fallback_logged: std::array::from_fn(|_| AtomicBool::new(false)),
        }))
    }
}

// ---------------------------------------------------------------------------
// Process-wide installation (captured by ChunkGenerator::new).
// ---------------------------------------------------------------------------

static NEXT_EPOCH: AtomicU64 = AtomicU64::new(1);
static INSTALLED_EPOCH: AtomicU64 = AtomicU64::new(0);
static INSTALLED: RwLock<Option<Arc<GenHooks>>> = RwLock::new(None);

/// Install the session's hook config (or `None` for a hookless session).
/// Always bumps the epoch, so cached per-thread generators rebuild and capture
/// the new config. Called from `ModHost::initialize`, BEFORE any generation
/// for the new session is submitted.
pub(crate) fn install(hooks: Option<Arc<GenHooks>>) {
    let epoch = match &hooks {
        Some(h) => h.epoch,
        None => NEXT_EPOCH.fetch_add(1, Ordering::Relaxed),
    };
    *INSTALLED.write().unwrap() = hooks;
    INSTALLED_EPOCH.store(epoch, Ordering::Release);
}

/// The installed config, if any. Read at `ChunkGenerator` construction — never
/// per section.
pub(crate) fn active() -> Option<Arc<GenHooks>> {
    // Cheap out before touching the lock: 0 = nothing was ever installed
    // (tooling binaries and hookless test processes never pay the lock).
    if INSTALLED_EPOCH.load(Ordering::Acquire) == 0 {
        return None;
    }
    INSTALLED.read().unwrap().clone()
}

/// Identity of the installed config for per-thread generator cache keys
/// (`(seed, installed_epoch)`): one atomic load on the job hot path.
pub(crate) fn installed_epoch() -> u64 {
    INSTALLED_EPOCH.load(Ordering::Acquire)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chunk::SectionPos;
    use crate::worldgen::driver::ChunkGenerator;

    /// A minimal guest whose init succeeds and whose every dispatch traps —
    /// the "runaway/broken gen mod" for the fallback contract.
    fn trapping_module() -> Module {
        let wat = r#"(module
  (import "env" "host_dispatch" (func $hd (param i32 i32) (result i64)))
  (memory (export "memory") 1)
  (func (export "mod_init"))
  (func (export "mod_alloc") (param i32) (result i32) (i32.const 4096))
  (func (export "mod_free") (param i32 i32))
  (func (export "mod_dispatch") (param i32 i32) (result i64) unreachable))"#;
        Module::new(super::super::host::engine(), wat.as_bytes()).expect("assemble trap guest")
    }

    /// Conflict contract: two mods replacing the same stage → LAST in load
    /// order wins; `RegisterGenerator` claims every stage and later
    /// stage-specific replacements override it per stage.
    #[test]
    fn stage_replacement_conflicts_resolve_to_last_in_load_order() {
        let module = trapping_module();
        let mut b = GenHooksBuilder::new(1);
        b.add_generator("alpha", &module, 7);
        b.add_stage_replacement("beta", &module, WorldgenStage::Terrain, 9);
        let hooks = b.build().expect("hooks registered");

        for stage in ALL_STAGES {
            assert!(hooks.replaces(stage), "{stage:?} is replaced");
        }
        let terrain = hooks.replacements[stage_index(WorldgenStage::Terrain)]
            .as_ref()
            .unwrap();
        assert_eq!(hooks.mods[terrain.mod_idx].id, "beta", "later mod wins");
        assert_eq!(terrain.callback_id, 9);
        let climate = hooks.replacements[stage_index(WorldgenStage::Climate)]
            .as_ref()
            .unwrap();
        assert_eq!(hooks.mods[climate.mod_idx].id, "alpha");

        // Nothing registered = no config = the empty fast path.
        assert!(GenHooksBuilder::new(1).build().is_none());
    }

    /// The Phase 4 proof against the REAL smoke mod: its worldgen feature is
    /// deterministic (same bytes per (seed, section) across regenerations,
    /// threads, generation order, and the column-vs-section path) and
    /// seam-consistent (a pillar crossing a section border materialises
    /// identically from both sections' independent calls). Registry-visible
    /// pack content is required, so the assertions run in a child process
    /// with `LLAMACRAFT_MODS` pointing at a fixture install of the pack
    /// (the 2a pattern — the registry is a process-wide LazyLock).
    #[test]
    fn smoke_gen_feature_deterministic_and_seam_consistent() {
        let Some(wasm) = super::super::tests::built_mod_wasm("smoke") else {
            return;
        };
        let root = std::env::temp_dir().join(format!("llamacraft-genhooks-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let pack = root.join("mods/smoke");
        std::fs::create_dir_all(&pack).unwrap();
        let src = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("mods-src/smoke/pack");
        // Recursive: the pack ships subdirectories (ui/documents etc.), not
        // just top-level catalogs.
        fn copy_tree(src: &std::path::Path, dst: &std::path::Path) {
            std::fs::create_dir_all(dst).unwrap();
            for entry in std::fs::read_dir(src).unwrap() {
                let entry = entry.unwrap();
                let to = dst.join(entry.file_name());
                if entry.path().is_dir() {
                    copy_tree(&entry.path(), &to);
                } else {
                    std::fs::copy(entry.path(), &to).unwrap();
                }
            }
        }
        copy_tree(&src, &pack);
        std::fs::copy(&wasm, pack.join("mod.wasm")).unwrap();

        let exe = std::env::current_exe().expect("test binary path");
        let out = std::process::Command::new(exe)
            .arg("modding::gen::tests::smoke_gen_feature_inner")
            .arg("--exact")
            .arg("--ignored")
            .arg("--nocapture")
            .env("LLAMACRAFT_MODS", root.join("mods"))
            .output()
            .expect("spawn test binary");
        let _ = std::fs::remove_dir_all(&root);
        assert!(
            out.status.success(),
            "inner test failed\n--- stdout ---\n{}\n--- stderr ---\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr),
        );
    }

    /// Runs ONLY in the child process spawned above (needs `LLAMACRAFT_MODS`
    /// pointing at the smoke pack before first registry touch). Drives the
    /// full production flow: pack discovery → `ModHost::load` → `initialize`
    /// (which installs the gen hook config) → `ChunkGenerator::new` capture.
    #[test]
    #[ignore = "spawned by smoke_gen_feature_deterministic_and_seam_consistent with a fixture pack env"]
    fn smoke_gen_feature_inner() {
        use crate::chunk::{CHUNK_SY, SECTION_SIZE};
        use crate::events::{EventBus, TickSystems};
        use crate::mathh::Vec3;
        use crate::player::Player;
        use crate::world::World;
        use crate::worldgen::generate_chunk_with;

        // A seed whose scan region provably contains boundary-crossing pillars
        // (surfaces at y ≡ 13..15 mod 16 under the rarity roll).
        let seed: u32 = 0x1234_5678;
        let mut world = World::new(seed, 1);
        let mut player = Player::new(Vec3::new(0.0, 80.0, 0.0));
        let mut bus = EventBus::default();
        let mut systems = TickSystems::default();
        let mut host = super::super::ModHost::load(seed, &Default::default());
        let mut next_spatial_sound_handle = 1;
        host.initialize(
            &mut world,
            &mut player,
            &mut bus,
            &mut systems,
            &mut next_spatial_sound_handle,
        );

        let hooks = active().expect("the smoke mod registered a worldgen feature");
        assert!(hooks.any_features_after(WorldgenStage::Trees));
        let smoke_id = crate::registry::names()
            .blocks
            .id("smoke:smoke_block")
            .expect("the pack registered smoke:smoke_block");

        let generator = ChunkGenerator::new(seed);
        assert!(
            generator.has_gen_hooks(),
            "ChunkGenerator::new captures the installed hooks"
        );

        // --- (a) column-path == section-path, and regeneration is stable ----
        for &(cx, cz) in &[(0, 0), (2, -1)] {
            let chunk = generate_chunk_with(&generator, cx, cz);
            let col = generator.generate_column_gen(cx, cz);
            for cy in 0..(CHUNK_SY / SECTION_SIZE) as i32 {
                let sp = SectionPos::new(cx, cy, cz);
                let s1 = generator.generate_section(sp, &col);
                let s2 = generator.generate_section(sp, &col);
                assert_eq!(
                    s1.blocks_slice(),
                    s2.blocks_slice(),
                    "regeneration differs at ({cx},{cy},{cz})"
                );
                for ly in 0..SECTION_SIZE {
                    let wy = cy as usize * SECTION_SIZE + ly;
                    for z in 0..SECTION_SIZE {
                        for x in 0..SECTION_SIZE {
                            assert_eq!(
                                s1.block_raw(x, ly, z),
                                chunk.block_raw(x, wy, z),
                                "path mismatch at ({cx},{cy},{cz}) local ({x},{ly},{z})"
                            );
                        }
                    }
                }
            }
        }

        // --- (b) presence + vertical seam consistency ------------------------
        // Scan a region: collect every smoke cell per column. Each column's
        // cells must form ONE contiguous vertical run (a gap AT a border = a
        // seam cut; runs shortened by tree canopies — the per-cell air
        // predicate — stay contiguous), at least one run must cross a section
        // border, and every crossing run must reach the full (max observed)
        // length — a seam bug truncates exactly the boundary-crossing runs.
        let radius = 5i32;
        let mut runs: Vec<Vec<i32>> = Vec::new();
        let mut crossings = 0usize;
        for cz in -radius..=radius {
            for cx in -radius..=radius {
                let col = generator.generate_column_gen(cx, cz);
                // The band of sections that can hold this chunk's pillars:
                // anchors are the columns' (cave-aware) ground tops.
                let (mut lo, mut hi) = (i32::MAX, i32::MIN);
                for z in 0..SECTION_SIZE {
                    for x in 0..SECTION_SIZE {
                        let h = col.heightmap_surface_y(x, z);
                        lo = lo.min(h);
                        hi = hi.max(h);
                    }
                }
                let cy_band = ((lo + 1) >> 4).max(0)..=((hi + 8) >> 4).min(15);
                let mut cells: std::collections::HashMap<(usize, usize), Vec<i32>> =
                    std::collections::HashMap::new();
                for cy in cy_band {
                    let s = generator.generate_section(SectionPos::new(cx, cy, cz), &col);
                    for ly in 0..SECTION_SIZE {
                        for z in 0..SECTION_SIZE {
                            for x in 0..SECTION_SIZE {
                                if s.block_raw(x, ly, z) == smoke_id {
                                    cells
                                        .entry((x, z))
                                        .or_default()
                                        .push(cy * SECTION_SIZE as i32 + ly as i32);
                                }
                            }
                        }
                    }
                }
                for (_, mut ys) in cells {
                    ys.sort_unstable();
                    assert!(
                        ys.windows(2).all(|w| w[1] == w[0] + 1),
                        "smoke run has a gap at chunk ({cx},{cz}): {ys:?} — a seam cut"
                    );
                    if ys.iter().any(|y| y % 16 == 0) && ys[0] % 16 != 0 {
                        crossings += 1;
                    }
                    runs.push(ys);
                }
            }
        }
        assert!(
            !runs.is_empty(),
            "the scan found no smoke pillars; widen the radius or change the seed"
        );
        let max_len = runs.iter().map(Vec::len).max().unwrap();
        for run in &runs {
            let crosses = run.iter().any(|y| y % 16 == 0) && run[0] % 16 != 0;
            assert!(
                !crosses || run.len() == max_len,
                "a boundary-crossing run is truncated ({run:?}, full length {max_len}) — \
                 the upper section dropped its share"
            );
        }
        assert!(
            crossings > 0,
            "no pillar crossed a section border in the scan; widen the radius or change the seed"
        );

        // --- order + thread independence on a crossing pair ------------------
        let crossing = runs
            .iter()
            .find(|r| r.iter().any(|y| y % 16 == 0) && r[0] % 16 != 0)
            .expect("crossings > 0");
        // Recover the crossing column's chunk by rescanning cheaply: the run's
        // ys pin the two cy values; find the chunk owning it via a fresh scan.
        let (below_cy, above_cy) = (crossing[0] / 16, crossing[crossing.len() - 1] / 16);
        assert_ne!(below_cy, above_cy);
        let mut pair_chunk = None;
        'outer: for cz in -radius..=radius {
            for cx in -radius..=radius {
                let col = generator.generate_column_gen(cx, cz);
                let s = generator.generate_section(SectionPos::new(cx, above_cy, cz), &col);
                for z in 0..SECTION_SIZE {
                    for x in 0..SECTION_SIZE {
                        if s.block_raw(
                            x,
                            (crossing[crossing.len() - 1] - above_cy * 16) as usize,
                            z,
                        ) == smoke_id
                        {
                            pair_chunk = Some((cx, cz));
                            break 'outer;
                        }
                    }
                }
            }
        }
        let (cx, cz) = pair_chunk.expect("the crossing run's chunk is in the scanned region");
        let generate_pair = move |flip: bool| {
            std::thread::spawn(move || {
                let g = ChunkGenerator::new(seed); // fresh thread, fresh gen instance
                let col = g.generate_column_gen(cx, cz);
                let (first, second) = if flip {
                    (above_cy, below_cy)
                } else {
                    (below_cy, above_cy)
                };
                let a = g.generate_section(SectionPos::new(cx, first, cz), &col);
                let b = g.generate_section(SectionPos::new(cx, second, cz), &col);
                let mut out = [
                    (first, a.blocks_slice().to_vec()),
                    (second, b.blocks_slice().to_vec()),
                ];
                out.sort_by_key(|(cy, _)| *cy);
                out
            })
            .join()
            .expect("gen thread")
        };
        assert_eq!(
            generate_pair(false),
            generate_pair(true),
            "section bytes depend on generation order or thread"
        );
    }

    /// Failure contract: a trapping replacement falls back to the ENGINE
    /// stage and a trapping feature is skipped — the generated section is
    /// byte-identical to a hookless generator's.
    #[test]
    fn trapping_gen_mod_falls_back_to_the_engine_stage() {
        let module = trapping_module();
        let mut b = GenHooksBuilder::new(0x312);
        b.add_stage_replacement("hostile", &module, WorldgenStage::Terrain, 1);
        b.add_stage_replacement("hostile", &module, WorldgenStage::Vegetation, 2);
        b.add_feature("hostile", &module, WorldgenStage::Trees, 3);
        let hooks = b.build().expect("hooks registered");

        let seed = 0x312;
        let hooked = ChunkGenerator::with_hooks(seed, Some(hooks));
        let engine = ChunkGenerator::with_hooks(seed, None);
        for &(cx, cy, cz) in &[(0, 3, 0), (1, 4, -1), (-2, 2, 5)] {
            let col_hooked = hooked.generate_column_gen(cx, cz);
            let col_engine = engine.generate_column_gen(cx, cz);
            let sp = SectionPos::new(cx, cy, cz);
            let a = hooked.generate_section(sp, &col_hooked);
            let b = engine.generate_section(sp, &col_engine);
            assert_eq!(
                a.blocks_slice(),
                b.blocks_slice(),
                "engine fallback must be byte-identical at ({cx},{cy},{cz})"
            );
        }
    }
}
