//! The wasmtime side of the host: engine configuration, module cache, the
//! `host_dispatch` import, and the per-mod store state (RNG streams, the
//! registration window, diagnostics counters).
//!
//! Engine config is part of the determinism contract: NaN
//! canonicalization ON, no threads, no WASI, no relaxed-SIMD, epoch
//! interruption armed by a background ticker thread so a runaway mod traps out
//! instead of hanging the tick loop.
//!
//! Call handling is split per capability domain (one submodule per family);
//! the exhaustive switchboard in [`handle_host_call`] routes every ABI
//! variant to its home, so a new variant cannot compile without picking
//! one. The client-instance surface lives in [`super::client`].

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

use mod_api::{HostCall, HostRet, RuntimeSide};
use wasmtime::{
    AsContextMut, Caller, Config, Engine, Linker, Memory, StoreLimits, StoreLimitsBuilder,
    TypedFunc,
};

use super::client::ClientStoreData;

pub(in crate::modding) mod guards;
pub(in crate::modding) mod module_cache;

pub(in crate::modding) use module_cache::module_for;

mod blocks;
mod containers;
mod core;
mod entities;
mod gui;
mod kv;
mod player;
mod registry;
mod sounds;
pub(in crate::modding) mod tags;
mod worldgen;

/// How often the background ticker advances the engine epoch.
const EPOCH_PERIOD: Duration = Duration::from_millis(50);

/// Epochs of GUEST COMPUTE a single dispatch may span before it traps: a
/// generous ~2 s for work that should take microseconds. Time spent inside
/// re-entrant host calls is NOT charged — `host_dispatch` re-arms the deadline
/// with the remaining budget when a host call returns, so a host-side stall
/// (e.g. a slow storage read) cannot get an innocent mod disabled. Hitting
/// the budget is a mod bug; the mod is disabled for the session and the tick
/// continues.
pub(in crate::modding) const DISPATCH_DEADLINE_EPOCHS: u64 = 40;

/// Host calls one dispatch may make — the backstop that keeps the watchdog
/// meaningful now that host-call time is uncharged: a guest spinning on cheap
/// host calls consumes almost no charged epochs, so the call count is what
/// bounds it. Orders of magnitude above legitimate use (the heaviest bundled
/// dispatches make dozens).
pub(in crate::modding) const DISPATCH_HOST_CALL_MAX: u32 = 65_536;

/// Byte cap for the [`short_debug`] call renderings kept for disable-message
/// diagnostics.
pub(in crate::modding) const DIAG_DEBUG_CAP: usize = 160;

/// Linear-memory cap per mod instance (64 MiB) — a leaky mod fails its own
/// allocations (and traps out) instead of eating the game's address space.
const GUEST_MEMORY_CAP: usize = 64 << 20;

/// Mirror of the engine's epoch counter (wasmtime does not expose a getter),
/// advanced in lockstep by the ticker so the host can measure how many epochs
/// a guest stretch consumed. Diagnostics-grade accuracy: ±1 epoch races with
/// the ticker are fine.
static EPOCH_NOW: AtomicU64 = AtomicU64::new(0);

pub(in crate::modding) fn epoch_now() -> u64 {
    EPOCH_NOW.load(Ordering::Relaxed)
}

/// Advance the engine epoch and its mirror together — the ticker's step, also
/// how tests simulate host-side stalls without waiting wall time.
fn advance_epoch(engine: &Engine, ticks: u64) {
    for _ in 0..ticks {
        EPOCH_NOW.fetch_add(1, Ordering::Relaxed);
        engine.increment_epoch();
    }
}

#[cfg(test)]
pub(in crate::modding) fn test_advance_epochs(ticks: u64) {
    advance_epoch(engine(), ticks);
}

