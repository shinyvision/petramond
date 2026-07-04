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

mod convert;
pub(crate) mod gen;
mod host;
mod instance;
pub(crate) mod manifest;
pub(crate) mod modset;
mod scope;

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use mod_api::{EventKind, EventPayload, GuestCall, GuestRet};

use crate::events::{EventBus, Outcome, SimCtx, TickSystems};
use crate::game::TickEvents;
use crate::player::Player;
use crate::world::World;

use host::Registration;
use instance::ModInstance;

type SharedInstance = Rc<RefCell<ModInstance>>;

/// A loaded mod's identity + compiled module (kept for the worldgen hook
/// config, whose per-thread instances re-instantiate from it).
struct ModMeta {
    id: String,
    /// `None` only for test-injected instances (no module handle available);
    /// such mods cannot register worldgen hooks.
    module: Option<wasmtime::Module>,
}

/// Every loaded mod instance, in pack load order.
pub(crate) struct ModHost {
    instances: Vec<SharedInstance>,
    /// Parallel to `instances`.
    metas: Vec<ModMeta>,
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
                    instances.push(Rc::new(RefCell::new(inst)));
                    metas.push(ModMeta {
                        id: id.clone(),
                        module: Some(module),
                    });
                }
                Err(e) => log::error!("mod '{id}' disabled for this session: {e}"),
            }
        }
        Self { instances, metas }
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
            instances: vec![Rc::new(RefCell::new(inst))],
            metas: vec![ModMeta {
                id: mod_id.to_owned(),
                module: Some(module),
            }],
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
                .map(|i| Rc::new(RefCell::new(i)))
                .collect(),
            metas,
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
        bus: &mut EventBus,
        systems: &mut TickSystems,
        next_spatial_sound_handle: &mut u64,
    ) {
        let mut gen_hooks = gen::GenHooksBuilder::new(world.seed);
        for (shared, meta) in self.instances.iter().zip(&self.metas) {
            // Init runs outside any tick; give host calls a real context
            // anyway (CurrentTick is legal during init) via a scratch feed.
            let mut feed = TickEvents::with_next_spatial_sound_handle(*next_spatial_sound_handle);
            let registrations = {
                let mut inst = shared.borrow_mut();
                let mut ctx = SimCtx {
                    world: &mut *world,
                    player: &mut *player,
                    feed: &mut feed,
                    queue: bus.queue_mut(),
                };
                inst.call_init(&mut ctx);
                inst.take_registrations()
            };
            *next_spatial_sound_handle = feed.next_spatial_sound_handle();
            for registration in registrations {
                if registration.is_gen() {
                    match &meta.module {
                        Some(module) => gen_hooks.add_registration(&meta.id, module, &registration),
                        None => log::error!(
                            "mod '{}': worldgen hooks need a compiled module handle; \
                             registration dropped",
                            meta.id
                        ),
                    }
                } else {
                    apply_registration(shared, registration, bus, systems);
                }
            }
        }
        gen::install(gen_hooks.build());
    }

    /// Dispatch a GUI button click to the OWNING mod — the pack whose
    /// namespace `kind_key` carries (Phase 5). Runs on the tick, from the
    /// menu stage's click drain. Engine kinds carry the reserved `llama`
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
        self.instances[i].borrow_mut().call_guest(ctx, &call);
    }

    #[cfg(test)]
    pub(crate) fn loaded(&self) -> usize {
        self.instances.len()
    }

    /// Test observability for mod `index`: (disabled, successful guest
    /// dispatches, host-call stats).
    #[cfg(test)]
    pub(crate) fn probe(&self, index: usize) -> (bool, u64, host::HostStats) {
        let inst = self.instances[index].borrow();
        (inst.disabled(), inst.dispatches(), inst.stats())
    }
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
            let inst = Rc::clone(shared);
            systems.attach(convert::attach(stage, attach), priority, move |ctx| {
                let call = GuestCall::TickSystem { id: system_id };
                inst.borrow_mut().call_guest(ctx, &call);
            });
        }
        Registration::EventHandler {
            event,
            priority,
            handler_id,
        } => wire_event_handler(shared, event, priority, handler_id, bus),
        // Gen registrations go to the worldgen hook config in `initialize`,
        // never to the bus/scheduler.
        Registration::WorldgenFeature { .. }
        | Registration::StageReplacement { .. }
        | Registration::Generator { .. } => {
            unreachable!("gen registrations are routed to GenHooksBuilder")
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
    match inst.borrow_mut().call_guest(ctx, &call)? {
        GuestRet::Event { outcome, payload } => Some((outcome, payload)),
        _ => {
            inst.borrow_mut()
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
        let inst = Rc::clone(shared);
        bus.on_post(kind, priority, move |ctx, ev| {
            call_event(&inst, ctx, handler_id, convert::post_event(ev));
        });
        return;
    }
    // Pre kinds: each needs its typed bus slot, and only the fields the
    // taxonomy marks mutable are read back from the echoed payload.
    let inst = Rc::clone(shared);
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
        EventKind::MobHurtPre => bus.on_mob_hurt_pre(priority, move |ctx, ev| {
            match call_event(&inst, ctx, handler_id, convert::mob_hurt_pre(ev)) {
                Some((outcome, echoed)) => {
                    if let EventPayload::MobHurtPre { amount, .. } = echoed {
                        ev.amount = amount;
                    }
                    convert::outcome(outcome)
                }
                None => Outcome::Continue,
            }
        }),
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

#[cfg(test)]
pub(crate) mod tests;
