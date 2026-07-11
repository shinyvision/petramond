//! WASM mod host — modding system Phase 2b (see WIKI/modding.md).
//!
//! Loads each pack's `mod.wasm` (core module, built by `make mods` from
//! `mods-src/`), runs its `mod_init` registration window, and wires the
//! registered tick systems / event handlers into the Phase 1 seams
//! ([`crate::events`]) as closures that postcard-dispatch into the guest.
//!
//! Ownership: `Game` owns the [`ModHost`]; each registered closure holds an
//! `Rc` of its own mod's instance (main-thread only, like the bus itself), so
//! disabling a mod is one flag — its closures turn into no-ops. Dispatch order
//! is the bus contract, `(priority, registration order)`: engine handlers
//! register first (none yet), then mods in load order, then each mod's own
//! registrations in the order its `mod_init` issued them.
//!
//! Determinism: guests get NaN-canonicalized floats, no clock/entropy imports,
//! seeded RNG streams, and the tick counter — see the contract section of the
//! wiki page. A trapping / deadline-blowing / protocol-breaking mod is
//! disabled for the session with a visible error and the tick continues.

pub(crate) mod ai;
pub(crate) mod client;
mod convert;
pub(crate) mod gen;
mod host;
mod instance;
pub(crate) mod manifest;
pub(crate) mod modset;
mod scope;

/// The session's loaded recipe catalog, shared with the host so `RecipeResult`
/// answers from the exact table the engine cooks from (same process-wide
/// install pattern as [`gen`]; replaced by each `Game::new`).
static ACTIVE_RECIPES: std::sync::RwLock<Option<std::sync::Arc<crate::crafting::Recipes>>> =
    std::sync::RwLock::new(None);

/// Install the session's recipe snapshot (called by `Game::new`).
pub(crate) fn install_recipes(recipes: std::sync::Arc<crate::crafting::Recipes>) {
    *ACTIVE_RECIPES.write().unwrap() = Some(recipes);
}

/// The installed recipe snapshot, if a session has published one.
pub(crate) fn active_recipes() -> Option<std::sync::Arc<crate::crafting::Recipes>> {
    ACTIVE_RECIPES.read().unwrap().clone()
}

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use mod_api::{EventKind, EventPayload, GuestCall, GuestRet, HostileSpawnCandidate};

use crate::events::{
    EventBus, MobDamageFeedback, MobDamageFeedbackComponent, MobDamageSound, Outcome, SimCtx,
    TickSystems,
};
use crate::game::TickEvents;
use crate::mathh::IVec3;
use crate::mob::{Mob, MobCategory};
use crate::player::Player;
use crate::world::World;

pub(crate) use client::{ClientCommand, ClientImageData, ClientOverlayRegistration};
use host::Registration;
use instance::ModInstance;

// `Arc<Mutex<…>>` (not `Rc<RefCell<…>>`) because the whole `ServerGame` —
// including every registration's shared instance handle — moves to the server
// thread (multiplayer Phase D); the mutex is uncontended (one sim thread).
type SharedInstance = Arc<Mutex<ModInstance>>;

/// A loaded mod's identity + compiled module (kept for the worldgen hook
/// config, whose per-thread instances re-instantiate from it).
struct ModMeta {
    id: String,
    /// `None` only for test-injected instances (no module handle available);
    /// such mods cannot register worldgen hooks.
    module: Option<wasmtime::Module>,
}

struct HostileSpawnerRegistration {
    instance: SharedInstance,
    priority: i32,
    callback_id: u32,
    order: usize,
}

struct BlockBehaviorRegistration {
    instance: SharedInstance,
    callback_id: u32,
}

