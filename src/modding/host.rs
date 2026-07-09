//! The wasmtime side of the host: engine configuration, module cache, the
//! `host_dispatch` import, and the per-mod store state (RNG streams, the
//! registration window, diagnostics counters).
//!
//! Engine config is part of the determinism contract (WIKI/modding.md): NaN
//! canonicalization ON, no threads, no WASI, no relaxed-SIMD, epoch
//! interruption armed by a background ticker thread so a runaway mod traps out
//! instead of hanging the tick loop.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

use mod_api::{HostCall, HostRet, MobSnapshot, PlayerSnapshot};
use wasmtime::{
    Caller, Config, Engine, Linker, Memory, Module, StoreLimits, StoreLimitsBuilder, TypedFunc,
};

use crate::block::Block;
use crate::entity::DroppedItem;
use crate::events::{ModAction, PostEvent, SimCtx};
use crate::item::{ItemStack, ItemType};
use crate::mathh::Vec3;

use super::convert::{to_ivec, to_vec};
use super::scope;

/// Per-entry limits for the mod KV surfaces (world / section-cell / mob).
/// Violations are [`HostRet::Error`] — a mod bug, surfaced loudly by the SDK.
const KV_MAX_KEY_BYTES: usize = 256;
const KV_MAX_VALUE_BYTES: usize = 64 * 1024;

/// How often the background ticker advances the engine epoch.
const EPOCH_PERIOD: Duration = Duration::from_millis(50);

/// Epochs a single guest dispatch may span before it traps: a generous ~2 s of
/// wall time for work that should take microseconds. Hitting it is a mod bug;
/// the mod is disabled for the session and the tick continues.
pub(super) const DISPATCH_DEADLINE_EPOCHS: u64 = 40;

/// Linear-memory cap per mod instance (64 MiB) — a leaky mod fails its own
/// allocations (and traps out) instead of eating the game's address space.
const GUEST_MEMORY_CAP: usize = 64 << 20;

