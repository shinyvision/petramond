//! Worldgen hooks: the registered feature/stage-replacement
//! config, its process-wide installation, and the per-thread guest dispatch.
//!
//! # Shape
//!
//! `mod_init` on the MAIN load records worldgen registrations; `ModHost::
//! initialize` folds them into one immutable [`GenHooks`] and [`install`]s it
//! process-wide. `ChunkGenerator::new` captures the installed config (an `Arc`
//! clone), so every generator built for a session — worker threads, the main
//! thread, tooling — agrees on the hook set; the installed epoch rides the
//! per-thread generator cache keys (`ChunkGenerator::installed_config`) so a
//! new session's config replaces stale cached generators.
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
//! Trap / emergency deadline / protocol break / invalid ids disable
//! the MOD — not just the failing thread's instance: every gen instance holds
//! the mod's session-wide [`ModHealth`] (shared with its tick instance), so
//! the first failure on any worker stops the mod on every worker before their
//! next dispatch, and generation cannot keep including a mod on some threads
//! while others dropped it. A failed FEATURE is skipped, a failed stage
//! REPLACEMENT falls back to the ENGINE stage (logged loudly, once per
//! stage). Fuel is metered for one-time cost warnings; crossing a warning
//! threshold does not interrupt or disable a mod.
//!
//! # Empty-hook cost
//!
//! With no hooks installed the whole system is one `Option` on the generator
//! (checked per stage) plus one atomic epoch load per cached-generator lookup
//! — no snapshots, no allocation, byte-identical output (the genparity pin).

use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use mod_api::{GuestCall, GuestRet, WorldgenStage};
use wasmtime::Module;

use petramond_world::biome;
use petramond_world::block::Block;
use petramond_world::chunk::{SEA_LEVEL, SECTION_VOLUME};

use super::health::{ModHealth, ModHealthBoard};
use super::host::Registration;
use super::instance::ModInstance;

mod ahead;
mod claims;
mod quiet;

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

struct GenModule {
    id: String,
    module: Module,
    health: Arc<ModHealth>,
    expected_gen_regs: usize,
}

struct FeatureHook {
    mod_idx: usize,
    feature_id: u32,
    stage_idx: usize,
    filter: mod_api::GenFeatureFilter,
    quiet: quiet::Quiet,
    claims: claims::ClaimTiles,
    ahead: ahead::Ahead,
}

struct StageHook {
    mod_idx: usize,
    callback_id: u32,
}

pub struct GenHooks {
    epoch: u64,
    seed: u32,
    mods: Vec<GenModule>,
    features: Vec<FeatureHook>,
    replacements: [Option<StageHook>; STAGE_COUNT],
    fallback_logged: [AtomicBool; STAGE_COUNT],
}

pub use petramond_worldgen::hooks::GenInputs;
use petramond_worldgen::hooks::{Claims, FeatureOutcome, GenHookDispatch, GenerationPlan};

impl GenHooks {
    pub fn replaces(&self, stage: WorldgenStage) -> bool {
        self.replacements[stage_index(stage)].is_some()
    }

    pub fn any_features_after(&self, stage: WorldgenStage) -> bool {
        let i = stage_index(stage);
        self.features.iter().any(|f| f.stage_idx == i)
    }

    pub fn features_after(&self, stage: WorldgenStage) -> Vec<usize> {
        let i = stage_index(stage);
        (0..self.features.len())
            .filter(|&f| self.features[f].stage_idx == i)
            .collect()
    }