/// Every loaded mod instance, in pack load order.
pub(crate) struct ModHost {
    instances: Vec<SharedInstance>,
    /// Parallel to `instances`.
    metas: Vec<ModMeta>,
    hostile_spawners: Vec<HostileSpawnerRegistration>,
    /// `blocks.json` `behavior` key (`mod_id:name`) → the owning mod's
    /// handler, for routing [`ModBlockHook`](crate::block::behavior::ModBlockHook)s.
    block_behaviors: std::collections::HashMap<String, BlockBehaviorRegistration>,
    /// The session's scripted AI-node registrations, retained so the server
    /// thread can install them into ITS thread-local dispatch registry
    /// ([`install_thread_ai_nodes`](Self::install_thread_ai_nodes)) — the
    /// registry is per-thread (see `ai.rs`), and `initialize` runs on the
    /// constructing thread.
    ai_nodes: std::collections::HashMap<String, ai::AiNodeRegistration>,
}

impl ModHost {
    /// Load the wasm module of every discovered pack that ships one
    /// (`assets::packs()` order = load order), except packs the world's
    /// settings disable: a disabled pack's wasm never instantiates, so it
    /// contributes no tick systems, event handlers, worldgen hooks, or GUI
    /// click ownership for this session. (Its catalog CONTENT stays in the
    /// process-wide registries — only reachability is gated; the save palette
    /// makes its world content decode to air.)
    pub(crate) fn load(world_seed: u32, disabled: &std::collections::BTreeSet<String>) -> Self {
        let mods = session_wasm_mods(crate::assets::packs(), disabled);
        Self::from_wasm_list(world_seed, &mods)
    }

    /// Load explicit `(mod id, wasm path)` pairs — the pack-independent entry
    /// tests use. A module that fails to compile/instantiate is skipped with a
    /// logged error (= disabled at load).
    pub(crate) fn from_wasm_list(world_seed: u32, mods: &[(String, PathBuf)]) -> Self {
        let mut instances = Vec::new();
        let mut metas = Vec::new();
        for (id, wasm) in mods {
            let module = match host::module_for(wasm) {
                Ok(module) => module,
                Err(e) => {
                    log::error!("mod '{id}' disabled for this session: {e}");
                    continue;
                }
            };
            match ModInstance::from_module(id, &module, world_seed) {
                Ok(inst) => {
                    log::info!("mod '{id}' loaded from {}", wasm.display());
                    instances.push(Arc::new(Mutex::new(inst)));
                    metas.push(ModMeta {
                        id: id.clone(),
                        module: Some(module),
                    });
                }
                Err(e) => log::error!("mod '{id}' disabled for this session: {e}"),
            }
        }
        Self {
            instances,
            metas,
            hostile_spawners: Vec::new(),
            block_behaviors: std::collections::HashMap::new(),
            ai_nodes: std::collections::HashMap::new(),
        }
    }

