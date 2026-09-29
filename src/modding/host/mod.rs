use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

use mod_api::{Decoded, ErrorCode, HostCall, HostRet, RuntimeSide, Scope};
use wasmtime::{AsContextMut, Caller, Config, Engine, Linker, Memory, TypedFunc};

use super::client::ClientStoreData;
use super::health::ModHealth;
use super::watchdog::{Context, MemoryGuard, Throttle};

pub(in crate::modding) mod budget;
pub(in crate::modding) mod guards;
pub(in crate::modding) mod module_cache;

pub(in crate::modding) use module_cache::module_for;

mod actors;
pub(in crate::modding) mod blocks;
mod conditions;
mod construction;
mod containers;
mod core;
mod entities;
mod gui;
mod item_motion;
mod kv;
pub(in crate::modding) mod memo;
pub(in crate::modding) mod player;
mod registry;
mod schematics;
mod sounds;
pub(in crate::modding) mod tags;
mod worldgen;

const EPOCH_PERIOD: Duration = Duration::from_millis(50);

pub(in crate::modding) const DISPATCH_HOST_CALL_MAX: u32 = 65_536;

pub(in crate::modding) const DIAG_DEBUG_CAP: usize = 160;

pub(in crate::modding) const WASM32_MEMORY_MAX: u64 = 1 << 32;

static EPOCH_NOW: AtomicU64 = AtomicU64::new(0);

pub(in crate::modding) fn epoch_now() -> u64 {
    EPOCH_NOW.load(Ordering::Relaxed)
}

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

#[cfg(test)]
pub(in crate::modding) static HOST_CALL_TEST_HOOK: Mutex<Option<HostCallHook>> = Mutex::new(None);

#[cfg(test)]
type HostCallHook = (String, fn());

/// The engine configuration every module is compiled and run with. Precompiled artifacts
/// ([`crate::modding::precompile_module`]) must be produced from this same configuration.
pub(in crate::modding) fn engine_config() -> Config {
    let mut config = Config::new();
    config.cranelift_nan_canonicalization(true);
    config.wasm_relaxed_simd(false);
    config.epoch_interruption(true);
    config.consume_fuel(true);
    config
}

/// Compiles for `target` with the features every machine of that target has (no host
/// detection), so an artifact deserializes on any of them: x86-64 gets SSE4.1 (2008+, and what
/// core SIMD needs), everything else its baseline.
pub(in crate::modding) fn retarget(config: &mut Config, target: &str) -> Result<(), String> {
    config
        .target(target)
        .map_err(|e| format!("target {target}: {e:#}"))?;
    if target.starts_with("x86_64") {
        for flag in ["has_sse3", "has_ssse3", "has_sse41"] {
            unsafe { config.cranelift_flag_enable(flag) };
        }
    }
    Ok(())
}

pub(in crate::modding) fn engine() -> &'static Engine {
    static ENGINE: LazyLock<Engine> = LazyLock::new(|| {
        let config = engine_config();
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

#[derive(Copy, Clone, PartialEq, Eq)]
pub(in crate::modding) enum Phase {
    Init,
    Run,
}

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
        filter: mod_api::EventFilter,
    },
    WorldgenFeature {
        stage: mod_api::WorldgenStage,
        feature_id: u32,
        filter: mod_api::GenFeatureFilter,
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
        key: String,
        callback_id: u32,
    },
    AiNode {
        key: String,
        callback_id: u32,
    },
}

impl Registration {
    pub(in crate::modding) fn is_gen(&self) -> bool {
        matches!(
            self,
            Registration::WorldgenFeature { .. }
                | Registration::StageReplacement { .. }
                | Registration::Generator { .. }
        )
    }
}

#[derive(Default, Copy, Clone)]
pub struct HostStats {
    pub host_calls: u64,
    pub registered: u64,
    pub rejected_registrations: u64,
}