    pub fn dispatch_feature(&self, idx: usize, inputs: &GenInputs) -> FeatureOutcome {
        let hook = &self.features[idx];
        if !hook
            .filter
            .intersects(inputs.section_pos[1], inputs.surface_heights)
        {
            return FeatureOutcome::Skipped;
        }
        if hook.quiet.contains(inputs.section_pos) {
            return FeatureOutcome::Skipped;
        }
        if let Some(plan) = hook.ahead.take(inputs.section_pos) {
            return FeatureOutcome::Plan(plan);
        }
        let call = GuestCall::GenFeature {
            feature_id: hook.feature_id,
            section_pos: inputs.section_pos,
            seed: inputs.seed,
            blocks: inputs
                .blocks
                .filter(|_| hook.filter.needs_blocks)
                .map_or_else(Vec::new, |c| c.iter().collect()),
            surface_heights: if hook.filter.needs_columns {
                inputs.surface_heights.to_vec()
            } else {
                Vec::new()
            },
            biomes: if hook.filter.needs_columns {
                inputs.biomes.to_vec()
            } else {
                Vec::new()
            },
            sea_level: SEA_LEVEL,
        };
        self.dispatch(hook.mod_idx, &call, |ret| match ret {
            GuestRet::GenOutput(mut w) => {
                self.hold_ahead(hook, std::mem::take(&mut w.ahead))?;
                let nothing_in = std::mem::take(&mut w.nothing_in);
                let Some(plan) = outcome(w, self.seed, self.epoch)? else {
                    return Ok(FeatureOutcome::Deferred);
                };
                validate_nothing_in(&nothing_in, inputs.section_pos, &plan)?;
                hook.quiet.declare(inputs.section_pos, &nothing_in);
                Ok(FeatureOutcome::Plan(plan))
            }
            other => Err(reply_shape("GenFeature", "GenOutput", &other)),
        })
        .unwrap_or(FeatureOutcome::Skipped)
    }

    /// Validates the section outputs a reply handed over and holds them for their sections.
    fn hold_ahead(
        &self,
        hook: &FeatureHook,
        ahead: Vec<mod_api::SectionOutput>,
    ) -> Result<(), String> {
        if ahead.len() > ahead::MAX_PER_REPLY {
            return Err("generation output hands over too many sections".into());
        }
        let mut held = Vec::with_capacity(ahead.len());
        for mod_api::SectionOutput {
            section,
            mut output,
        } in ahead
        {
            if output.deferred || !output.ahead.is_empty() {
                return Err(format!(
                    "the output handed over for section {section:?} is deferred or nested"
                ));
            }
            let nothing_in = std::mem::take(&mut output.nothing_in);
            let plan = validated_writes(output, self.seed, self.epoch)?;
            validate_nothing_in(&nothing_in, section, &plan)?;
            hook.quiet.declare(section, &nothing_in);
            held.push((section, plan));
        }
        hook.ahead.hold(held);
        Ok(())
    }

    /// Every claim touching `min..=max` from the features that keep columns for themselves;
    /// `None` when one is still being worked out on another worker.
    pub fn claims(&self, min: [i32; 2], max: [i32; 2]) -> Option<Claims> {
        let asked = mod_api::ColumnBox { min, max };
        let tile = |v: i32| v.div_euclid(claims::TILE);
        let mut found: Vec<Arc<mod_api::ColumnMask>> = Vec::new();
        for hook in self.features.iter().filter(|h| h.filter.claims) {
            for tz in tile(min[1])..=tile(max[1]) {
                for tx in tile(min[0])..=tile(max[0]) {
                    let tile_claims = match hook.claims.get([tx, tz]) {
                        Some(tile_claims) => tile_claims,
                        None => {
                            let rect = claims::tile_box([tx, tz]);
                            let call = GuestCall::GenClaims {
                                feature_id: hook.feature_id,
                                seed: self.seed,
                                sea_level: SEA_LEVEL,
                                min: rect.min,
                                max: rect.max,
                            };
                            let reply = self.dispatch(hook.mod_idx, &call, |ret| match ret {
                                GuestRet::GenClaims(mut reply) => {
                                    self.hold_ahead(hook, std::mem::take(&mut reply.ahead))?;
                                    claims::validated(reply)
                                }
                                other => Err(reply_shape("GenClaims", "GenClaims", &other)),
                            });
                            let tile_claims = match reply {
                                Some(Some(tile_claims)) => tile_claims,
                                Some(None) => return None,
                                None => Arc::from([]),
                            };
                            hook.claims.put([tx, tz], Arc::clone(&tile_claims));
                            tile_claims
                        }
                    };
                    for mask in tile_claims.iter() {
                        if mask.bounds.overlaps(&asked) && !found.iter().any(|m| **m == **mask) {
                            found.push(Arc::clone(mask));
                        }
                    }
                }
            }
        }
        Some(Claims::new(found))
    }