    /// Test helper: a host with one WAT guest registered under `mod_id` whose
    /// `mod_dispatch` answers `GuestRet::Unit` (postcard `[0]` staged at 512)
    /// to everything — for driving engine-side dispatch plumbing (the GUI
    /// click drain) without a compiled mod.
    #[cfg(test)]
    pub(crate) fn test_unit_guest_host(mod_id: &str) -> Self {
        let wat = r#"(module
  (memory (export "memory") 1)
  (data (i32.const 512) "\00")
  (func (export "mod_init"))
  (func (export "mod_alloc") (param i32) (result i32) (i32.const 4096))
  (func (export "mod_free") (param i32 i32))
  (func (export "mod_dispatch") (param i32 i32) (result i64)
    (i64.const 2199023255553)))"#;
        assert_eq!(mod_api::pack_ptr_len(512, 1), 2199023255553);
        let module =
            wasmtime::Module::new(host::engine(), wat.as_bytes()).expect("assemble unit guest");
        let inst = ModInstance::from_module(mod_id, &module, 1).expect("instantiate unit guest");
        Self {
            instances: vec![Arc::new(Mutex::new(inst))],
            metas: vec![ModMeta {
                id: mod_id.to_owned(),
                module: Some(module),
            }],
            hostile_spawners: Vec::new(),
            block_behaviors: std::collections::HashMap::new(),
            ai_nodes: std::collections::HashMap::new(),
        }
    }

    /// Test entry: adopt pre-built instances (e.g. WAT-built hostile guests).
    #[cfg(test)]
    fn from_instances(instances: Vec<ModInstance>) -> Self {
        let metas = instances
            .iter()
            .map(|_| ModMeta {
                id: "hostile".into(),
                module: None,
            })
            .collect();
        Self {
            instances: instances
                .into_iter()
                .map(|i| Arc::new(Mutex::new(i)))
                .collect(),
            metas,
            hostile_spawners: Vec::new(),
            block_behaviors: std::collections::HashMap::new(),
            ai_nodes: std::collections::HashMap::new(),
        }
    }

    /// Run every mod's `mod_init` (its one registration window), wire the
    /// collected registrations into the bus/scheduler, and install the
    /// session's worldgen hook config (empty or not — installing always, with
    /// a fresh epoch, is what evicts a previous session's config). Call once,
    /// after the engine's own handlers (if any) have registered, so mods sort
    /// behind them at equal priority.
    pub(crate) fn initialize(
        &mut self,
        world: &mut World,
        player: &mut Player,
        gui_state: &mut std::sync::Arc<crate::gui::GuiStateMap>,
        bus: &mut EventBus,
        systems: &mut TickSystems,
        next_spatial_sound_handle: &mut u64,
    ) {
        let mut gen_hooks = gen::GenHooksBuilder::new(world.seed);
        let mut ai_nodes: std::collections::HashMap<String, ai::AiNodeRegistration> =
            std::collections::HashMap::new();
        let mut hostile_order = self.hostile_spawners.len();
        for (shared, meta) in self.instances.iter().zip(&self.metas) {
            // Init runs outside any tick; give host calls a real context
            // anyway (CurrentTick is legal during init) via a scratch feed.
            let mut feed = TickEvents::with_next_spatial_sound_handle(*next_spatial_sound_handle);
            let registrations = {
                let mut inst = shared.lock().unwrap();
                let mut ctx = SimCtx {
                    world: &mut *world,
                    player: &mut *player,
                    gui_state: &mut *gui_state,
                    feed: &mut feed,
                    queue: bus.queue_mut(),
                };
                inst.call_init(&mut ctx);
                inst.take_registrations()
            };
            *next_spatial_sound_handle = feed.next_spatial_sound_handle();
            for registration in registrations {
                match registration {
                    Registration::HostileSpawner {
                        priority,
                        callback_id,
                    } => {
                        self.hostile_spawners.push(HostileSpawnerRegistration {
                            instance: Arc::clone(shared),
                            priority,
                            callback_id,
                            order: hostile_order,
                        });
                        hostile_order += 1;
                    }
                    Registration::BlockBehavior { key, callback_id } => {
                        // Keys are namespace-validated at the host call; a
                        // duplicate within one pack is a mod bug — last wins.
                        if self
                            .block_behaviors
                            .insert(
                                key.clone(),
                                BlockBehaviorRegistration {
                                    instance: Arc::clone(shared),
                                    callback_id,
                                },
                            )
                            .is_some()
                        {
                            log::warn!(
                                "mod '{}': block behavior '{key}' registered twice; \
                                 the later registration wins",
                                meta.id
                            );
                        }
                    }
                    Registration::AiNode { key, callback_id } => {
                        if ai_nodes
                            .insert(
                                key.clone(),
                                ai::AiNodeRegistration {
                                    instance: Arc::clone(shared),
                                    callback_id,
                                },
                            )
                            .is_some()
                        {
                            log::warn!(
                                "mod '{}': AI node '{key}' registered twice; \
                                 the later registration wins",
                                meta.id
                            );
                        }
                    }
                    other if other.is_gen() => match &meta.module {
                        Some(module) => gen_hooks.add_registration(&meta.id, module, &other),
                        None => log::error!(
                            "mod '{}': worldgen hooks need a compiled module handle; \
                             registration dropped",
                            meta.id
                        ),
                    },
                    other => {
                        apply_registration(shared, other, bus, systems);
                    }
                }
            }
        }
        self.hostile_spawners.sort_by_key(|r| (r.priority, r.order));
        gen::install(gen_hooks.build());
        ai::install(ai_nodes.clone());
        self.ai_nodes = ai_nodes;
    }

    /// Install the session's scripted AI-node map into THIS thread's dispatch
    /// registry. The server thread calls it once at startup (multiplayer
    /// Phase D): `initialize` installed on the constructing thread, and the
    /// registry is deliberately thread-local (test isolation — see `ai.rs`).
    pub(crate) fn install_thread_ai_nodes(&self) {
        ai::install(self.ai_nodes.clone());
    }

    /// Dispatch a GUI button click to the OWNING mod — the pack whose
    /// namespace `kind_key` carries (Phase 5). Runs on the tick, from the
    /// menu stage's click drain. Engine kinds carry the reserved `petramond`
    /// namespace but are not mod kinds, and a content-only pack may ship a GUI
    /// with no wasm: both simply dispatch nothing.
    pub(crate) fn dispatch_gui_click(
        &mut self,
        ctx: &mut SimCtx,
        kind_key: &str,
        widget_id: &str,
        pos: Option<[i32; 3]>,
    ) {
        let Some((owner, _)) = kind_key.split_once(':') else {
            return;
        };
        let Some(i) = self.metas.iter().position(|m| m.id == owner) else {
            return;
        };
        let call = GuestCall::GuiClick {
            kind_key: kind_key.to_owned(),
            widget_id: widget_id.to_owned(),
            pos,
        };
        self.instances[i].lock().unwrap().call_guest(ctx, &call);
    }

    pub(crate) fn has_hostile_spawners(&self) -> bool {
        !self.hostile_spawners.is_empty()
    }

    pub(crate) fn has_block_behaviors(&self) -> bool {
        !self.block_behaviors.is_empty()
    }

    /// Forward the world's queued mod-behavior hooks (drained after its
    /// scheduled/random ticks, in fire order) to the mods that registered
    /// their keys. A hook whose key no mod registered is dropped silently —
    /// the block stays inert, exactly like a row pointing at a disabled
    /// pack's behavior.
    pub(crate) fn dispatch_block_hooks(
        &self,
        ctx: &mut SimCtx,
        hooks: &[crate::block::behavior::ModBlockHook],
    ) {
        for hook in hooks {
            let Some(reg) = self.block_behaviors.get(hook.key) else {
                continue;
            };
            let call = GuestCall::BlockBehavior {
                callback_id: reg.callback_id,
                kind: hook.kind,
                pos: [hook.pos.x, hook.pos.y, hook.pos.z],
            };
            let reply = reg.instance.lock().unwrap().call_guest(ctx, &call);
            match reply {
                None | Some(GuestRet::Unit) => {}
                Some(_) => reg
                    .instance
                    .lock()
                    .unwrap()
                    .disable("returned a non-unit reply to a block behavior dispatch"),
            }
        }
    }

    pub(crate) fn hostile_spawn_kind(
        &self,
        ctx: &mut SimCtx,
        candidate: &HostileSpawnCandidate,
    ) -> Option<Mob> {
        for spawner in &self.hostile_spawners {
            let call = GuestCall::HostileSpawnCandidate {
                callback_id: spawner.callback_id,
                candidate: candidate.clone(),
            };
            let reply = {
                let mut inst = spawner.instance.lock().unwrap();
                inst.call_guest(ctx, &call)
            };
            let Some(reply) = reply else {
                continue;
            };
            match reply {
                GuestRet::HostileSpawn(Some(key)) => {
                    if let Some(kind) = hostile_kind_for_key(ctx.world, &key, candidate) {
                        return Some(kind);
                    }
                }
                GuestRet::HostileSpawn(None) => {}
                _ => {
                    spawner
                        .instance
                        .lock()
                        .unwrap()
                        .disable("returned a non-hostile-spawn reply to a hostile spawn dispatch");
                }
            }
        }
        None
    }

    #[cfg(test)]
    pub(crate) fn loaded(&self) -> usize {
        self.instances.len()
    }

    /// Test observability for mod `index`: (disabled, successful guest
    /// dispatches, host-call stats).
    #[cfg(test)]
    pub(crate) fn probe(&self, index: usize) -> (bool, u64, host::HostStats) {
        let inst = self.instances[index].lock().unwrap();
        (inst.disabled(), inst.dispatches(), inst.stats())
    }
}