pub(in crate::modding) struct ModStoreData {
    pub mod_id: String,
    world_seed: u32,
    pub phase: Phase,
    pub pending: Vec<Registration>,
    rng: HashMap<String, u64>,
    pub memory: Option<Memory>,
    pub alloc: Option<TypedFunc<u32, u32>>,
    pub guest_memory_max: u64,
    pub stats: HostStats,
    pub side: RuntimeSide,
    pub client: Option<ClientStoreData>,
    pub(in crate::modding) health: Arc<ModHealth>,
    pub(in crate::modding) context: Context,
    in_flight_allowance: u64,
    in_flight_used: u64,
    in_flight_armed_at: u64,
    dispatch_host_calls: u32,
    pub(in crate::modding) dispatch_host_wall: std::time::Duration,
    last_host_call: Option<(Vec<u8>, bool)>,
    /// The encoded reply of the host call in progress: one buffer per instance, reused.
    reply_buf: Vec<u8>,
    pub(in crate::modding) throttle: Throttle,
    pub(in crate::modding) client_period: u64,
    pub(in crate::modding) limiter: MemoryGuard,
}

impl ModStoreData {
    #[cfg_attr(not(test), allow(dead_code))]
    pub(in crate::modding) fn new(mod_id: &str, world_seed: u32) -> Self {
        Self::new_for_side(
            mod_id,
            world_seed,
            RuntimeSide::Server,
            None,
            ModHealth::standalone(mod_id),
        )
    }

    pub(in crate::modding) fn new_for_side(
        mod_id: &str,
        world_seed: u32,
        side: RuntimeSide,
        client_buckets: Option<super::client::ClientBuckets>,
        health: Arc<ModHealth>,
    ) -> Self {
        let budgets = health.watchdog().budgets();
        let in_flight_allowance = health.watchdog().in_flight_allowance(Context::Init);
        Self {
            mod_id: mod_id.to_owned(),
            world_seed,
            phase: Phase::Init,
            pending: Vec::new(),
            rng: HashMap::new(),
            memory: None,
            alloc: None,
            guest_memory_max: WASM32_MEMORY_MAX,
            stats: HostStats::default(),
            side,
            client: client_buckets.map(ClientStoreData::new),
            limiter: MemoryGuard::new(Arc::clone(&health)),
            throttle: Throttle::new(&budgets),
            client_period: 0,
            health,
            context: Context::Init,
            in_flight_allowance,
            in_flight_used: 0,
            in_flight_armed_at: epoch_now(),
            dispatch_host_calls: 0,
            dispatch_host_wall: std::time::Duration::ZERO,
            last_host_call: None,
            reply_buf: Vec::new(),
        }
    }

    /// Starts one call in `context`; returns the epochs it may run before the watchdog is asked.
    pub(in crate::modding) fn begin_dispatch(&mut self, context: Context) -> u64 {
        self.context = context;
        self.in_flight_allowance = self.health.watchdog().in_flight_allowance(context);
        self.in_flight_used = 0;
        self.in_flight_armed_at = epoch_now();
        self.dispatch_host_calls = 0;
        self.dispatch_host_wall = std::time::Duration::ZERO;
        self.last_host_call = None;
        self.in_flight_allowance
    }

    /// Charges the guest time since the call was last armed (host calls don't count) and, once
    /// the call has spent its allowance, asks the watchdog for more. Returns the epochs left, or
    /// why the watchdog stopped the call.
    pub(in crate::modding) fn charge_guest_time(&mut self) -> Result<u64, String> {
        let now = epoch_now();
        self.in_flight_used += now.saturating_sub(self.in_flight_armed_at);
        self.in_flight_armed_at = now;
        if self.in_flight_used >= self.in_flight_allowance {
            self.in_flight_allowance = self
                .health
                .watchdog()
                .extend_in_flight(self.context, self.in_flight_used)?;
        }
        Ok(self.in_flight_allowance - self.in_flight_used)
    }

    /// Resumes the guest clock after a host call; returns the epochs the call has left.
    fn rearm(&mut self) -> u64 {
        self.in_flight_armed_at = epoch_now();
        self.in_flight_allowance
            .saturating_sub(self.in_flight_used)
            .max(1)
    }