    pub fn replace_stage(&self, stage: WorldgenStage, inputs: &GenInputs) -> FeatureOutcome {
        let Some(hook) = self.replacements[stage_index(stage)].as_ref() else {
            return FeatureOutcome::Skipped;
        };
        let call = self.stage_call(hook, stage, inputs);
        let res = self.dispatch(hook.mod_idx, &call, |ret| match ret {
            GuestRet::GenOutput(w) => Ok(match outcome(w, self.seed, self.epoch)? {
                Some(plan) => FeatureOutcome::Plan(plan),
                None => FeatureOutcome::Deferred,
            }),
            other => Err(reply_shape("GenStage", "GenOutput", &other)),
        });
        match res {
            Some(outcome) => outcome,
            None => {
                self.log_fallback(stage, hook.mod_idx);
                FeatureOutcome::Skipped
            }
        }
    }

    pub fn replace_terrain(&self, inputs: &GenInputs) -> Option<Vec<u16>> {
        let stage = WorldgenStage::Terrain;
        let hook = self.replacements[stage_index(stage)].as_ref()?;
        let call = self.stage_call(hook, stage, inputs);
        let res = self.dispatch(hook.mod_idx, &call, |ret| match ret {
            GuestRet::GenBlocks(fill) => {
                if fill.len() != SECTION_VOLUME {
                    return Err(format!(
                        "terrain replacement returned {} cells; a section fill is exactly {}",
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

    pub fn replace_climate(&self, inputs: &GenInputs) -> Option<Vec<u8>> {
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
                if let Some(&bad) = map
                    .iter()
                    .find(|&&id| id == 0 || usize::from(id) > biome::count())
                {
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
            blocks: inputs.blocks.map_or_else(Vec::new, |c| c.iter().collect()),
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

    fn dispatch<T>(
        &self,
        mod_idx: usize,
        call: &GuestCall,
        validate: impl FnOnce(GuestRet) -> Result<T, String>,
    ) -> Option<T> {
        if self.mods[mod_idx].health.is_disabled() {
            return None;
        }
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
                t.slots[mod_idx] = Slot::Live(Box::new(self.instantiate(mod_idx)?));
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

    fn instantiate(&self, mod_idx: usize) -> Option<ModInstance> {
        let m = &self.mods[mod_idx];
        let mut inst = match ModInstance::from_module_side(
            &m.id,
            &m.module,
            self.seed,
            mod_api::RuntimeSide::Worldgen,
            None,
            Arc::clone(&m.health),
        ) {
            Ok(inst) => inst,
            Err(e) => {
                m.health
                    .disable(&format!("worldgen instance failed to instantiate: {e}"));
                return None;
            }
        };
        inst.call_init_detached();
        if inst.disabled() {
            return None;
        }
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
        GuestRet::GenOutput(_) => "GenOutput",
        GuestRet::GenBlocks(_) => "GenBlocks",
        GuestRet::GenBiomes(_) => "GenBiomes",
        GuestRet::HostileSpawn(_) => "HostileSpawn",
        GuestRet::AiDecision(_) => "AiDecision",
        GuestRet::AiDecisions(_) => "AiDecisions",
        GuestRet::BakedSim(_) => "BakedSim",
        GuestRet::BakedRender(_) => "BakedRender",
        GuestRet::BakedItem(_) => "BakedItem",
        GuestRet::ShapePlacement(_) => "ShapePlacement",
        GuestRet::Unsupported => "Unsupported",
        GuestRet::GenClaims(_) => "GenClaims",
    };
    format!("{call} expected a {expected} reply, got {got}")
}

/// The output's validated writes; `None` when it deferred its section.
fn outcome(
    output: mod_api::GenOutput,
    seed: u32,
    epoch: u64,
) -> Result<Option<GenerationPlan>, String> {
    if !output.ahead.is_empty() {
        return Err("only a feature may hand over outputs for other sections".into());
    }
    if output.deferred {
        if !super::has_pending_key() {
            return Err("deferred its section without a pending memo claim to wait on".into());
        }
        return Ok(None);
    }
    validated_writes(output, seed, epoch).map(Some)
}

fn validate_nothing_in(
    nothing_in: &[mod_api::SectionBox],
    section: [i32; 3],
    plan: &GenerationPlan,
) -> Result<(), String> {
    if nothing_in.len() > MAX_GEN_BOXES {
        return Err("generation output declares too many section boxes".into());
    }
    if let Some(bad) = nothing_in
        .iter()
        .find(|b| (0..3).any(|a| b.min[a] > b.max[a]))
    {
        return Err(format!(
            "worldgen section box {:?}..={:?} is inverted",
            bad.min, bad.max
        ));
    }
    if !plan.is_empty() && nothing_in.iter().any(|b| b.contains(section)) {
        return Err(format!(
            "declared it writes nothing in section {section:?} while writing there"
        ));
    }
    Ok(())
}

fn validated_writes(
    output: mod_api::GenOutput,
    seed: u32,
    epoch: u64,
) -> Result<GenerationPlan, String> {
    let registered = Block::all().len();
    if output.blocks.len() + output.authored.cells.len() > MAX_GEN_WRITES
        || output.fills.len() > MAX_GEN_FILLS
        || output.structures.len() > 256
        || output.features.len() > 32
    {
        return Err("generation output exceeds placement budget".into());
    }
    if let Some(bad) = output.fills.iter().find(|fill| {
        fill.block.0 as usize >= registered || (0..3).any(|a| fill.min[a] > fill.max[a])
    }) {
        return Err(format!(
            "worldgen fill {:?}..={:?} of block id {} is inverted or unregistered",
            bad.min, bad.max, bad.block.0
        ));
    }
    if let Some((_, bad)) = output
        .blocks
        .iter()
        .find(|(_, id)| id.0 as usize >= registered)
    {
        return Err(format!(
            "worldgen write with unregistered block id {}",
            bad.0
        ));
    }
    let structures = output
        .structures
        .into_iter()
        .map(|placement| {
            let template = petramond_world::structure::by_key(&placement.template)
                .ok_or_else(|| format!("unknown structure '{}'", placement.template))?;
            let turn = petramond_world::world::placement::authored::Turn::new(placement.turn)?;
            template.place(placement.origin.into(), turn)
        })
        .collect::<Result<_, String>>()?;
    let features = output
        .features
        .into_iter()
        .map(|placement| {
            petramond_worldgen::growth::PlacedFeature::resolve_first(
                &placement.feature,
                &placement.origins,
                seed,
                placement.salt,
            )
        })
        .collect::<Result<_, String>>()?;
    Ok(GenerationPlan {
        fills: output.fills.iter().collect(),
        features,
        authored: authored_cells(output.authored, registered, epoch)?,
        blocks: output.blocks.into_iter().map(|(p, id)| (p, id.0)).collect(),
        structures,
    })
}

const MAX_GEN_WRITES: usize = 262_144;
const LAYOUT_CACHE_MAX: usize = 4096;

type LayoutCache =
    rustc_hash::FxHashMap<Box<[u8]>, Arc<[petramond_world::world::placement::CellWrite]>>;

thread_local! {
    /// Each authored palette entry's layout, by the entry's bytes, for the session `epoch`.
    static LAYOUTS: RefCell<(u64, LayoutCache)> = RefCell::new((0, LayoutCache::default()));
}
const MAX_GEN_FILLS: usize = 4096;
const MAX_GEN_BOXES: usize = 4096;

/// Expands the authored palette through each block's own layout, exactly as a template's
/// palette is, then attaches cell data to the cells those writes placed.
fn authored_cells(
    authored: mod_api::AuthoredWrites,
    registered: usize,
    epoch: u64,
) -> Result<Vec<petramond_world::structure::Cell>, String> {
    use petramond_world::world::placement::authored::{self, Expansion, Turn};
    if authored.palette.len() > 4096 || authored.data.len() > MAX_GEN_WRITES {
        return Err("authored generation output is malformed or exceeds its budget".into());
    }
    let layouts = LAYOUTS.with(|cache| {
        let mut cache = cache.borrow_mut();
        let (cached_epoch, by_entry) = &mut *cache;
        if *cached_epoch != epoch {
            *cached_epoch = epoch;
            by_entry.clear();
        }
        authored
            .palette
            .iter()
            .enumerate()
            .map(|(i, entry)| {
                let entry = entry.map_err(|()| "authored palette is malformed".to_string())?;
                if entry.block.0 as usize >= registered {
                    return Err(format!(
                        "authored material {i} names unregistered block id {}",
                        entry.block.0
                    ));
                }
                if let Some(layout) = by_entry.get(entry.bytes) {
                    return Ok(Arc::clone(layout));
                }
                let state = entry
                    .state()
                    .map(|(k, v)| (k.to_owned(), v.to_owned()))
                    .collect();
                let layout: Arc<[_]> =
                    authored::layout(Block(entry.block.0), &state, Turn::default())
                        .map_err(|e| format!("authored material {i}: {e}"))?
                        .into();
                if by_entry.len() >= LAYOUT_CACHE_MAX {
                    by_entry.clear();
                }
                by_entry.insert(entry.bytes.into(), Arc::clone(&layout));
                Ok(layout)
            })
            .collect::<Result<Vec<_>, String>>()
    })?;
    let mut expansion = Expansion::with_capacity(authored.cells.len());
    for (pos, material) in authored.cells.iter() {
        let layout = layouts
            .get(material as usize)
            .ok_or_else(|| format!("authored cell names missing material {material}"))?;
        if expansion.writes() + layout.len() > MAX_GEN_WRITES {
            return Err("authored footprints exceed the placement budget".into());
        }
        expansion.place(pos.into(), layout);
    }
    let mut cells = expansion.finish()?;
    for datum in authored.data {
        if !petramond_world::registry::is_namespaced(&datum.key)
            || datum.key.len() > mod_api::KV_MAX_KEY_BYTES
            || datum.value.len() > mod_api::KV_MAX_VALUE_BYTES
        {
            return Err(format!("invalid authored cell data '{}'", datum.key));
        }
        let cell = cells
            .get_mut(datum.pos)
            .ok_or_else(|| format!("cell data '{}' needs an authored cell", datum.key))?;
        if cell.data.len() >= mod_api::CELL_KV_MAX_KEYS
            || cell.data.insert(datum.key.clone(), datum.value).is_some()
        {
            return Err(format!("duplicate or excess cell data '{}'", datum.key));
        }
    }
    Ok(cells.into_cells())
}

enum Slot {
    Empty,
    Live(Box<ModInstance>),
}

struct ThreadSlots {
    epoch: u64,
    slots: Vec<Slot>,
}

thread_local! {
    static THREAD_SLOTS: RefCell<ThreadSlots> = const { RefCell::new(ThreadSlots {
        epoch: 0,
        slots: Vec::new(),
    }) };
}

pub struct GenHooksBuilder {
    seed: u32,
    health: ModHealthBoard,
    mods: Vec<GenModule>,
    features: Vec<FeatureHook>,
    replacements: [Option<StageHook>; STAGE_COUNT],
}

impl GenHooksBuilder {
    pub(super) fn new(seed: u32, health: ModHealthBoard) -> Self {
        Self {
            seed,
            health,
            mods: Vec::new(),
            features: Vec::new(),
            replacements: Default::default(),
        }
    }

    pub(super) fn add_registration(&mut self, mod_id: &str, module: &Module, reg: &Registration) {
        match *reg {
            Registration::WorldgenFeature {
                stage,
                feature_id,
                filter,
            } => self.add_feature(mod_id, module, stage, feature_id, filter),
            Registration::StageReplacement { stage, callback_id } => {
                self.add_stage_replacement(mod_id, module, stage, callback_id)
            }
            Registration::Generator { callback_id } => {
                self.add_generator(mod_id, module, callback_id)
            }
            Registration::TickSystem { .. }
            | Registration::EventHandler { .. }
            | Registration::HostileSpawner { .. }
            | Registration::BlockBehavior { .. }
            | Registration::AiNode { .. } => {}
        }
    }

    pub fn add_feature(
        &mut self,
        mod_id: &str,
        module: &Module,
        stage: WorldgenStage,
        feature_id: u32,
        filter: mod_api::GenFeatureFilter,
    ) {
        let mod_idx = self.mod_index(mod_id, module);
        self.features.push(FeatureHook {
            mod_idx,
            feature_id,
            stage_idx: stage_index(stage),
            filter,
            quiet: Default::default(),
            claims: Default::default(),
            ahead: Default::default(),
        });
    }

    pub fn add_stage_replacement(
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

    pub fn add_generator(&mut self, mod_id: &str, module: &Module, callback_id: u32) {
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
                    health: self.health.health(mod_id),
                    expected_gen_regs: 0,
                });
                self.mods.len() - 1
            }
        };
        self.mods[idx].expected_gen_regs += 1;
        idx
    }

    pub fn build(self) -> Option<Arc<GenHooks>> {
        if self.features.is_empty() && self.replacements.iter().all(Option::is_none) {
            return None;
        }
        Some(Arc::new(GenHooks {
            epoch: petramond_worldgen::hooks::next_epoch(),
            seed: self.seed,
            mods: self.mods,
            features: self.features,
            replacements: self.replacements,
            fallback_logged: std::array::from_fn(|_| AtomicBool::new(false)),
        }))
    }
}

impl GenHookDispatch for GenHooks {
    fn epoch(&self) -> u64 {
        self.epoch
    }
    fn replaces(&self, stage: WorldgenStage) -> bool {
        GenHooks::replaces(self, stage)
    }
    fn replace_climate(&self, inputs: &GenInputs) -> Option<Vec<u8>> {
        GenHooks::replace_climate(self, inputs)
    }
    fn replace_terrain(&self, inputs: &GenInputs) -> Option<Vec<u16>> {
        GenHooks::replace_terrain(self, inputs)
    }
    fn replace_stage(&self, stage: WorldgenStage, inputs: &GenInputs) -> FeatureOutcome {
        GenHooks::replace_stage(self, stage, inputs)
    }
    fn any_features_after(&self, stage: WorldgenStage) -> bool {
        GenHooks::any_features_after(self, stage)
    }
    fn features_after(&self, stage: WorldgenStage) -> Vec<usize> {
        GenHooks::features_after(self, stage)
    }
    fn dispatch_feature(&self, idx: usize, inputs: &GenInputs) -> FeatureOutcome {
        GenHooks::dispatch_feature(self, idx, inputs)
    }
    fn wait_deferred(&self) {
        super::wait_for_pending();
    }
    fn claims(&self, min: [i32; 2], max: [i32; 2]) -> Option<Claims> {
        GenHooks::claims(self, min, max)
    }
}

pub fn install(hooks: Option<Arc<GenHooks>>) {
    petramond_worldgen::hooks::install(hooks.map(|h| h as Arc<dyn GenHookDispatch>));
}

#[cfg(test)]
mod tests;