fn hostile_kind_for_key(
    world: &World,
    key: &str,
    candidate: &HostileSpawnCandidate,
) -> Option<Mob> {
    let kind = crate::mob::defs()
        .iter()
        .position(|d| d.key == key)
        .map(|i| Mob(i as u8))?;
    let def = crate::mob::def(kind);
    if def.category != MobCategory::Hostile {
        return None;
    }
    if crate::registry::namespace(def.name).is_some_and(|ns| world.disabled_mods().contains(ns)) {
        return None;
    }
    let feet = IVec3::new(candidate.cell[0], candidate.cell[1], candidate.cell[2]);
    crate::mob::spawn_body_fits_at(world, kind, feet).then_some(kind)
}

/// The `(mod id, wasm path)` pairs a session instantiates: every id-bearing
/// pack that ships wasm, minus the world's disabled set. Pure — the enabled-
/// set filtering contract, unit-tested against synthetic pack lists.
fn session_wasm_mods(
    packs: &[crate::assets::Pack],
    disabled: &std::collections::BTreeSet<String>,
) -> Vec<(String, PathBuf)> {
    packs
        .iter()
        .filter_map(|p| {
            let id = p.id.clone()?;
            let wasm = p.wasm.clone()?;
            if disabled.contains(&id) {
                log::info!("mod '{id}' is disabled for this world (settings.json); not loading");
                return None;
            }
            Some((id, wasm))
        })
        .collect()
}

