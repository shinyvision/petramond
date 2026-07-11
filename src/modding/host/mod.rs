//! The wasmtime side of the host: engine configuration, module cache, the
//! `host_dispatch` import, and the per-mod store state (RNG streams, the
//! registration window, diagnostics counters).
//!
//! Engine config is part of the determinism contract (WIKI/modding.md): NaN
//! canonicalization ON, no threads, no WASI, no relaxed-SIMD, epoch
//! interruption armed by a background ticker thread so a runaway mod traps out
//! instead of hanging the tick loop.
//!
//! Call handling is split per capability domain (one submodule per family);
//! the exhaustive switchboard in [`handle_host_call`] routes every ABI
//! variant to its home, so a new variant cannot compile without picking
//! one. The client-instance surface lives in [`super::client`].

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

use mod_api::{HostCall, HostRet, RuntimeSide};
use wasmtime::{
    Caller, Config, Engine, Linker, Memory, Module, StoreLimits, StoreLimitsBuilder, TypedFunc,
};

use super::client::ClientStoreData;

pub(in crate::modding) mod guards;

mod blocks;
mod containers;
mod core;
mod entities;
mod gui;
mod kv;
mod player;
mod sounds;
mod worldgen;

/// How often the background ticker advances the engine epoch.
const EPOCH_PERIOD: Duration = Duration::from_millis(50);

/// Epochs a single guest dispatch may span before it traps: a generous ~2 s of
/// wall time for work that should take microseconds. Hitting it is a mod bug;
/// the mod is disabled for the session and the tick continues.
pub(in crate::modding) const DISPATCH_DEADLINE_EPOCHS: u64 = 40;

/// Linear-memory cap per mod instance (64 MiB) — a leaky mod fails its own
/// allocations (and traps out) instead of eating the game's address space.
const GUEST_MEMORY_CAP: usize = 64 << 20;

/// The process-wide wasmtime engine, plus its epoch ticker thread. The ticker
/// only bumps a counter — it never touches the simulation — so determinism is
/// unaffected; it exists purely so the deadline can fire while the main thread
/// is stuck inside a guest.
pub(in crate::modding) fn engine() -> &'static Engine {
    static ENGINE: LazyLock<Engine> = LazyLock::new(|| {
        let mut config = Config::new();
        config.cranelift_nan_canonicalization(true);
        config.wasm_relaxed_simd(false);
        config.epoch_interruption(true);
        let engine = Engine::new(&config).expect("wasmtime engine config");
        let weak = engine.weak();
        std::thread::Builder::new()
            .name("mod-epoch".into())
            .spawn(move || loop {
                std::thread::sleep(EPOCH_PERIOD);
                match weak.upgrade() {
                    Some(engine) => engine.increment_epoch(),
                    None => break,
                }
            })
            .expect("spawn mod epoch ticker");
        engine
    });
    &ENGINE
}

/// Compile (or fetch the cached compilation of) the module at `path`. Cached
/// per path for the process lifetime: tests and repeated `Game::new` calls pay
/// the cranelift compile once.
pub(in crate::modding) fn module_for(path: &Path) -> Result<Module, String> {
    static CACHE: LazyLock<Mutex<HashMap<PathBuf, Module>>> =
        LazyLock::new(|| Mutex::new(HashMap::new()));
    let mut cache = CACHE.lock().unwrap();
    if let Some(module) = cache.get(path) {
        return Ok(module.clone());
    }
    let bytes = std::fs::read(path).map_err(|e| format!("read {}: {e}", path.display()))?;
    let module =
        Module::new(engine(), &bytes).map_err(|e| format!("compile {}: {e:#}", path.display()))?;
    cache.insert(path.to_path_buf(), module.clone());
    Ok(module)
}

/// Whether the store is inside its registration window (`mod_init`).
#[derive(Copy, Clone, PartialEq, Eq)]
pub(in crate::modding) enum Phase {
    Init,
    Run,
}