    pub(in crate::modding) fn last_host_call(&self) -> Option<(String, bool)> {
        let (request, returned) = self.last_host_call.as_ref()?;
        let call: HostCall = mod_api::decode(request).ok()?;
        Some((short_debug(&call, DIAG_DEBUG_CAP), *returned))
    }

    pub(in crate::modding) fn dispatch_host_calls(&self) -> u32 {
        self.dispatch_host_calls
    }

    pub(super) fn world_seed(&self) -> u32 {
        self.world_seed
    }

    pub(in crate::modding) fn set_world_seed(&mut self, seed: u32) {
        self.world_seed = seed;
    }

    pub(super) fn register(&mut self, reg: Registration) -> HostRet {
        self.stats.registered += 1;
        self.pending.push(reg);
        HostRet::Unit
    }

    pub(super) fn refuse_registration(&mut self, code: ErrorCode, why: String) -> HostRet {
        self.stats.rejected_registrations += 1;
        HostRet::error(code, why)
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

fn stream_seed(world_seed: u32, mod_id: &str, key: &str) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for b in mod_id.bytes().chain([0u8]).chain(key.bytes()) {
        h = (h ^ b as u64).wrapping_mul(0x1_0000_0000_01b3);
    }
    h ^ (world_seed as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)
}

fn splitmix_next(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

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

pub(in crate::modding) fn handle_host_call(
    data: &mut ModStoreData,
    call: impl Into<HostCall>,
) -> HostRet {
    let call = call.into();
    data.stats.host_calls += 1;
    #[cfg(test)]
    if let Some((id, hook)) = HOST_CALL_TEST_HOOK.lock().unwrap().as_ref() {
        if *id == data.mod_id {
            hook();
        }
    }
    if let Err(refusal) = admit(data, &call) {
        return refusal;
    }
    let client = data.side == RuntimeSide::Client;
    match call {
        HostCall::Core(call) => core::handle_core_call(data, call),
        HostCall::Block(mod_api::BlockCall::Raycast {
            from,
            dir,
            max,
            filter,
        }) if client => super::client::raycast(from, dir, max, filter),
        HostCall::Block(call) => blocks::handle_block_call(&data.mod_id, call),
        HostCall::Entity(call) => entities::handle_entity_call(&data.mod_id, call),
        HostCall::Player(call) => player::handle_player_call(&data.mod_id, call),
        HostCall::Body(call) if client => super::client::handle_body_call(data, call),
        HostCall::Body(call) => player::handle_body_call(&data.mod_id, call),
        HostCall::Sound(call) => sounds::handle_sound_call(&data.mod_id, call),
        HostCall::Kv(call) => kv::handle_kv_call(&data.mod_id, &data.health, call),
        HostCall::Tag(call) => tags::handle_tag_call(&data.mod_id, call),
        HostCall::Registry(call) => registry::handle_registry_call(call),
        HostCall::Worldgen(call) => worldgen::handle_worldgen_call(data, call),
        HostCall::Memo(call) => memo::handle_memo_call(data, call),
        HostCall::Gui(call) => gui::handle_gui_call(&data.mod_id, call),
        HostCall::Container(call) => containers::handle_container_call(&data.mod_id, call),
        HostCall::Client(call) => super::client::handle_client_call(data, call),
        HostCall::ItemMotion(call) => item_motion::handle(call),
        HostCall::Condition(call) => conditions::handle(call),
        HostCall::Construction(call) => construction::handle_construction_call(call),
        HostCall::Actor(call) => actors::handle_actor_call(&data.mod_id, call),
        HostCall::Schematic(call) => schematics::handle_schematic_call(&data.mod_id, call),
        HostCall::ClientFile(call) => super::client::handle_file_call(data, call),
        HostCall::ClientCapture(call) => super::client::handle_capture_call(data, call),
        HostCall::ClientPresentation(call) => super::client::handle_presentation_call(data, call),
        HostCall::ClientMedia(call) => super::client::handle_media_call(data, call),
    }
}

fn admit(data: &mut ModStoreData, call: &HostCall) -> Result<(), HostRet> {
    let legality = call.legality();
    let shell = data.client.as_ref().is_some_and(|client| client.shell);
    if !legality.sides.admits(data.side, shell) {
        let why = if shell && legality.sides.allows(data.side) {
            format!(
                "{} needs a world, and this client_wasm instance runs on the shell with none \
                 (see ClientContext)",
                call.name()
            )
        } else {
            format!(
                "{} is not available to a {:?} instance",
                call.name(),
                data.side
            )
        };
        return Err(HostRet::error(ErrorCode::WrongSide, why));
    }
    if legality.scope == Scope::Init && data.phase != Phase::Init {
        return Err(data.refuse_registration(
            ErrorCode::NotInInit,
            format!("{} is legal only during mod_init", call.name()),
        ));
    }
    if super::scope::read_only_active() && !guards::read_only_permits(call) {
        return Err(HostRet::error(
            ErrorCode::ReadOnly,
            format!(
                "{} is not allowed during a read-only dispatch (e.g. a shape placement plan)",
                call.name()
            ),
        ));
    }
    Ok(())
}

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
                if len as usize > memory.data_size(&caller) {
                    return Err(wasmtime::Error::msg("host call exceeds guest memory"));
                }
                // The previous call's request buffer is reused for this one.
                let mut buf = caller
                    .data_mut()
                    .last_host_call
                    .take()
                    .map_or_else(Vec::new, |(buf, _)| buf);
                buf.clear();
                buf.resize(len as usize, 0);
                memory.read(&caller, ptr as usize, &mut buf)?;
                let call = match mod_api::decode_host_call(&buf) {
                    Ok(Decoded::Known(call)) => Some(call),
                    Ok(Decoded::Unknown { domain, variant }) => {
                        log::debug!(
                            "mod '{}': host call {domain:?}/#{variant} is newer than mod ABI {}; \
                             replying Unsupported",
                            caller.data().mod_id,
                            mod_api::ABI_VERSION,
                        );
                        None
                    }
                    Err(e) => {
                        return Err(wasmtime::Error::msg(format!("malformed host call: {e}")));
                    }
                };
                let request_len = buf.len();
                {
                    let data = caller.data_mut();
                    data.dispatch_host_calls += 1;
                    if data.dispatch_host_calls > DISPATCH_HOST_CALL_MAX {
                        return Err(wasmtime::Error::msg(format!(
                            "dispatch exceeded {DISPATCH_HOST_CALL_MAX} host calls"
                        )));
                    }
                    data.charge_guest_time().map_err(wasmtime::Error::msg)?;
                    data.last_host_call = Some((buf, false));
                }
                let host_started = std::time::Instant::now();
                let ret = match call {
                    Some(call) => handle_host_call(caller.data_mut(), call),
                    None => HostRet::Unsupported,
                };
                caller.data_mut().dispatch_host_wall += host_started.elapsed();
                let mut bytes = std::mem::take(&mut caller.data_mut().reply_buf);
                let reply_len = match mod_api::encode_into(&ret, &mut bytes) {
                    Ok(len) => len,
                    Err(e) => {
                        return Err(wasmtime::Error::msg(format!("encode host reply: {e}")));
                    }
                };
                let cost = budget::host_call_fuel(caller.data().side, request_len, reply_len);
                let fuel = caller.get_fuel()?;
                caller.set_fuel(fuel.saturating_sub(cost))?;
                let alloc =
                    caller.data().alloc.clone().ok_or_else(|| {
                        wasmtime::Error::msg("host_dispatch during instantiation")
                    })?;
                let budget = {
                    let data = caller.data_mut();
                    if let Some((_, returned)) = &mut data.last_host_call {
                        *returned = true;
                    }
                    data.rearm()
                };
                caller.as_context_mut().set_epoch_deadline(budget);
                let reply_ptr = alloc.call(&mut caller, reply_len as u32)?;
                memory.write(&mut caller, reply_ptr as usize, &bytes[..reply_len])?;
                caller.data_mut().reply_buf = bytes;
                Ok(mod_api::pack_ptr_len(reply_ptr, reply_len as u32))
            },
        )
        .map_err(|e| format!("define host_dispatch: {e:#}"))?;
    Ok(linker)
}