/// Wire one collected registration into the engine seam it targets, as a
/// closure dispatching into `shared`'s guest.
fn apply_registration(
    shared: &SharedInstance,
    registration: Registration,
    bus: &mut EventBus,
    systems: &mut TickSystems,
) {
    match registration {
        Registration::TickSystem {
            stage,
            attach,
            priority,
            system_id,
        } => {
            let inst = Arc::clone(shared);
            systems.attach(convert::attach(stage, attach), priority, move |ctx| {
                let call = GuestCall::TickSystem { id: system_id };
                inst.lock().unwrap().call_guest(ctx, &call);
            });
        }
        Registration::EventHandler {
            event,
            priority,
            handler_id,
        } => wire_event_handler(shared, event, priority, handler_id, bus),
        // Gen/spawner/behavior registrations go to their own registries in
        // `initialize`, never to the bus/scheduler.
        Registration::WorldgenFeature { .. }
        | Registration::StageReplacement { .. }
        | Registration::Generator { .. }
        | Registration::HostileSpawner { .. }
        | Registration::BlockBehavior { .. }
        | Registration::AiNode { .. } => {
            unreachable!("non-system registrations are routed during ModHost::initialize")
        }
    }
}

/// Dispatch one event to the guest handler and return its verdict + echoed
/// payload. `None` = mod disabled (now or earlier): the event proceeds as if
/// unhandled. A reply of the wrong shape is a protocol break and disables the
/// mod like any trap.
fn call_event(
    inst: &SharedInstance,
    ctx: &mut SimCtx,
    handler_id: u32,
    payload: EventPayload,
) -> Option<(mod_api::Outcome, EventPayload)> {
    let call = GuestCall::HandleEvent {
        id: handler_id,
        kind: payload.kind(),
        payload,
    };
    match inst.lock().unwrap().call_guest(ctx, &call)? {
        GuestRet::Event { outcome, payload } => Some((outcome, payload)),
        _ => {
            inst.lock()
                .unwrap()
                .disable("returned a non-event reply to an event dispatch");
            None
        }
    }
}