/// A registration collected during `mod_init`, applied to the bus/scheduler by
/// [`super::ModHost::initialize`] after the guest call returns.
pub(in crate::modding) enum Registration {
    TickSystem {
        stage: mod_api::Stage,
        attach: mod_api::AttachSide,
        priority: i32,
        system_id: u32,
    },
    EventHandler {
        event: mod_api::EventKind,
        priority: i32,
        handler_id: u32,
    },
    WorldgenFeature {
        stage: mod_api::WorldgenStage,
        feature_id: u32,
    },
    StageReplacement {
        stage: mod_api::WorldgenStage,
        callback_id: u32,
    },
    Generator {
        callback_id: u32,
    },
    HostileSpawner {
        priority: i32,
        callback_id: u32,
    },
    BlockBehavior {
        /// The namespaced `blocks.json` `behavior` key this mod handles.
        key: String,
        callback_id: u32,
    },
    AiNode {
        /// The namespaced `mobs.json` brain-row `node` key this mod handles.
        key: String,
        callback_id: u32,
    },
}

impl Registration {
    /// Whether this registration targets the worldgen hook config (recorded by
    /// the MAIN load; accepted-and-ignored on per-thread gen instances).
    pub(in crate::modding) fn is_gen(&self) -> bool {
        matches!(
            self,
            Registration::WorldgenFeature { .. }
                | Registration::StageReplacement { .. }
                | Registration::Generator { .. }
        )
    }
}

/// Diagnostics counters (also the observability hooks the contract tests use).
#[derive(Default, Copy, Clone)]
pub(crate) struct HostStats {
    /// `host_dispatch` calls that decoded successfully.
    pub host_calls: u64,
    /// Registrations accepted during the init window.
    pub registered: u64,
    /// Registration attempts rejected outside the window.
    pub rejected_registrations: u64,
}

/// Per-mod store data: everything `host_dispatch` can reach without the
/// scoped [`SimCtx`](crate::events::SimCtx).
pub(in crate::modding) struct ModStoreData {
    pub mod_id: String,
    world_seed: u32,
    pub phase: Phase,
    pub pending: Vec<Registration>,
    /// Named deterministic RNG streams: state per key, seeded from
    /// (world seed, mod id, key) on first use.
    rng: HashMap<String, u64>,
    /// Cached handles into the guest, set right after instantiation.
    pub memory: Option<Memory>,
    pub alloc: Option<TypedFunc<u32, u32>>,
    pub limits: StoreLimits,
    pub stats: HostStats,
    pub side: RuntimeSide,
    pub client: Option<ClientStoreData>,
}

impl ModStoreData {
    #[cfg_attr(not(test), allow(dead_code))]
    pub(in crate::modding) fn new(mod_id: &str, world_seed: u32) -> Self {
        Self::new_for_side(mod_id, world_seed, RuntimeSide::Server, None)
    }

    pub(in crate::modding) fn new_for_side(
        mod_id: &str,
        world_seed: u32,
        side: RuntimeSide,
        client_storage_dir: Option<PathBuf>,
    ) -> Self {
        Self {
            mod_id: mod_id.to_owned(),
            world_seed,
            phase: Phase::Init,
            pending: Vec::new(),
            rng: HashMap::new(),
            memory: None,
            alloc: None,
            limits: StoreLimitsBuilder::new()
                .memory_size(GUEST_MEMORY_CAP)
                .build(),
            stats: HostStats::default(),
            side,
            client: client_storage_dir.map(ClientStoreData::new),
        }
    }

    pub(super) fn register(&mut self, reg: Registration) -> HostRet {
        if self.phase != Phase::Init {
            self.stats.rejected_registrations += 1;
            return HostRet::Error(
                "mod registrations may only be registered during mod_init".into(),
            );
        }
        self.stats.registered += 1;
        self.pending.push(reg);
        HostRet::Unit
    }

    pub(super) fn rng_next(&mut self, stream_key: &str) -> u64 {
        let state = match self.rng.get_mut(stream_key) {
            Some(state) => state,
            None => {
                let seed = stream_seed(self.world_seed, &self.mod_id, stream_key);
                self.rng.entry(stream_key.to_owned()).or_insert(seed)
            }
        };
        splitmix_next(state)
    }
}