/// Test-only seam: runs at the top of every host call made by the mod whose
/// id matches, so a test can simulate a host call stalling for many epochs.
#[cfg(test)]
pub(in crate::modding) static HOST_CALL_TEST_HOOK: Mutex<Option<(String, fn())>> = Mutex::new(None);

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
                    Some(engine) => advance_epoch(&engine, 1),
                    None => break,
                }
            })
            .expect("spawn mod epoch ticker");
        engine
    });
    &ENGINE
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
    /// Watchdog accounting for the current dispatch (see
    /// [`DISPATCH_DEADLINE_EPOCHS`]): guest-compute epochs still available,
    /// and the [`epoch_now`] reading when the guest was last (re-)entered.
    deadline_budget: u64,
    deadline_armed_at: u64,
    /// Host calls made by the current dispatch ([`DISPATCH_HOST_CALL_MAX`]).
    dispatch_host_calls: u32,
    /// Wall time the current dispatch spent inside host calls — the
    /// guest/host split for slow-dispatch diagnostics (target
    /// `petramond::modding::perf`).
    pub(in crate::modding) dispatch_host_wall: std::time::Duration,
    /// Bounded rendering of the dispatch's most recent host call and whether
    /// it returned — diagnostics for disable messages.
    pub(in crate::modding) last_host_call: Option<(String, bool)>,
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
            deadline_budget: DISPATCH_DEADLINE_EPOCHS,
            deadline_armed_at: epoch_now(),
            dispatch_host_calls: 0,
            dispatch_host_wall: std::time::Duration::ZERO,
            last_host_call: None,
        }
    }

    /// Reset the watchdog accounting for one guest entry; the caller arms the
    /// store's epoch deadline with the same budget.
    pub(in crate::modding) fn begin_dispatch(&mut self) {
        self.deadline_budget = DISPATCH_DEADLINE_EPOCHS;
        self.deadline_armed_at = epoch_now();
        self.dispatch_host_calls = 0;
        self.dispatch_host_wall = std::time::Duration::ZERO;
        self.last_host_call = None;
    }

    pub(in crate::modding) fn dispatch_host_calls(&self) -> u32 {
        self.dispatch_host_calls
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

/// `{value:?}` truncated at roughly `cap` bytes — safe on variants carrying
/// large payloads (an image call would otherwise Debug-print byte by byte).
pub(in crate::modding) fn short_debug(value: &dyn std::fmt::Debug, cap: usize) -> String {
    struct Bounded {
        out: String,
        cap: usize,
    }
    impl std::fmt::Write for Bounded {
        fn write_str(&mut self, s: &str) -> std::fmt::Result {
            let room = self.cap.saturating_sub(self.out.len());
            let take = (0..=room.min(s.len()))
                .rev()
                .find(|&i| s.is_char_boundary(i))
                .unwrap_or(0);
            self.out.push_str(&s[..take]);
            // Reporting an error aborts the formatting walk right here, so a
            // multi-megabyte payload never renders past the cap.
            if take < s.len() {
                Err(std::fmt::Error)
            } else {
                Ok(())
            }
        }
    }
    let mut w = Bounded {
        out: String::new(),
        cap,
    };
    if std::fmt::write(&mut w, format_args!("{value:?}")).is_err() {
        w.out.push('…');
    }
    w.out
}

/// THE host-call switchboard: routes every ABI variant to its category
/// handler below (exhaustive, so a new variant must pick a home here). Calls
/// that need the live simulation reach it through [`scope::with_active`];
/// everything else lives on the store.
pub(in crate::modding) fn handle_host_call(data: &mut ModStoreData, call: HostCall) -> HostRet {
    data.stats.host_calls += 1;
    #[cfg(test)]
    if let Some((id, hook)) = HOST_CALL_TEST_HOOK.lock().unwrap().as_ref() {
        if *id == data.mod_id {
            hook();
        }
    }
    if data.side == RuntimeSide::Client && !super::client::client_capability(&call) {
        return HostRet::Error(
            "simulation host calls are unavailable to a client_wasm instance".into(),
        );
    }
    // A client instance's `PlayerState` answers the actor snapshot published
    // by the prediction dispatch — never the sim query path below.
    if data.side == RuntimeSide::Client && matches!(call, HostCall::PlayerState) {
        return match super::client::scope::active_actor() {
            Some(actor) => HostRet::Player(actor),
            None => HostRet::Error(
                "PlayerState on a client instance is available during prediction dispatches only"
                    .into(),
            ),
        };
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
        | HostCall::CollisionShapeAt { .. }
        | HostCall::BiomeAt { .. }
        | HostCall::SurfaceYAt { .. }
        | HostCall::FindBlocks { .. }
        | HostCall::SwapModelBlock { .. } => blocks::handle_block_call(&data.mod_id, call),
        HostCall::SpawnMob { .. }
        | HostCall::MobInfo { .. }
        | HostCall::MobCanReach { .. }
        | HostCall::MobsInRadius { .. }
        | HostCall::DamageMob { .. }
        | HostCall::DespawnMob { .. }
        | HostCall::MobEmitterSet { .. }
        | HostCall::MobAnimSet { .. }
        | HostCall::MobAnimRate { .. }
        | HostCall::MobAnimSeek { .. }
        | HostCall::MobAnimState { .. }
        | HostCall::MobDrive { .. }
        | HostCall::MobMount { .. }
        | HostCall::PlayerPoseSet { .. }
        | HostCall::MobDismount { .. }
        | HostCall::MobRiders { .. }
        | HostCall::BlockModelGroup { .. }
        | HostCall::SpawnItem { .. } => entities::handle_entity_call(&data.mod_id, call),
        HostCall::PlayerState
        | HostCall::DamagePlayer { .. }
        | HostCall::ApplyKnockback { .. }
        | HostCall::GiveItem { .. }
        | HostCall::ConsumeHeld { .. }
        | HostCall::ReplaceHeldOne { .. }
        | HostCall::SetHealth { .. }
        | HostCall::Teleport { .. }
        | HostCall::EffectApply { .. }
        | HostCall::EffectsActive
        | HostCall::PlayerInput { .. }
        | HostCall::Players
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
        | HostCall::SectionKvDelete { .. } => kv::handle_kv_call(&data.mod_id, call),
        HostCall::MobTagGet { .. }
        | HostCall::MobTagSet { .. }
        | HostCall::MobTagDelete { .. }
        | HostCall::MobTagsGet { .. }
        | HostCall::MobsWithTag { .. } => tags::handle_tag_call(&data.mod_id, call),
        HostCall::ResolveBlock { .. }
        | HostCall::ResolveItem { .. }
        | HostCall::ResolveMob { .. }
        | HostCall::BlockNames { .. }
        | HostCall::ItemNames { .. }
        | HostCall::MobNames { .. }
        | HostCall::BlocksByTag { .. }
        | HostCall::ItemsByTag { .. }
        | HostCall::ItemInfo { .. }
        | HostCall::ResolveShape { .. } => registry::handle_registry_call(call),
        HostCall::RegisterWorldgenFeature { .. }
        | HostCall::RegisterStageReplacement { .. }
        | HostCall::RegisterGenerator { .. } => worldgen::handle_worldgen_call(data, call),
        HostCall::GuiStateSet { .. }
        | HostCall::GuiStateGet { .. }
        | HostCall::GuiOpen { .. }
        | HostCall::GuiClose => gui::handle_gui_call(&data.mod_id, call),
        HostCall::ContainerGet { .. }
        | HostCall::ContainerGetMany { .. }
        | HostCall::ContainerSet { .. }
        | HostCall::RecipeResult { .. } => containers::handle_container_call(&data.mod_id, call),
        HostCall::ClientRegisterOverlay { .. }
        | HostCall::ClientRegisterKey { .. }
        | HostCall::ClientSurfaceColumns { .. }
        | HostCall::ClientUiStateSet { .. }
        | HostCall::ClientUiStateGet { .. }
        | HostCall::ClientImageSet { .. }
        | HostCall::ClientImageBlit { .. }
        | HostCall::ClientTextMeasure { .. }
        | HostCall::ClientImageDrawTexts { .. }
        | HostCall::ClientGuiOpen { .. }
        | HostCall::ClientGuiClose
        | HostCall::ClientCanvasOpen { .. }
        | HostCall::ClientCanvasClose
        | HostCall::ClientCanvasSceneSet { .. }
        | HostCall::ClientCanvasViewSet { .. }
        | HostCall::ClientStorageGetMany { .. }
        | HostCall::ClientStorageSetMany { .. }
        | HostCall::ClientStorageReadBegin { .. }
        | HostCall::ClientStorageReadPoll { .. }
        | HostCall::ClientEnvParams { .. }
        | HostCall::ClientBiomeAt { .. }
        | HostCall::ClientAmbientSet { .. }
        | HostCall::ClientLoopSet { .. }
        | HostCall::ClientMoodSet { .. }
        | HostCall::ClientBlocksAt { .. } => super::client::handle_client_call(data, call),
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
                {
                    // Charge the guest stretch since the last (re-)arm; the
                    // host execution below stays uncharged.
                    let now = epoch_now();
                    let data = caller.data_mut();
                    data.dispatch_host_calls += 1;
                    if data.dispatch_host_calls > DISPATCH_HOST_CALL_MAX {
                        return Err(wasmtime::Error::msg(format!(
                            "dispatch exceeded {DISPATCH_HOST_CALL_MAX} host calls"
                        )));
                    }
                    let used = now.saturating_sub(data.deadline_armed_at);
                    data.deadline_budget = data.deadline_budget.saturating_sub(used);
                    if data.deadline_budget == 0 {
                        return Err(wasmtime::Error::msg(
                            "dispatch exhausted its guest compute budget",
                        ));
                    }
                    data.last_host_call = Some((short_debug(&call, DIAG_DEBUG_CAP), false));
                }
                let host_started = std::time::Instant::now();
                let ret = handle_host_call(caller.data_mut(), call);
                caller.data_mut().dispatch_host_wall += host_started.elapsed();
                let bytes = mod_api::encode(&ret)
                    .map_err(|e| wasmtime::Error::msg(format!("encode host reply: {e}")))?;
                let alloc =
                    caller.data().alloc.clone().ok_or_else(|| {
                        wasmtime::Error::msg("host_dispatch during instantiation")
                    })?;
                // Host time is not the mod's fault: re-arm the deadline with
                // the remaining guest budget before re-entering guest code
                // (the reply-staging alloc below is already guest code).
                let budget = {
                    let data = caller.data_mut();
                    if let Some((_, returned)) = &mut data.last_host_call {
                        *returned = true;
                    }
                    data.deadline_armed_at = epoch_now();
                    data.deadline_budget
                };
                caller.as_context_mut().set_epoch_deadline(budget);
                let reply_ptr = alloc.call(&mut caller, bytes.len() as u32)?;
                memory.write(&mut caller, reply_ptr as usize, &bytes)?;
                Ok(mod_api::pack_ptr_len(reply_ptr, bytes.len() as u32))
            },
        )
        .map_err(|e| format!("define host_dispatch: {e:#}"))?;
    Ok(linker)
}