fn wire_event_handler(
    shared: &SharedInstance,
    event: EventKind,
    priority: i32,
    handler_id: u32,
    bus: &mut EventBus,
) {
    // Post kinds: observe-only, one generic wrapper.
    if let Some(kind) = convert::post_kind(event) {
        let inst = Arc::clone(shared);
        bus.on_post(kind, priority, move |ctx, ev| {
            call_event(&inst, ctx, handler_id, convert::post_event(ev));
        });
        return;
    }
    // Pre kinds: each needs its typed bus slot, and only the fields the
    // taxonomy marks mutable are read back from the echoed payload.
    let inst = Arc::clone(shared);
    match event {
        EventKind::BlockPlacePre => {
            bus.on_block_place_pre(priority, move |ctx, ev| {
                match call_event(&inst, ctx, handler_id, convert::block_place_pre(ev)) {
                    Some((outcome, _)) => convert::outcome(outcome),
                    None => Outcome::Continue,
                }
            })
        }
        EventKind::BlockBreakPre => {
            bus.on_block_break_pre(priority, move |ctx, ev| {
                match call_event(&inst, ctx, handler_id, convert::block_break_pre(ev)) {
                    Some((outcome, _)) => convert::outcome(outcome),
                    None => Outcome::Continue,
                }
            })
        }
        EventKind::BlockInteract => {
            bus.on_block_interact(priority, move |ctx, ev| {
                match call_event(&inst, ctx, handler_id, convert::block_interact(ev)) {
                    Some((outcome, _)) => convert::outcome(outcome),
                    None => Outcome::Continue,
                }
            })
        }
        EventKind::ItemUsePre => bus.on_item_use_pre(priority, move |ctx, ev| {
            match call_event(&inst, ctx, handler_id, convert::item_use_pre(ev)) {
                Some((outcome, _)) => convert::outcome(outcome),
                None => Outcome::Continue,
            }
        }),
        EventKind::MobDamagePre => {
            bus.on_mob_damage_pre(priority, move |ctx, ev| {
                match call_event(&inst, ctx, handler_id, convert::mob_damage_pre(ev)) {
                    Some((outcome, echoed)) => {
                        if let EventPayload::MobDamagePre {
                            amount, feedback, ..
                        } = echoed
                        {
                            ev.amount = amount;
                            ev.feedback = mob_damage_feedback(feedback);
                        }
                        convert::outcome(outcome)
                    }
                    None => Outcome::Continue,
                }
            })
        }
        EventKind::PlayerDamagePre => bus.on_player_damage_pre(priority, move |ctx, ev| {
            match call_event(&inst, ctx, handler_id, convert::player_damage_pre(ev)) {
                Some((outcome, echoed)) => {
                    if let EventPayload::PlayerDamagePre { amount, .. } = echoed {
                        ev.amount = amount;
                    }
                    convert::outcome(outcome)
                }
                None => Outcome::Continue,
            }
        }),
        // Handled by the post branch above.
        _ => unreachable!("post kind fell through"),
    }
}

fn mob_damage_feedback(feedback: mod_api::MobDamageFeedback) -> MobDamageFeedback {
    MobDamageFeedback {
        components: feedback
            .components
            .into_iter()
            .map(mob_damage_feedback_component)
            .collect(),
    }
}

fn mob_damage_feedback_component(
    component: mod_api::MobDamageFeedbackComponent,
) -> MobDamageFeedbackComponent {
    match component {
        mod_api::MobDamageFeedbackComponent::DecreaseHealth => {
            MobDamageFeedbackComponent::DecreaseHealth
        }
        mod_api::MobDamageFeedbackComponent::Flash { duration } => {
            MobDamageFeedbackComponent::Flash {
                duration: finite_nonnegative(duration, 0.0),
            }
        }
        mod_api::MobDamageFeedbackComponent::Knockback { scale, duration } => {
            MobDamageFeedbackComponent::Knockback {
                scale: finite_nonnegative(scale, 0.0).clamp(0.0, 8.0),
                duration: finite_nonnegative(duration, 0.0),
            }
        }
        mod_api::MobDamageFeedbackComponent::Sound { category } => {
            MobDamageFeedbackComponent::Sound {
                category: match category {
                    mod_api::MobDamageSound::Hurt => MobDamageSound::Hurt,
                    mod_api::MobDamageSound::Death => MobDamageSound::Death,
                },
            }
        }
        mod_api::MobDamageFeedbackComponent::Ragdoll => MobDamageFeedbackComponent::Ragdoll,
    }
}

fn finite_nonnegative(value: f32, fallback: f32) -> f32 {
    if value.is_finite() {
        value.max(0.0)
    } else {
        fallback
    }
}

#[cfg(test)]
pub(crate) mod tests;