/// Intern a mod id so engine-side [`crate::events::DamageSource::Mod`] can
/// carry it as a `Copy` `&'static str`. Bounded: one leaked entry per distinct
/// pack id per process, deduplicated across sessions/tests.
pub(super) fn intern_mod_id(id: &str) -> &'static str {
    static IDS: LazyLock<Mutex<HashSet<&'static str>>> =
        LazyLock::new(|| Mutex::new(HashSet::new()));
    let mut ids = IDS.lock().unwrap();
    match ids.get(id) {
        Some(s) => s,
        None => {
            let s: &'static str = Box::leak(id.to_owned().into_boxed_str());
            ids.insert(s);
            s
        }
    }
}

/// Seed for a mod's named RNG stream: FNV-1a over `mod_id NUL key`, mixed with
/// the world seed. Deterministic per (world, mod, key) and decorrelated between
/// streams.
fn stream_seed(world_seed: u32, mod_id: &str, key: &str) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for b in mod_id.bytes().chain([0u8]).chain(key.bytes()) {
        h = (h ^ b as u64).wrapping_mul(0x1_0000_0000_01b3);
    }
    h ^ (world_seed as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)
}

/// One SplitMix64 step (the same finalizer as [`crate::entity::hash01`]).
fn splitmix_next(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// THE host-call switchboard: routes every ABI variant to its category
/// handler below (exhaustive, so a new variant must pick a home here). Calls
/// that need the live simulation reach it through [`scope::with_active`];
/// everything else lives on the store.
pub(in crate::modding) fn handle_host_call(data: &mut ModStoreData, call: HostCall) -> HostRet {
    data.stats.host_calls += 1;
    if data.side == RuntimeSide::Client && !super::client::client_capability(&call) {
        return HostRet::Error(
            "simulation host calls are unavailable to a client_wasm instance".into(),
        );
    }
    match call {
        HostCall::Log { .. }
        | HostCall::RuntimeSide
        | HostCall::CurrentTick
        | HostCall::RngU64 { .. }
        | HostCall::RegisterTickSystem { .. }
        | HostCall::RegisterEventHandler { .. }
        | HostCall::RegisterHostileSpawner { .. }
        | HostCall::RegisterBlockBehavior { .. }
        | HostCall::RegisterAiNode { .. }
        | HostCall::ShaderSetParam { .. } => core::handle_core_call(data, call),
        HostCall::GetBlock { .. }
        | HostCall::GetBlocks { .. }
        | HostCall::SetBlock { .. }
        | HostCall::SetBlocks { .. }
        | HostCall::ScheduleTick { .. }
        | HostCall::IsLoaded { .. }
        | HostCall::LightAt { .. }
        | HostCall::BlockIsFullSpawnSupport { .. }
        | HostCall::SwapModelBlock { .. } => blocks::handle_block_call(&data.mod_id, call),
        HostCall::SpawnMob { .. }
        | HostCall::MobsInRadius { .. }
        | HostCall::DamageMob { .. }
        | HostCall::DespawnMob { .. }
        | HostCall::MobEmitterSet { .. }
        | HostCall::SpawnItem { .. } => entities::handle_entity_call(&data.mod_id, call),
        HostCall::PlayerState
        | HostCall::DamagePlayer { .. }
        | HostCall::ApplyKnockback { .. }
        | HostCall::GiveItem { .. }
        | HostCall::KillPlayer
        | HostCall::SetHealth { .. }
        | HostCall::Teleport { .. }
        | HostCall::EffectApply { .. }
        | HostCall::EffectRemove { .. }
        | HostCall::EffectsActive
        | HostCall::ChatSend { .. } => player::handle_player_call(&data.mod_id, call),
        HostCall::EmitSound { .. }
        | HostCall::SoundPlayAt { .. }
        | HostCall::SoundPlayOnMob { .. }
        | HostCall::SoundStop { .. }
        | HostCall::EmitterBurst { .. } => sounds::handle_sound_call(&data.mod_id, call),
        HostCall::WorldKvGet { .. }
        | HostCall::WorldKvSet { .. }
        | HostCall::WorldKvDelete { .. }
        | HostCall::SectionKvGet { .. }
        | HostCall::SectionKvSet { .. }
        | HostCall::SectionKvDelete { .. }
        | HostCall::MobKvGet { .. }
        | HostCall::MobKvSet { .. }
        | HostCall::MobKvDelete { .. } => kv::handle_kv_call(&data.mod_id, call),
        HostCall::ResolveBlock { .. }
        | HostCall::RegisterWorldgenFeature { .. }
        | HostCall::RegisterStageReplacement { .. }
        | HostCall::RegisterGenerator { .. } => worldgen::handle_worldgen_call(data, call),
        HostCall::GuiStateSet { .. }
        | HostCall::GuiStateGet { .. }
        | HostCall::GuiOpen { .. }
        | HostCall::GuiClose => gui::handle_gui_call(&data.mod_id, call),
        HostCall::ContainerGet { .. }
        | HostCall::ContainerGetMany { .. }
        | HostCall::ContainerSet { .. }
        | HostCall::ItemInfo { .. }
        | HostCall::RecipeResult { .. } => containers::handle_container_call(&data.mod_id, call),
        HostCall::ClientRegisterOverlay { .. }
        | HostCall::ClientRegisterKey { .. }
        | HostCall::ClientSurface { .. }
        | HostCall::ClientUiStateSet { .. }
        | HostCall::ClientUiStateGet { .. }
        | HostCall::ClientImageSet { .. }
        | HostCall::ClientTextMeasure { .. }
        | HostCall::ClientImageDrawTexts { .. }
        | HostCall::ClientGuiOpen { .. }
        | HostCall::ClientGuiClose
        | HostCall::ClientCanvasOpen { .. }
        | HostCall::ClientCanvasClose
        | HostCall::ClientCanvasSceneSet { .. }
        | HostCall::ClientCanvasViewSet { .. }
        | HostCall::ClientStorageGetMany { .. }
        | HostCall::ClientStorageSetMany { .. } => super::client::handle_client_call(data, call),
    }
}

/// Build the linker exposing the single guest import,
/// `env::host_dispatch(ptr, len) -> u64` (packed reply `ptr << 32 | len`).
/// Reply buffers are allocated IN the guest via its exported `mod_alloc` —
/// wasmtime host functions may re-enter the calling instance — and freed by
/// the guest once decoded.
pub(in crate::modding) fn linker() -> Result<Linker<ModStoreData>, String> {
    let mut linker = Linker::new(engine());
    linker
        .func_wrap(
            "env",
            "host_dispatch",
            |mut caller: Caller<'_, ModStoreData>, ptr: u32, len: u32| -> wasmtime::Result<u64> {
                let memory = caller
                    .data()
                    .memory
                    .ok_or_else(|| wasmtime::Error::msg("host_dispatch during instantiation"))?;
                // Bounds-check BEFORE sizing the copy so a hostile length
                // can't balloon a host allocation; a lying guest just traps.
                if len as usize > memory.data_size(&caller) {
                    return Err(wasmtime::Error::msg("host call exceeds guest memory"));
                }
                let mut buf = vec![0u8; len as usize];
                memory.read(&caller, ptr as usize, &mut buf)?;
                // A call the host cannot decode is a broken ABI, not a bad
                // argument: trap (=> the mod is disabled), don't guess.
                let call: HostCall = mod_api::decode(&buf)
                    .map_err(|e| wasmtime::Error::msg(format!("malformed host call: {e}")))?;
                let ret = handle_host_call(caller.data_mut(), call);
                let bytes = mod_api::encode(&ret)
                    .map_err(|e| wasmtime::Error::msg(format!("encode host reply: {e}")))?;
                let alloc =
                    caller.data().alloc.clone().ok_or_else(|| {
                        wasmtime::Error::msg("host_dispatch during instantiation")
                    })?;
                let reply_ptr = alloc.call(&mut caller, bytes.len() as u32)?;
                memory.write(&mut caller, reply_ptr as usize, &bytes)?;
                Ok(mod_api::pack_ptr_len(reply_ptr, bytes.len() as u32))
            },
        )
        .map_err(|e| format!("define host_dispatch: {e:#}"))?;
    Ok(linker)
}