/// The process-wide wasmtime engine, plus its epoch ticker thread. The ticker
/// only bumps a counter — it never touches the simulation — so determinism is
/// unaffected; it exists purely so the deadline can fire while the main thread
/// is stuck inside a guest.
pub(super) fn engine() -> &'static Engine {
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
pub(super) fn module_for(path: &Path) -> Result<Module, String> {
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
pub(super) enum Phase {
    Init,
    Run,
}

/// A registration collected during `mod_init`, applied to the bus/scheduler by
/// [`super::ModHost::initialize`] after the guest call returns.
pub(super) enum Registration {
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
    pub(super) fn is_gen(&self) -> bool {
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
pub(super) struct ModStoreData {
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
}

impl ModStoreData {
    pub(super) fn new(mod_id: &str, world_seed: u32) -> Self {
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
        }
    }

    fn register(&mut self, reg: Registration) -> HostRet {
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

    fn rng_next(&mut self, stream_key: &str) -> u64 {
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
fn intern_mod_id(id: &str) -> &'static str {
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

/// The mod-KV write guard: WRITES (set/delete) must use either the calling
/// mod's own `mod_id:` prefix or an exposed engine `petramond:` key. Reads may cross
/// namespaces (the interop surface), and keys/values are size-capped.
/// `Some(err)` rejects the call.
fn kv_write_guard(mod_id: &str, key: &str, value_len: usize) -> Option<HostRet> {
    if key.len() > KV_MAX_KEY_BYTES {
        return Some(HostRet::Error(format!(
            "KV key is {} bytes; the limit is {KV_MAX_KEY_BYTES}",
            key.len()
        )));
    }
    if value_len > KV_MAX_VALUE_BYTES {
        return Some(HostRet::Error(format!(
            "KV value is {value_len} bytes; the limit is {KV_MAX_VALUE_BYTES}"
        )));
    }
    public_write_key_guard(mod_id, key)
}

fn key_owned_by_namespace(namespace: &str, key: &str) -> bool {
    key.strip_prefix(namespace)
        .and_then(|rest| rest.strip_prefix(':'))
        .is_some_and(|name| !name.is_empty())
}

fn public_write_key_guard(mod_id: &str, key: &str) -> Option<HostRet> {
    let mod_owned = key_owned_by_namespace(mod_id, key);
    let engine_owned = key_owned_by_namespace(crate::registry::ENGINE_NAMESPACE, key);
    if !(mod_owned || engine_owned) {
        return Some(HostRet::Error(format!(
            "mod writes must use this mod's own namespace ('{mod_id}:name') or an engine-owned \
             '{engine}:name' key; got '{key}' (reads may cross namespaces)",
            engine = crate::registry::ENGINE_NAMESPACE
        )));
    }
    None
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
fn handle_host_call(data: &mut ModStoreData, call: HostCall) -> HostRet {
    data.stats.host_calls += 1;
    match call {
        HostCall::Log { .. }
        | HostCall::CurrentTick
        | HostCall::RngU64 { .. }
        | HostCall::RegisterTickSystem { .. }
        | HostCall::RegisterEventHandler { .. }
        | HostCall::RegisterHostileSpawner { .. }
        | HostCall::RegisterBlockBehavior { .. }
        | HostCall::RegisterAiNode { .. }
        | HostCall::ShaderSetParam { .. } => handle_core_call(data, call),
        HostCall::GetBlock { .. }
        | HostCall::GetBlocks { .. }
        | HostCall::SetBlock { .. }
        | HostCall::SetBlocks { .. }
        | HostCall::ScheduleTick { .. }
        | HostCall::IsLoaded { .. }
        | HostCall::LightAt { .. }
        | HostCall::BlockIsFullSpawnSupport { .. }
        | HostCall::SwapModelBlock { .. } => handle_block_call(&data.mod_id, call),
        HostCall::SpawnMob { .. }
        | HostCall::MobsInRadius { .. }
        | HostCall::DamageMob { .. }
        | HostCall::DespawnMob { .. }
        | HostCall::SpawnItem { .. } => handle_entity_call(&data.mod_id, call),
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
        | HostCall::ChatSend { .. } => handle_player_call(&data.mod_id, call),
        HostCall::EmitSound { .. }
        | HostCall::SoundPlayAt { .. }
        | HostCall::SoundPlayOnMob { .. }
        | HostCall::SoundStop { .. } => handle_sound_call(&data.mod_id, call),
        HostCall::WorldKvGet { .. }
        | HostCall::WorldKvSet { .. }
        | HostCall::WorldKvDelete { .. }
        | HostCall::SectionKvGet { .. }
        | HostCall::SectionKvSet { .. }
        | HostCall::SectionKvDelete { .. }
        | HostCall::MobKvGet { .. }
        | HostCall::MobKvSet { .. }
        | HostCall::MobKvDelete { .. } => handle_kv_call(&data.mod_id, call),
        HostCall::ResolveBlock { .. }
        | HostCall::RegisterWorldgenFeature { .. }
        | HostCall::RegisterStageReplacement { .. }
        | HostCall::RegisterGenerator { .. } => handle_worldgen_call(data, call),
        HostCall::GuiStateSet { .. }
        | HostCall::GuiStateGet { .. }
        | HostCall::GuiOpen { .. }
        | HostCall::GuiClose => handle_gui_call(&data.mod_id, call),
        HostCall::ContainerGet { .. }
        | HostCall::ContainerGetMany { .. }
        | HostCall::ContainerSet { .. }
        | HostCall::ItemInfo { .. }
        | HostCall::RecipeResult { .. } => handle_container_call(&data.mod_id, call),
    }
}

/// Mod container slots + the item/recipe registry reads that make furnace-like
/// mod logic possible without duplicating engine data.
fn handle_container_call(mod_id: &str, call: HostCall) -> HostRet {
    match call {
        HostCall::ContainerGet { pos } => sim_query(|ctx| {
            // Multi-cell blocks keep ONE container at the group anchor;
            // canonicalize so any footprint cell reads the same slots the GUI
            // and break-scatter use.
            let p = ctx.world.container_anchor(to_ivec(pos));
            HostRet::ContainerSlots(ctx.world.container_at(p).map(|c| {
                c.slots
                    .iter()
                    .map(|slot| slot.map(item_stack_data))
                    .collect()
            }))
        }),
        HostCall::ContainerGetMany { positions } => sim_query(|ctx| {
            HostRet::Containers(
                positions
                    .iter()
                    .map(|&pos| {
                        let p = ctx.world.container_anchor(to_ivec(pos));
                        ctx.world.container_at(p).map(|c| {
                            c.slots
                                .iter()
                                .map(|slot| slot.map(item_stack_data))
                                .collect()
                        })
                    })
                    .collect(),
            )
        }),
        HostCall::ContainerSet { pos, slots } => {
            // Resolve+validate every entry BEFORE any write, so a bad entry
            // can't leave a half-applied batch.
            let mut writes: Vec<(usize, Option<crate::item::ItemStack>)> = Vec::new();
            for (i, slot) in &slots {
                let i = *i as usize;
                if i >= crate::container::MAX_CONTAINER_SLOTS {
                    return HostRet::Error(format!(
                        "ContainerSet: slot {i} is past the cap ({})",
                        crate::container::MAX_CONTAINER_SLOTS
                    ));
                }
                let stack = match slot {
                    None => None,
                    Some(data) => {
                        // A typo'd registry key is not a protocol break: warn
                        // and refuse the batch (the GiveItem/EffectApply
                        // policy), don't trap the whole mod.
                        let Some(item) = item_by_key(&data.key) else {
                            log::warn!(
                                "[mod {mod_id}] ContainerSet: unknown item '{}' — \
                                 batch not applied",
                                data.key
                            );
                            return HostRet::Bool(false);
                        };
                        (data.count > 0).then(|| crate::item::ItemStack::new(item, data.count))
                    }
                };
                writes.push((i, stack));
            }
            let mod_id = mod_id.to_owned();
            sim_query(move |ctx| {
                // Same anchor rule as ContainerGet: writing through a
                // non-anchor footprint cell must not mint a second container
                // the GUI and break-scatter would never see.
                let p = ctx.world.container_anchor(to_ivec(pos));
                // A mod owns only its own blocks' containers: the block at
                // `pos` must be registered to the caller's namespace.
                // Stream-final read: a half-streamed cell shows the generated
                // base (a foreign block) — that must be "not stored", not a
                // namespace violation.
                let Some(block) = ctx.world.block_if_stream_final(p.x, p.y, p.z) else {
                    return HostRet::Bool(false);
                };
                let block_name = crate::registry::names()
                    .blocks
                    .name(block.id())
                    .unwrap_or("?");
                if !key_owned_by_namespace(&mod_id, block_name) {
                    return HostRet::Error(format!(
                        "ContainerSet: block '{block_name}' at {pos:?} is not owned by mod \
                         '{mod_id}' (writes are namespace-guarded; reads may cross)"
                    ));
                }
                let len = writes.iter().map(|(i, _)| i + 1).max().unwrap_or(0);
                if !ctx.world.ensure_container(p, len) {
                    return HostRet::Bool(false);
                }
                if let Some(container) = ctx.world.container_at_mut(p) {
                    for (i, stack) in writes {
                        container.slots[i] = stack;
                    }
                }
                ctx.world.mark_chunk_modified(p);
                HostRet::Bool(true)
            })
        }
        HostCall::ItemInfo { key } => {
            HostRet::ItemInfo(item_by_key(&key).map(|item| mod_api::ItemInfoData {
                max_stack: item.max_stack_size(),
                fuel_burn_ticks: item.fuel_burn_ticks() as u32,
                tags: item.tags().iter().map(|t| t.name().to_owned()).collect(),
            }))
        }
        HostCall::RecipeResult { class, key } => {
            let Some(recipes) = crate::modding::active_recipes() else {
                log::warn!("[mod {mod_id}] RecipeResult: no recipe catalog installed");
                return HostRet::ItemStack(None);
            };
            let Some(item) = item_by_key(&key) else {
                return HostRet::ItemStack(None);
            };
            HostRet::ItemStack(recipes.process(&class, item).map(item_stack_data))
        }
        other => HostRet::Error(format!(
            "non-container call {other:?} mis-routed to handle_container_call (host bug)"
        )),
    }
}

/// An engine stack as its ABI crossing (registry key + count).
fn item_stack_data(stack: crate::item::ItemStack) -> mod_api::ItemStackData {
    mod_api::ItemStackData {
        key: stack.item.key().to_owned(),
        count: stack.count,
    }
}

/// Store-side core calls: logging, the tick counter, RNG streams, the
/// `mod_init` registration window, and shader params.
fn handle_core_call(data: &mut ModStoreData, call: HostCall) -> HostRet {
    match call {
        HostCall::Log { msg } => {
            log::info!("[mod {}] {msg}", data.mod_id);
            HostRet::Unit
        }
        HostCall::CurrentTick => match scope::with_active(|ctx| ctx.world.current_tick()) {
            Some(tick) => HostRet::U64(tick),
            None => HostRet::Error("no simulation context is active".into()),
        },
        HostCall::RngU64 { stream_key } => HostRet::U64(data.rng_next(&stream_key)),
        HostCall::RegisterTickSystem {
            stage,
            attach,
            priority,
            system_id,
        } => data.register(Registration::TickSystem {
            stage,
            attach,
            priority,
            system_id,
        }),
        HostCall::RegisterEventHandler {
            event,
            priority,
            handler_id,
        } => data.register(Registration::EventHandler {
            event,
            priority,
            handler_id,
        }),
        HostCall::RegisterHostileSpawner {
            callback_id,
            priority,
        } => data.register(Registration::HostileSpawner {
            priority,
            callback_id,
        }),
        HostCall::RegisterBlockBehavior { key, callback_id } => {
            // A behavior key routes hooks back to its owner, so it must carry
            // THIS mod's namespace (same ownership rule as catalog keys).
            if !key_owned_by_namespace(&data.mod_id, &key) {
                return HostRet::Error(format!(
                    "block behavior key '{key}' must be namespaced '{}:name'",
                    data.mod_id
                ));
            }
            data.register(Registration::BlockBehavior { key, callback_id })
        }
        HostCall::RegisterAiNode { key, callback_id } => {
            if !key_owned_by_namespace(&data.mod_id, &key) {
                return HostRet::Error(format!(
                    "AI node key '{key}' must be namespaced '{}:name'",
                    data.mod_id
                ));
            }
            data.register(Registration::AiNode { key, callback_id })
        }
        HostCall::ShaderSetParam { key, value } => match public_write_key_guard(&data.mod_id, &key)
        {
            Some(e) => e,
            None => sim_call(|ctx| ctx.world.set_shader_param(key, value)),
        },
        other => HostRet::Error(format!(
            "non-core call {other:?} mis-routed to handle_core_call (host bug)"
        )),
    }
}

/// Phase 3b: blocks (all sim-scoped, delegating to World).
fn handle_block_call(mod_id: &str, call: HostCall) -> HostRet {
    match call {
        HostCall::SwapModelBlock { pos, block } => match checked_block(block) {
            Err(e) => e,
            Ok(b) => {
                // Both sides of the swap must be the caller's own blocks: this
                // is a machine flipping ITS placed variant, never a tool for
                // rewriting someone else's content.
                let new_name = crate::registry::names().blocks.name(b.id()).unwrap_or("?");
                if !key_owned_by_namespace(mod_id, new_name) {
                    return HostRet::Error(format!(
                        "SwapModelBlock: block '{new_name}' is not owned by mod '{mod_id}'"
                    ));
                }
                let mod_id = mod_id.to_owned();
                sim_query(move |ctx| {
                    let p = to_ivec(pos);
                    // Stream-final read: while a saved overlay is in flight
                    // the cell shows the generated base — a foreign block —
                    // which must read as "unloaded", not as a namespace
                    // violation.
                    let Some(old) = ctx.world.block_if_stream_final(p.x, p.y, p.z) else {
                        return HostRet::Bool(false);
                    };
                    let old_name = crate::registry::names()
                        .blocks
                        .name(old.id())
                        .unwrap_or("?");
                    if !key_owned_by_namespace(&mod_id, old_name) {
                        return HostRet::Error(format!(
                            "SwapModelBlock: block '{old_name}' at {pos:?} is not owned by mod \
                             '{mod_id}'"
                        ));
                    }
                    HostRet::Bool(ctx.world.swap_model_block(p, b))
                })
            }
        },
        // Mod reads report None ("unloaded") while a section's streamed
        // content is not final — a half-streamed read would show the
        // generated base where the player's saved record is about to land.
        HostCall::GetBlock { pos } => sim_query(|ctx| {
            let p = to_ivec(pos);
            HostRet::Block(
                ctx.world
                    .block_if_stream_final(p.x, p.y, p.z)
                    .map(|b| mod_api::BlockId(b.id())),
            )
        }),
        HostCall::GetBlocks { positions } => sim_query(|ctx| {
            HostRet::Blocks(
                positions
                    .iter()
                    .map(|&pos| {
                        let p = to_ivec(pos);
                        ctx.world
                            .block_if_stream_final(p.x, p.y, p.z)
                            .map(|b| mod_api::BlockId(b.id()))
                    })
                    .collect(),
            )
        }),
        HostCall::SetBlock { pos, block } => match checked_block(block) {
            Err(e) => e,
            Ok(b) => sim_query(|ctx| {
                let p = to_ivec(pos);
                HostRet::Bool(ctx.world.set_block_world(p.x, p.y, p.z, b))
            }),
        },
        HostCall::SetBlocks { blocks } => sim_query(|ctx| {
            let mut set = 0u64;
            for &(pos, block) in &blocks {
                let Ok(b) = checked_block(block) else {
                    return HostRet::Error(format!("SetBlocks: unregistered block id {}", block.0));
                };
                let p = to_ivec(pos);
                if ctx.world.set_block_world(p.x, p.y, p.z, b) {
                    set += 1;
                }
            }
            HostRet::U64(set)
        }),
        HostCall::ScheduleTick { pos, delay } => {
            sim_call(|ctx| ctx.world.schedule_tick(to_ivec(pos), delay))
        }
        HostCall::IsLoaded { pos } => sim_query(|ctx| {
            let p = to_ivec(pos);
            HostRet::Bool(ctx.world.section_stream_final_at(p.x, p.y, p.z))
        }),
        HostCall::LightAt { pos } => sim_query(|ctx| {
            let p = to_ivec(pos);
            HostRet::Light {
                combined: ctx.world.combined_light6_at_world(p.x, p.y, p.z),
                sky: ctx.world.skylight6_at_world(p.x, p.y, p.z),
                block: ctx.world.blocklight6_at_world(p.x, p.y, p.z),
            }
        }),
        HostCall::BlockIsFullSpawnSupport { pos } => sim_query(|ctx| {
            let p = to_ivec(pos);
            HostRet::Bool(ctx.world.block_is_full_spawn_support(p.x, p.y, p.z))
        }),
        other => HostRet::Error(format!(
            "non-block call {other:?} mis-routed to handle_block_call (host bug)"
        )),
    }
}

/// Phase 3b: entities (mob spawn/query/hurt/despawn, item drops).
fn handle_entity_call(mod_id: &str, call: HostCall) -> HostRet {
    match call {
        HostCall::SpawnMob { key, pos, yaw } => match finite3(pos, "SpawnMob.pos") {
            Err(e) => e,
            Ok(pos) => sim_query(|ctx| {
                let Some(kind) = crate::mob::defs()
                    .iter()
                    .position(|d| d.key == key)
                    .map(|i| crate::mob::Mob(i as u8))
                else {
                    log::warn!("[mod {mod_id}] SpawnMob: unknown species '{key}'");
                    return HostRet::Bool(false);
                };
                let spawned = ctx.world.spawn_mob(kind, pos, yaw);
                if spawned {
                    ctx.queue.emit(PostEvent::MobSpawned { kind, pos });
                }
                HostRet::Bool(spawned)
            }),
        },
        HostCall::MobsInRadius { pos, radius } => match finite3(pos, "MobsInRadius.pos") {
            Err(e) => e,
            Ok(pos) => sim_query(|ctx| {
                if !radius.is_finite() {
                    return HostRet::Error("MobsInRadius: non-finite radius".into());
                }
                let r2 = radius * radius;
                HostRet::Mobs(
                    ctx.world
                        .mobs()
                        .instances()
                        .iter()
                        .enumerate()
                        .filter(|(_, m)| !m.is_dead())
                        .filter(|(_, m)| (m.pos - pos).length_squared() <= r2)
                        .map(|(i, m)| MobSnapshot {
                            index: i as u32,
                            key: crate::mob::def(m.kind).key.to_owned(),
                            pos: [m.pos.x, m.pos.y, m.pos.z],
                            health: m.health(),
                            id: m.id(),
                        })
                        .collect(),
                )
            }),
        },
        HostCall::DamageMob {
            index,
            amount,
            origin,
        } => match origin.map(|p| finite3(p, "DamageMob.origin")).transpose() {
            Err(e) => e,
            Ok(origin) => {
                let mod_id = intern_mod_id(mod_id);
                sim_call(|ctx| {
                    ctx.queue.push_action(ModAction::DamageMob {
                        index: index as usize,
                        amount,
                        mod_id,
                        origin,
                    })
                })
            }
        },
        HostCall::DespawnMob { index } => {
            sim_query(|ctx| HostRet::Bool(ctx.world.mobs_mut().remove(index as usize)))
        }
        HostCall::SpawnItem {
            item_key,
            count,
            pos,
        } => match finite3(pos, "SpawnItem.pos") {
            Err(e) => e,
            Ok(pos) => sim_query(|ctx| {
                let Some(item) = item_by_key(&item_key) else {
                    log::warn!("[mod {mod_id}] SpawnItem: unknown item '{item_key}'");
                    return HostRet::Bool(false);
                };
                if count == 0 {
                    return HostRet::Bool(false);
                }
                spawn_item_stacks(ctx, item, count, pos);
                HostRet::Bool(true)
            }),
        },
        other => HostRet::Error(format!(
            "non-entity call {other:?} mis-routed to handle_entity_call (host bug)"
        )),
    }
}

/// Phase 3b: player (snapshot, damage/kill through the funnel, inventory,
/// movement primitives).
fn handle_player_call(mod_id: &str, call: HostCall) -> HostRet {
    match call {
        HostCall::PlayerState => sim_query(|ctx| {
            let p = &*ctx.player;
            HostRet::Player(PlayerSnapshot {
                pos: [p.pos.x, p.pos.y, p.pos.z],
                vel: [p.vel.x, p.vel.y, p.vel.z],
                yaw: p.yaw,
                pitch: p.pitch,
                health: p.health(),
                on_ground: p.on_ground,
                spectator: p.is_spectator(),
            })
        }),
        HostCall::DamagePlayer { amount } => {
            let mod_id = intern_mod_id(mod_id);
            sim_call(|ctx| {
                ctx.queue
                    .push_action(ModAction::DamagePlayer { amount, mod_id })
            })
        }
        HostCall::ApplyKnockback { impulse } => match finite3(impulse, "ApplyKnockback.impulse") {
            Err(e) => e,
            Ok(impulse) => sim_call(|ctx| ctx.player.apply_knockback(impulse)),
        },
        HostCall::GiveItem { item_key, count } => sim_query(|ctx| {
            let Some(item) = item_by_key(&item_key) else {
                log::warn!("[mod {mod_id}] GiveItem: unknown item '{item_key}'");
                return HostRet::Bool(false);
            };
            give_item(ctx, item, count);
            HostRet::Bool(true)
        }),
        HostCall::KillPlayer => {
            let mod_id = intern_mod_id(mod_id);
            sim_call(|ctx| ctx.queue.push_action(ModAction::KillPlayer { mod_id }))
        }
        HostCall::SetHealth { value } => sim_call(|ctx| ctx.player.set_health(value)),
        HostCall::Teleport { pos } => match finite3(pos, "Teleport.pos") {
            Err(e) => e,
            Ok(pos) => sim_call(|ctx| ctx.player.teleport(pos)),
        },
        // Status effects are player-state primitives like SetHealth: direct
        // mutation, no events. Unknown keys are forgiving (Bool(false)) — a
        // typo'd key is not a protocol break.
        HostCall::EffectApply { key, ticks } => sim_query(|ctx| {
            let Some(effect) = crate::effect::by_name(&key) else {
                log::warn!("[mod {mod_id}] EffectApply: unknown effect '{key}'");
                return HostRet::Bool(false);
            };
            ctx.player.apply_effect(effect, ticks);
            HostRet::Bool(true)
        }),
        HostCall::EffectRemove { key } => sim_query(|ctx| {
            let Some(effect) = crate::effect::by_name(&key) else {
                log::warn!("[mod {mod_id}] EffectRemove: unknown effect '{key}'");
                return HostRet::Bool(false);
            };
            ctx.player.remove_effect(effect);
            HostRet::Bool(true)
        }),
        HostCall::EffectsActive => sim_query(|ctx| {
            HostRet::Effects(
                ctx.player
                    .effects()
                    .iter()
                    .map(|e| mod_api::EffectStateData {
                        key: e.effect.def().name.to_owned(),
                        remaining: e.remaining,
                    })
                    .collect(),
            )
        }),
        HostCall::ChatSend { text, targets } => sim_query(|ctx| {
            // Empty / whitespace-only text is rejected at delivery time too;
            // report it here so the mod can tell a no-op from a queued send.
            if text.trim().is_empty() {
                return HostRet::Bool(false);
            }
            ctx.queue.push_action(ModAction::ChatSend { text, targets });
            HostRet::Bool(true)
        }),
        other => HostRet::Error(format!(
            "non-player call {other:?} mis-routed to handle_player_call (host bug)"
        )),
    }
}

/// Phase 3b: sound (one-shots plus the handle-based spatial commands; the sim
/// never touches audio — everything rides `TickEvents` to the app layer).
fn handle_sound_call(mod_id: &str, call: HostCall) -> HostRet {
    match call {
        HostCall::EmitSound { key, pos } => sim_query(|ctx| {
            let Some(sound) = crate::audio::sound_by_name(&key) else {
                log::warn!("[mod {mod_id}] EmitSound: unknown sound '{key}'");
                return HostRet::Bool(false);
            };
            // The sim never touches audio: the sound rides the NON-lossy tick
            // queue on `TickEvents` and the app layer plays it next frame.
            ctx.feed.world.sounds.push(crate::game::ModSound {
                sound,
                pos: pos.map(to_vec),
            });
            HostRet::Bool(true)
        }),
        HostCall::SoundPlayAt {
            key,
            pos,
            volume,
            pitch,
        } => sim_query(|ctx| {
            let Some(sound) = crate::audio::sound_by_name(&key) else {
                log::warn!("[mod {mod_id}] SoundPlayAt: unknown sound '{key}'");
                return HostRet::U64(0);
            };
            if !spatial_sound_params_ok(pos, volume, pitch) {
                log::warn!("[mod {mod_id}] SoundPlayAt: rejected non-finite or negative parameter");
                return HostRet::U64(0);
            }
            let handle = ctx.feed.alloc_spatial_sound_handle();
            ctx.feed
                .world
                .spatial_sounds
                .push(crate::game::ModSpatialSoundCommand::PlayAt {
                    handle,
                    sound,
                    pos: to_vec(pos),
                    volume,
                    pitch,
                });
            HostRet::U64(handle)
        }),
        HostCall::SoundPlayOnMob {
            mob_id,
            key,
            volume,
            pitch,
        } => sim_query(|ctx| {
            let Some(sound) = crate::audio::sound_by_name(&key) else {
                log::warn!("[mod {mod_id}] SoundPlayOnMob: unknown sound '{key}'");
                return HostRet::U64(0);
            };
            if !spatial_sound_scalar_params_ok(volume, pitch) {
                log::warn!(
                    "[mod {mod_id}] SoundPlayOnMob: rejected non-finite or negative parameter"
                );
                return HostRet::U64(0);
            }
            let Some(last_pos) = ctx
                .world
                .mobs()
                .instances()
                .iter()
                .find(|m| m.id() == mob_id && !m.is_dead())
                .map(|m| m.pos)
            else {
                log::warn!("[mod {mod_id}] SoundPlayOnMob: no live mob with stable id {mob_id}");
                return HostRet::U64(0);
            };
            let handle = ctx.feed.alloc_spatial_sound_handle();
            ctx.feed
                .world
                .spatial_sounds
                .push(crate::game::ModSpatialSoundCommand::PlayOnMob {
                    handle,
                    sound,
                    mob_id,
                    volume,
                    pitch,
                    last_pos,
                });
            HostRet::U64(handle)
        }),
        HostCall::SoundStop { handle } => sim_call(|ctx| {
            if handle != 0 {
                ctx.feed
                    .world
                    .spatial_sounds
                    .push(crate::game::ModSpatialSoundCommand::Stop { handle });
            }
        }),
        other => HostRet::Error(format!(
            "non-sound call {other:?} mis-routed to handle_sound_call (host bug)"
        )),
    }
}

/// Phase 3b: persistent KV (world / section-cell / mob surfaces; writes pass
/// [`kv_write_guard`]).
fn handle_kv_call(mod_id: &str, call: HostCall) -> HostRet {
    match call {
        HostCall::WorldKvGet { key } => {
            sim_query(|ctx| HostRet::Bytes(ctx.world.mod_kv_get(&key).map(<[u8]>::to_vec)))
        }
        HostCall::WorldKvSet { key, value } => match kv_write_guard(mod_id, &key, value.len()) {
            Some(err) => err,
            None => sim_call(|ctx| ctx.world.mod_kv_set(key, value)),
        },
        HostCall::WorldKvDelete { key } => match kv_write_guard(mod_id, &key, 0) {
            Some(err) => err,
            None => sim_query(|ctx| HostRet::Bool(ctx.world.mod_kv_remove(&key))),
        },
        HostCall::SectionKvGet { pos, key } => sim_query(|ctx| {
            let p = to_ivec(pos);
            HostRet::Bytes(
                ctx.world
                    .cell_kv_get(p.x, p.y, p.z, &key)
                    .map(<[u8]>::to_vec),
            )
        }),
        HostCall::SectionKvSet { pos, key, value } => {
            match kv_write_guard(mod_id, &key, value.len()) {
                Some(err) => err,
                None => sim_query(|ctx| {
                    let p = to_ivec(pos);
                    HostRet::Bool(ctx.world.cell_kv_set(p.x, p.y, p.z, key, value))
                }),
            }
        }
        HostCall::SectionKvDelete { pos, key } => match kv_write_guard(mod_id, &key, 0) {
            Some(err) => err,
            None => sim_query(|ctx| {
                let p = to_ivec(pos);
                HostRet::Bool(ctx.world.cell_kv_remove(p.x, p.y, p.z, &key))
            }),
        },
        HostCall::MobKvGet { mob_index, key } => sim_query(|ctx| {
            HostRet::Bytes(
                ctx.world
                    .mobs()
                    .mod_kv_get(mob_index as usize, &key)
                    .map(<[u8]>::to_vec),
            )
        }),
        HostCall::MobKvSet {
            mob_index,
            key,
            value,
        } => match kv_write_guard(mod_id, &key, value.len()) {
            Some(err) => err,
            None => sim_query(|ctx| {
                HostRet::Bool(
                    ctx.world
                        .mobs_mut()
                        .mod_kv_set(mob_index as usize, key, value),
                )
            }),
        },
        HostCall::MobKvDelete { mob_index, key } => match kv_write_guard(mod_id, &key, 0) {
            Some(err) => err,
            None => sim_query(|ctx| {
                HostRet::Bool(ctx.world.mobs_mut().mod_kv_remove(mob_index as usize, &key))
            }),
        },
        other => HostRet::Error(format!(
            "non-KV call {other:?} mis-routed to handle_kv_call (host bug)"
        )),
    }
}

/// Phase 4: worldgen hooks (block-name resolution plus the gen registrations).
fn handle_worldgen_call(data: &mut ModStoreData, call: HostCall) -> HostRet {
    match call {
        // ResolveBlock reads only the process-wide registry, so it is legal on
        // ANY instance — worldgen worker instances (which never get a SimCtx)
        // resolve their block ids during their own `mod_init`.
        HostCall::ResolveBlock { key } => HostRet::Block(
            crate::registry::names()
                .blocks
                .id(&key)
                .map(mod_api::BlockId),
        ),
        HostCall::RegisterWorldgenFeature { feature_id, stage } => {
            if stage == mod_api::WorldgenStage::Climate {
                return HostRet::Error(
                    "worldgen features cannot attach after the climate stage (it is \
                     column-level, before any blocks exist); use Terrain or later"
                        .into(),
                );
            }
            data.register(Registration::WorldgenFeature { stage, feature_id })
        }
        HostCall::RegisterStageReplacement { stage, callback_id } => {
            data.register(Registration::StageReplacement { stage, callback_id })
        }
        HostCall::RegisterGenerator { callback_id } => {
            data.register(Registration::Generator { callback_id })
        }
        other => HostRet::Error(format!(
            "non-worldgen call {other:?} mis-routed to handle_worldgen_call (host bug)"
        )),
    }
}

/// Phase 5: mod GUIs (session state map plus open/close).
/// State keys are mod-local: the map belongs to one GUI session (cleared
/// on open/close), so unlike the persistent KV no prefix is enforced.
fn handle_gui_call(mod_id: &str, call: HostCall) -> HostRet {
    match call {
        HostCall::GuiStateSet { key, value } => sim_call(|ctx| {
            crate::gui::gui_state_set(ctx.gui_state, key, super::convert::gui_value(value))
        }),
        HostCall::GuiStateGet { key } => sim_query(|ctx| {
            HostRet::GuiValue(ctx.gui_state.get(&key).map(super::convert::gui_value_out))
        }),
        HostCall::GuiOpen { kind_key } => {
            // Resolve WITHOUT registering: opening a kind nothing declared is
            // a mod bug, reported forgivingly (like an unknown sound key).
            let Some(kind) = crate::gui::resolve_kind(&kind_key).filter(|k| k.is_mod()) else {
                log::warn!("[mod {mod_id}] GuiOpen: unknown or non-mod gui kind '{kind_key}'");
                return HostRet::Bool(false);
            };
            sim_query(|ctx| {
                ctx.queue.push_action(ModAction::OpenGui { kind });
                HostRet::Bool(true)
            })
        }
        HostCall::GuiClose => sim_call(|ctx| ctx.queue.push_action(ModAction::CloseGui)),
        other => HostRet::Error(format!(
            "non-GUI call {other:?} mis-routed to handle_gui_call (host bug)"
        )),
    }
}

/// Run a call that mutates the live simulation, or reject it when no guest
/// dispatch scope is active (the same gate `CurrentTick` uses).
fn sim_call(f: impl FnOnce(&mut SimCtx<'_>)) -> HostRet {
    match scope::with_active(f) {
        Some(()) => HostRet::Unit,
        None => HostRet::Error("no simulation context is active".into()),
    }
}

/// [`sim_call`] for calls that compute their own reply.
fn sim_query(f: impl FnOnce(&mut SimCtx<'_>) -> HostRet) -> HostRet {
    scope::with_active(f)
        .unwrap_or_else(|| HostRet::Error("no simulation context is active".into()))
}

/// Validate an ABI block id against the loaded registry — an unregistered id
/// must never reach world storage.
fn checked_block(block: mod_api::BlockId) -> Result<Block, HostRet> {
    if (block.0 as usize) < Block::all().len() {
        Ok(Block(block.0))
    } else {
        Err(HostRet::Error(format!(
            "unregistered block id {} (ids are session-scoped; resolve them from your own \
             catalog rows, never persist them)",
            block.0
        )))
    }
}

/// Reject non-finite guest floats before they reach engine state (NaNs are
/// canonicalized by wasmtime but still NaN; infinities pass through).
fn finite3(v: [f32; 3], what: &str) -> Result<Vec3, HostRet> {
    if v.iter().all(|c| c.is_finite()) {
        Ok(to_vec(v))
    } else {
        Err(HostRet::Error(format!("{what}: non-finite component")))
    }
}

fn spatial_sound_params_ok(pos: [f32; 3], volume: f32, pitch: f32) -> bool {
    pos.iter().all(|c| c.is_finite()) && spatial_sound_scalar_params_ok(volume, pitch)
}

fn spatial_sound_scalar_params_ok(volume: f32, pitch: f32) -> bool {
    volume.is_finite() && volume >= 0.0 && pitch.is_finite() && pitch > 0.0
}

/// The runtime item registered under `key` (`ItemType::key` — the stable
/// snake_case identity, `mod_id:name` for pack items).
fn item_by_key(key: &str) -> Option<ItemType> {
    ItemType::all().iter().copied().find(|i| i.key() == key)
}

/// Spawn `count` of `item` as dropped entities at `pos`, splitting oversized
/// counts into max-stack-size drops. Pop seeds derive from (tick, pos, i) so
/// the spawn is deterministic without any Game-side counter.
fn spawn_item_stacks(ctx: &mut SimCtx<'_>, item: ItemType, count: u8, pos: Vec3) {
    let cell = crate::mathh::voxel_at(pos);
    let sky = ctx.world.skylight6_at_world(cell.x, cell.y, cell.z);
    let block = ctx.world.blocklight6_at_world(cell.x, cell.y, cell.z);
    let mut remaining = count;
    let mut i = 0u32;
    while remaining > 0 {
        let put = remaining.min(item.max_stack_size());
        remaining -= put;
        let seed = drop_seed(ctx.world.current_tick(), pos, i);
        let mut drop = DroppedItem::new(pos, ItemStack::new(item, put), seed);
        drop.skylight = sky;
        drop.blocklight = block;
        ctx.world.spawn_item(drop);
        i += 1;
    }
}

/// Deterministic per-drop pop seed: a SplitMix64 finalizer over the tick, the
/// spawn position bits, and the in-call index.
fn drop_seed(tick: u64, pos: Vec3, i: u32) -> u32 {
    let mut z = tick
        ^ ((pos.x.to_bits() as u64) << 32 | pos.z.to_bits() as u64)
        ^ ((pos.y.to_bits() as u64) << 16)
        ^ ((i as u64) << 1);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    (z ^ (z >> 31)) as u32
}

/// Give the player `count` of `item` through the normal inventory fill;
/// whatever does not fit drops at the player's feet like any other overflow.
fn give_item(ctx: &mut SimCtx<'_>, item: ItemType, count: u8) {
    let mut remaining = count;
    while remaining > 0 {
        let put = remaining.min(item.max_stack_size());
        remaining -= put;
        if let Some(leftover) = ctx.player.inventory.add(ItemStack::new(item, put)) {
            let at = ctx.player.body_center();
            let seed = drop_seed(ctx.world.current_tick(), at, remaining as u32);
            let cell = crate::mathh::voxel_at(at);
            let mut drop = DroppedItem::new(at, leftover, seed);
            drop.skylight = ctx.world.skylight6_at_world(cell.x, cell.y, cell.z);
            drop.blocklight = ctx.world.blocklight6_at_world(cell.x, cell.y, cell.z);
            ctx.world.spawn_item(drop);
        }
    }
}

/// Build the linker exposing the single guest import,
/// `env::host_dispatch(ptr, len) -> u64` (packed reply `ptr << 32 | len`).
/// Reply buffers are allocated IN the guest via its exported `mod_alloc` —
/// wasmtime host functions may re-enter the calling instance — and freed by
/// the guest once decoded.
pub(super) fn linker() -> Result<Linker<ModStoreData>, String> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chunk::{ChunkPos, SECTION_VOLUME};
    use crate::events::PostQueue;
    use crate::game::TickEvents;
    use crate::player::Player;
    use crate::world::World;

    /// Run `f` with a live SimCtx published, as if inside a guest dispatch.
    fn with_ctx(f: impl FnOnce()) {
        let mut world = World::new(1, 1);
        let mut player = Player::new(Vec3::new(0.0, 80.0, 0.0));
        let mut feed = TickEvents::default();
        let mut queue = PostQueue::default();
        let mut gui = crate::gui::empty_gui_state();
        let mut ctx = SimCtx {
            world: &mut world,
            player: &mut player,
            gui_state: &mut gui,
            feed: &mut feed,
            queue: &mut queue,
        };
        scope::enter(&mut ctx, f);
    }

    /// Container host calls canonicalize any footprint cell of a multi-cell
    /// model block to the group ANCHOR: a write through a non-anchor cell
    /// must land in the one anchored container (the same slots the GUI and
    /// break-scatter use), never mint a second store at that cell.
    #[test]
    fn container_calls_canonicalize_to_the_group_anchor() {
        let mut world = World::new(1, 4);
        world.clear_world();
        world.insert_chunk_for_test(ChunkPos::new(0, 0), crate::chunk::Chunk::new(0, 0));
        let origin = crate::mathh::IVec3::new(5, 64, 5);
        assert!(world.place_model_block(origin, crate::block::Block::FurnitureWorkbench));
        let (_, anchor, cells) = world.model_group(origin).expect("a placed model group");
        let far = *cells
            .iter()
            .find(|c| **c != anchor)
            .expect("a non-anchor cell");

        // The workbench is engine-owned and ContainerSet is guarded to the
        // caller's own namespace, so the test store impersonates the engine
        // namespace — this keeps the test off the heavy WASM fixture.
        let mut store = ModStoreData::new(crate::registry::ENGINE_NAMESPACE, 1);
        let mut player = Player::new(Vec3::new(0.0, 80.0, 0.0));
        let mut feed = TickEvents::default();
        let mut queue = PostQueue::default();
        let mut gui = crate::gui::empty_gui_state();
        let mut ctx = SimCtx {
            world: &mut world,
            player: &mut player,
            gui_state: &mut gui,
            feed: &mut feed,
            queue: &mut queue,
        };
        scope::enter(&mut ctx, || {
            let set = handle_host_call(
                &mut store,
                HostCall::ContainerSet {
                    pos: [far.x, far.y, far.z],
                    slots: vec![(
                        0,
                        Some(mod_api::ItemStackData {
                            key: "petramond:coal".into(),
                            count: 3,
                        }),
                    )],
                },
            );
            assert_eq!(set, HostRet::Bool(true));
            // Reading through a different cell (the anchor) sees the write.
            let got = handle_host_call(
                &mut store,
                HostCall::ContainerGet {
                    pos: [anchor.x, anchor.y, anchor.z],
                },
            );
            let HostRet::ContainerSlots(Some(slots)) = got else {
                panic!("expected slots from the anchor, got {got:?}");
            };
            assert_eq!(slots[0].as_ref().map(|s| s.count), Some(3));
        });
        // One container, keyed at the anchor — nothing stranded at the cell.
        assert!(world.container_at(anchor).is_some());
        assert!(world.container_at(far).is_none());
    }

    /// The KV namespace contract: writes must carry the CALLER's own
    /// `mod_id:` prefix or an engine-owned `petramond:` key (foreign and bare keys
    /// are rejected with an error), while reads may cross namespaces — that
    /// asymmetry IS the cross-mod interop surface. Size caps reject oversized
    /// values.
    #[test]
    fn kv_writes_enforce_own_namespace_and_reads_cross() {
        let mut alpha = ModStoreData::new("alpha", 1);
        let mut beta = ModStoreData::new("beta", 1);
        with_ctx(|| {
            // Own-prefix write lands.
            assert_eq!(
                handle_host_call(
                    &mut alpha,
                    HostCall::WorldKvSet {
                        key: "alpha:x".into(),
                        value: vec![7],
                    },
                ),
                HostRet::Unit
            );
            // Engine-owned public surfaces are intentionally writable.
            assert_eq!(
                handle_host_call(
                    &mut beta,
                    HostCall::WorldKvSet {
                        key: "petramond:time".into(),
                        value: vec![1],
                    },
                ),
                HostRet::Unit
            );
            // A foreign-prefix write is rejected...
            assert!(matches!(
                handle_host_call(
                    &mut beta,
                    HostCall::WorldKvSet {
                        key: "alpha:x".into(),
                        value: vec![9],
                    },
                ),
                HostRet::Error(_)
            ));
            // ...and so are bare / degenerate keys.
            for bad in ["x", "alpha:", "petramond:", "alphax:y", "beta"] {
                assert!(
                    matches!(
                        handle_host_call(
                            &mut beta,
                            HostCall::WorldKvSet {
                                key: bad.into(),
                                value: vec![1],
                            },
                        ),
                        HostRet::Error(_)
                    ),
                    "write with key '{bad}' must be rejected"
                );
            }
            // The rejected write changed nothing; a cross-namespace READ works.
            assert_eq!(
                handle_host_call(
                    &mut beta,
                    HostCall::WorldKvGet {
                        key: "alpha:x".into(),
                    },
                ),
                HostRet::Bytes(Some(vec![7]))
            );
            assert_eq!(
                handle_host_call(
                    &mut alpha,
                    HostCall::WorldKvGet {
                        key: "petramond:time".into(),
                    },
                ),
                HostRet::Bytes(Some(vec![1]))
            );
            // Deletes are writes: foreign rejected, own applies.
            assert!(matches!(
                handle_host_call(
                    &mut beta,
                    HostCall::WorldKvDelete {
                        key: "alpha:x".into(),
                    },
                ),
                HostRet::Error(_)
            ));
            assert_eq!(
                handle_host_call(
                    &mut alpha,
                    HostCall::WorldKvDelete {
                        key: "alpha:x".into(),
                    },
                ),
                HostRet::Bool(true)
            );
            // The value size cap holds (same guard on every KV write surface).
            assert!(matches!(
                handle_host_call(
                    &mut alpha,
                    HostCall::WorldKvSet {
                        key: "alpha:big".into(),
                        value: vec![0; KV_MAX_VALUE_BYTES + 1],
                    },
                ),
                HostRet::Error(_)
            ));
        });
        // Outside any dispatch scope, sim-touching KV calls are rejected.
        assert!(matches!(
            handle_host_call(
                &mut alpha,
                HostCall::WorldKvGet {
                    key: "alpha:x".into(),
                },
            ),
            HostRet::Error(_)
        ));
    }

    /// Shader params are the visual environment surface mods use for sky
    /// shaders and other pack-owned effects: own namespace or engine `petramond:*`,
    /// tick-scoped, and stored in the world's neutral environment snapshot.
    #[test]
    fn shader_param_writes_are_namespaced_and_tick_scoped() {
        let mut alpha = ModStoreData::new("alpha", 1);
        let mut beta = ModStoreData::new("beta", 1);
        let mut world = World::new(1, 1);
        let mut player = Player::new(Vec3::new(0.0, 80.0, 0.0));
        let mut feed = TickEvents::default();
        let mut queue = PostQueue::default();
        let mut gui = crate::gui::empty_gui_state();
        let mut ctx = SimCtx {
            world: &mut world,
            player: &mut player,
            gui_state: &mut gui,
            feed: &mut feed,
            queue: &mut queue,
        };

        scope::enter(&mut ctx, || {
            assert_eq!(
                handle_host_call(
                    &mut alpha,
                    HostCall::ShaderSetParam {
                        key: "alpha:sky".into(),
                        value: [0.25, 0.5, 0.75, 1.0],
                    },
                ),
                HostRet::Unit
            );
            assert!(matches!(
                handle_host_call(
                    &mut beta,
                    HostCall::ShaderSetParam {
                        key: "alpha:sky".into(),
                        value: [1.0; 4],
                    },
                ),
                HostRet::Error(_)
            ));
            assert_eq!(
                handle_host_call(
                    &mut beta,
                    HostCall::ShaderSetParam {
                        key: "petramond:light".into(),
                        value: [0.8, 0.0, 0.0, 0.0],
                    },
                ),
                HostRet::Unit
            );
        });

        assert_eq!(
            world.environment().shader_params().get("alpha:sky"),
            Some(&[0.25, 0.5, 0.75, 1.0])
        );
        assert_eq!(
            world.environment().shader_params().get("petramond:light"),
            Some(&[0.8, 0.0, 0.0, 0.0])
        );
        assert!(matches!(
            handle_host_call(
                &mut alpha,
                HostCall::ShaderSetParam {
                    key: "alpha:outside".into(),
                    value: [0.0; 4],
                },
            ),
            HostRet::Error(_)
        ));
    }

    /// Worldgen hook registration is `mod_init`-window-gated like every other
    /// registration, and `Climate` is not a feature attach point (features
    /// write blocks; climate is column-level). `ResolveBlock` needs no window
    /// and no simulation scope — it must work on worldgen instances.
    #[test]
    fn gen_registrations_gate_on_the_init_window() {
        let mut data = ModStoreData::new("alpha", 1);
        assert_eq!(
            handle_host_call(
                &mut data,
                HostCall::RegisterWorldgenFeature {
                    feature_id: 1,
                    stage: mod_api::WorldgenStage::Trees,
                },
            ),
            HostRet::Unit
        );
        assert!(matches!(
            handle_host_call(
                &mut data,
                HostCall::RegisterWorldgenFeature {
                    feature_id: 2,
                    stage: mod_api::WorldgenStage::Climate,
                },
            ),
            HostRet::Error(_)
        ));
        assert_eq!(
            handle_host_call(
                &mut data,
                HostCall::RegisterStageReplacement {
                    stage: mod_api::WorldgenStage::Terrain,
                    callback_id: 3,
                },
            ),
            HostRet::Unit
        );
        assert_eq!(
            handle_host_call(&mut data, HostCall::RegisterGenerator { callback_id: 4 }),
            HostRet::Unit
        );
        assert_eq!(data.stats.registered, 3);
        assert!(data.pending.iter().all(Registration::is_gen));

        // Outside the window every gen registration is rejected...
        data.phase = Phase::Run;
        for call in [
            HostCall::RegisterWorldgenFeature {
                feature_id: 1,
                stage: mod_api::WorldgenStage::Trees,
            },
            HostCall::RegisterStageReplacement {
                stage: mod_api::WorldgenStage::Terrain,
                callback_id: 3,
            },
            HostCall::RegisterGenerator { callback_id: 4 },
        ] {
            assert!(matches!(
                handle_host_call(&mut data, call),
                HostRet::Error(_)
            ));
        }
        assert_eq!(data.stats.rejected_registrations, 3);
        // ...but ResolveBlock works anywhere, with no SimCtx published.
        assert_eq!(
            handle_host_call(
                &mut data,
                HostCall::ResolveBlock {
                    key: "petramond:air".into()
                },
            ),
            HostRet::Block(Some(mod_api::BlockId(0)))
        );
        assert_eq!(
            handle_host_call(
                &mut data,
                HostCall::ResolveBlock {
                    key: "no_such:block".into(),
                },
            ),
            HostRet::Block(None)
        );
    }

    /// `EmitSound` feeds the NON-lossy tick queue (never audio directly) and
    /// an unknown key reports failure without disabling anything.
    #[test]
    fn emit_sound_rides_the_tick_feed() {
        let mut data = ModStoreData::new("alpha", 1);
        let mut world = World::new(1, 1);
        let mut player = Player::new(Vec3::new(0.0, 80.0, 0.0));
        let mut feed = TickEvents::default();
        let mut queue = PostQueue::default();
        let mut gui = crate::gui::empty_gui_state();
        let mut ctx = SimCtx {
            world: &mut world,
            player: &mut player,
            gui_state: &mut gui,
            feed: &mut feed,
            queue: &mut queue,
        };
        scope::enter(&mut ctx, || {
            assert_eq!(
                handle_host_call(
                    &mut data,
                    HostCall::EmitSound {
                        key: "petramond:item_pickup".into(),
                        pos: Some([1.0, 64.0, 1.0]),
                    },
                ),
                HostRet::Bool(true)
            );
            assert_eq!(
                handle_host_call(
                    &mut data,
                    HostCall::EmitSound {
                        key: "no_such:sound".into(),
                        pos: None,
                    },
                ),
                HostRet::Bool(false)
            );
        });
        assert_eq!(feed.world.sounds.len(), 1, "one resolved sound queued");
        assert_eq!(feed.world.sounds[0].pos, Some(Vec3::new(1.0, 64.0, 1.0)));
    }

    #[test]
    fn spawn_mob_initializes_cached_light_before_first_render_snapshot() {
        let mut data = ModStoreData::new("alpha", 1);
        let mut world = World::new(1, 1);
        world.insert_empty_column_for_test(ChunkPos::new(0, 0));
        let section = world
            .section_at_world_mut_for_test(8, 64, 8)
            .expect("fixture loads the spawn section");
        section.set_skylight(vec![0; SECTION_VOLUME].into());
        section.set_blocklight(vec![0; SECTION_VOLUME].into());

        let mut player = Player::new(Vec3::new(0.0, 80.0, 0.0));
        let mut feed = TickEvents::default();
        let mut queue = PostQueue::default();
        let mut gui = crate::gui::empty_gui_state();
        let mut ctx = SimCtx {
            world: &mut world,
            player: &mut player,
            gui_state: &mut gui,
            feed: &mut feed,
            queue: &mut queue,
        };
        scope::enter(&mut ctx, || {
            assert_eq!(
                handle_host_call(
                    &mut data,
                    HostCall::SpawnMob {
                        key: "petramond:owl".into(),
                        pos: [8.5, 64.0, 8.5],
                        yaw: 0.0,
                    },
                ),
                HostRet::Bool(true)
            );
        });

        let mob = &world.mobs().instances()[0];
        assert_eq!(mob.skylight, 0);
        assert_eq!(mob.blocklight, 0);
    }

    #[test]
    fn spatial_sound_calls_queue_resolved_commands_with_deterministic_handles() {
        fn run_once() -> (u64, u64, Vec<crate::game::ModSpatialSoundCommand>) {
            let mut data = ModStoreData::new("alpha", 1);
            let mut world = World::new(1, 1);
            assert!(world
                .mobs_mut()
                .spawn(crate::mob::Mob::Owl, Vec3::new(2.0, 80.0, 3.0), 0.0));
            let mob_id = world.mobs().instances()[0].id();
            let mut player = Player::new(Vec3::new(0.0, 80.0, 0.0));
            let mut feed = TickEvents::default();
            let mut queue = PostQueue::default();
            let mut gui = crate::gui::empty_gui_state();
            let mut ctx = SimCtx {
                world: &mut world,
                player: &mut player,
                gui_state: &mut gui,
                feed: &mut feed,
                queue: &mut queue,
            };
            let mut handles = (0, 0);
            scope::enter(&mut ctx, || {
                handles.0 = match handle_host_call(
                    &mut data,
                    HostCall::SoundPlayAt {
                        key: "petramond:item_pickup".into(),
                        pos: [1.0, 81.0, 1.0],
                        volume: 0.5,
                        pitch: 1.25,
                    },
                ) {
                    HostRet::U64(handle) => handle,
                    other => panic!("SoundPlayAt returned {other:?}"),
                };
                handles.1 = match handle_host_call(
                    &mut data,
                    HostCall::SoundPlayOnMob {
                        mob_id,
                        key: "petramond:item_pickup".into(),
                        volume: 0.75,
                        pitch: 0.9,
                    },
                ) {
                    HostRet::U64(handle) => handle,
                    other => panic!("SoundPlayOnMob returned {other:?}"),
                };
                assert_eq!(
                    handle_host_call(&mut data, HostCall::SoundStop { handle: handles.0 }),
                    HostRet::Unit
                );
                assert_eq!(
                    handle_host_call(
                        &mut data,
                        HostCall::SoundPlayAt {
                            key: "no_such:sound".into(),
                            pos: [0.0, 0.0, 0.0],
                            volume: 1.0,
                            pitch: 1.0,
                        },
                    ),
                    HostRet::U64(0),
                    "unknown sounds do not allocate handles"
                );
            });
            (handles.0, handles.1, feed.world.spatial_sounds)
        }

        let first = run_once();
        let second = run_once();
        assert_ne!(first.0, 0);
        assert_ne!(first.0, first.1, "two starts get distinct handles");
        assert_eq!(
            first, second,
            "same session inputs produce the same handles"
        );

        let sound =
            crate::audio::sound_by_name("petramond:item_pickup").expect("engine sound exists");
        assert_eq!(first.2.len(), 3);
        assert_eq!(
            first.2[0],
            crate::game::ModSpatialSoundCommand::PlayAt {
                handle: first.0,
                sound,
                pos: Vec3::new(1.0, 81.0, 1.0),
                volume: 0.5,
                pitch: 1.25,
            }
        );
        match first.2[1] {
            crate::game::ModSpatialSoundCommand::PlayOnMob {
                handle,
                sound: queued_sound,
                mob_id,
                volume,
                pitch,
                last_pos,
            } => {
                assert_eq!(handle, first.1);
                assert_eq!(queued_sound, sound);
                assert_ne!(mob_id, 0);
                assert_eq!(volume, 0.75);
                assert_eq!(pitch, 0.9);
                assert_eq!(last_pos, Vec3::new(2.0, 80.0, 3.0));
            }
            other => panic!("expected mob-pinned sound command, got {other:?}"),
        }
        assert_eq!(
            first.2[2],
            crate::game::ModSpatialSoundCommand::Stop { handle: first.0 }
        );
    }

    #[test]
    fn mob_snapshot_id_survives_unrelated_despawn_index_shift() {
        let mut data = ModStoreData::new("alpha", 1);
        let mut world = World::new(1, 1);
        assert!(world
            .mobs_mut()
            .spawn(crate::mob::Mob::Owl, Vec3::new(1.0, 80.0, 1.0), 0.0));
        assert!(world
            .mobs_mut()
            .spawn(crate::mob::Mob::Owl, Vec3::new(2.0, 80.0, 2.0), 0.0));
        let mut player = Player::new(Vec3::new(0.0, 80.0, 0.0));
        let mut feed = TickEvents::default();
        let mut queue = PostQueue::default();
        let mut gui = crate::gui::empty_gui_state();
        let mut ctx = SimCtx {
            world: &mut world,
            player: &mut player,
            gui_state: &mut gui,
            feed: &mut feed,
            queue: &mut queue,
        };

        scope::enter(&mut ctx, || {
            let before = match handle_host_call(
                &mut data,
                HostCall::MobsInRadius {
                    pos: [0.0, 80.0, 0.0],
                    radius: 10.0,
                },
            ) {
                HostRet::Mobs(mobs) => mobs,
                other => panic!("MobsInRadius returned {other:?}"),
            };
            assert_eq!(before.len(), 2);
            let shifted_id = before[1].id;
            assert_ne!(before[0].id, shifted_id);

            assert_eq!(
                handle_host_call(
                    &mut data,
                    HostCall::DespawnMob {
                        index: before[0].index
                    }
                ),
                HostRet::Bool(true)
            );

            let after = match handle_host_call(
                &mut data,
                HostCall::MobsInRadius {
                    pos: [0.0, 80.0, 0.0],
                    radius: 10.0,
                },
            ) {
                HostRet::Mobs(mobs) => mobs,
                other => panic!("MobsInRadius returned {other:?}"),
            };
            assert_eq!(after.len(), 1);
            assert_eq!(after[0].index, 0, "swap_remove shifted the remaining mob");
            assert_eq!(after[0].id, shifted_id, "stable id survived the shift");
            assert_eq!(after[0].pos, [2.0, 80.0, 2.0]);
        });
    }
}
