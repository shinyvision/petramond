pub mod ai;
pub mod client;
mod convert;
pub mod gen;
mod health;
mod host;
pub(crate) use host::memo::{
    clear_pending_key, has_pending_key, park, sweep_parked, take_pending_key, wait_for_pending,
};
mod instance;
pub use petramond_world::pack_manifest as manifest;
pub mod modset;
mod scope;
mod shape_bake;
mod watchdog;

static ACTIVE_RECIPES: std::sync::RwLock<
    Option<std::sync::Arc<petramond_world::crafting::Recipes>>,
> = std::sync::RwLock::new(None);

pub fn install_recipes(recipes: std::sync::Arc<petramond_world::crafting::Recipes>) {
    *ACTIVE_RECIPES.write().unwrap() = Some(recipes);
}

pub fn active_recipes() -> Option<std::sync::Arc<petramond_world::crafting::Recipes>> {
    ACTIVE_RECIPES.read().unwrap().clone()
}

pub fn prewarm_modules() {
    let spawned = std::thread::Builder::new()
        .name("mod-prewarm".into())
        .spawn(|| {
            let paths: Vec<PathBuf> = petramond_world::assets::packs()
                .iter()
                .flat_map(|p| [p.wasm.clone(), p.client_wasm.clone()])
                .flatten()
                .collect();
            host::module_cache::prewarm(paths);
        });
    if let Err(e) = spawned {
        log::warn!("mod prewarm thread failed to spawn: {e}");
    }
}

pub fn clear_module_cache() {
    host::module_cache::clear();
}

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use mod_api::{EventFilter, EventKind, EventPayload, GuestCall, GuestRet, HostileSpawnCandidate};

use crate::events::tick::TickEvents;
use crate::events::{
    EventBus, MobDamageFeedback, MobDamageFeedbackComponent, MobDamageSound, Outcome, SimCtx,
    TickSystems,
};
use crate::mob::{Mob, MobCategory};
use crate::player::BonePose;
use crate::world::ServerWorld;
use petramond_math::math::IVec3;

pub use client::{
    ClientCanvasSceneData, ClientCommand, ClientImageData, ClientOverlayRegistration,
};
use health::ModHealthBoard;
use host::Registration;
use instance::ModInstance;

type SharedInstance = Arc<Mutex<ModInstance>>;

struct ModMeta {
    id: String,
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

pub struct ModHost {
    instances: Vec<SharedInstance>,
    metas: Vec<ModMeta>,
    hostile_spawners: Vec<HostileSpawnerRegistration>,
    block_behaviors: std::collections::HashMap<String, BlockBehaviorRegistration>,
    ai_nodes: std::collections::HashMap<String, ai::AiNodeRegistration>,
    health: ModHealthBoard,
}

impl ModHost {
    pub fn load(world_seed: u32, disabled: &std::collections::BTreeSet<String>) -> Self {
        let mods = session_wasm_mods(petramond_world::assets::packs(), disabled);
        Self::from_wasm_list(world_seed, &mods)
    }

    pub fn from_wasm_list(world_seed: u32, mods: &[(String, PathBuf)]) -> Self {
        host::module_cache::prewarm(mods.iter().map(|(_, wasm)| wasm.clone()));
        let health = ModHealthBoard::default();
        let mut instances = Vec::new();
        let mut metas = Vec::new();
        for (id, wasm) in mods {
            let module = match host::module_for(wasm) {
                Ok(module) => module,
                Err(e) => {
                    health.health(id).disable(&e);
                    continue;
                }
            };
            match ModInstance::from_module_side(
                id,
                &module,
                world_seed,
                mod_api::RuntimeSide::Server,
                None,
                health.health(id),
            ) {
                Ok(inst) => {
                    log::info!("mod '{id}' loaded from {}", wasm.display());
                    instances.push(Arc::new(Mutex::new(inst)));
                    metas.push(ModMeta {
                        id: id.clone(),
                        module: Some(module),
                    });
                }
                Err(e) => {
                    health.health(id).disable(&e);
                }
            }
        }
        Self {
            instances,
            metas,
            hostile_spawners: Vec::new(),
            block_behaviors: std::collections::HashMap::new(),
            ai_nodes: std::collections::HashMap::new(),
            health,
        }
    }

    pub fn disabled_since(&self, seen: usize) -> Vec<String> {
        self.health.disabled_since(seen)
    }

    pub fn disabled_count(&self) -> usize {
        self.health.disabled_count()
    }

    /// Test helper: one WAT guest under `mod_id`, and its `mod_dispatch` answers `GuestRet::Unit`
    /// to everything. Lets us drive dispatch plumbing like the GUI click drain without a compiled
    /// mod.
    #[cfg(any(test, feature = "test-support"))]
    pub fn test_unit_guest_host(mod_id: &str) -> Self {
        let wat = format!(
            r#"(module
  (memory (export "memory") 1)
  (data (i32.const 512) "\00")
{}  (func (export "mod_init") (param i32 i64))
  (func (export "mod_alloc") (param i32) (result i32) (i32.const 4096))
  (func (export "mod_free") (param i32 i32))
  (func (export "mod_dispatch") (param i32 i32) (result i64)
    (i64.const 2199023255553)))"#,
            instance::wat_abi_exports(mod_api::ABI_VERSION)
        );
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
            health: ModHealthBoard::default(),
        }
    }

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
            health: ModHealthBoard::default(),
        }
    }

    pub fn initialize(
        &mut self,
        world: &mut ServerWorld,
        bus: &mut EventBus,
        systems: &mut TickSystems,
        next_spatial_sound_handle: &mut u64,
    ) {
        let mut gen_hooks = gen::GenHooksBuilder::new(world.data().seed, self.health.clone());
        let mut ai_nodes: std::collections::HashMap<String, ai::AiNodeRegistration> =
            std::collections::HashMap::new();
        let mut hostile_order = self.hostile_spawners.len();
        for (shared, meta) in self.instances.iter().zip(&self.metas) {
            let mut feed = TickEvents::with_next_spatial_sound_handle(*next_spatial_sound_handle);
            let t_init = std::time::Instant::now();
            let registrations = {
                let mut inst = shared.lock().unwrap();
                let mut nobody = crate::events::RosterRefs::empty();
                let mut ctx = SimCtx {
                    world: &mut *world,
                    actor: None,
                    players: &mut nobody,
                    feed: &mut feed,
                    queue: bus.queue_mut(),
                };
                inst.call_init(&mut ctx);
                inst.take_registrations()
            };
            log::debug!(
                target: "petramond::modding::perf",
                "mod '{}' init in {:.1} ms",
                meta.id,
                t_init.elapsed().as_secs_f64() * 1e3
            );
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

    pub fn install_thread_ai_nodes(&self) {
        ai::install(self.ai_nodes.clone());
    }

    pub fn dispatch_gui_click(
        &mut self,
        ctx: &mut SimCtx,
        kind_key: &str,
        widget_id: &str,
        anchor: Option<crate::menu::MenuAnchor>,
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
            at: anchor.map(convert::container_address),
        };
        self.instances[i].lock().unwrap().call_guest(ctx, &call);
    }

    pub fn has_hostile_spawners(&self) -> bool {
        !self.hostile_spawners.is_empty()
    }

    pub fn has_block_behaviors(&self) -> bool {
        !self.block_behaviors.is_empty()
    }

    pub fn dispatch_block_hooks(
        &self,
        ctx: &mut SimCtx,
        hooks: &[petramond_world::block::behavior::BlockHook],
    ) {
        for hook in hooks {
            let Some(reg) = self.block_behaviors.get(hook.key) else {
                continue;
            };
            let call = GuestCall::BlockBehavior {
                callback_id: reg.callback_id,
                kind: hook.kind,
                pos: hook.pos.to_array(),
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

    pub fn bake_custom_shapes(&self, ctx: &mut SimCtx) {
        let cells = ctx.world.drain_custom_bake_dirty();
        if cells.is_empty() {
            return;
        }
        let mut groups: std::collections::BTreeMap<
            (String, u16),
            Vec<super::world::CustomBakeCell>,
        > = std::collections::BTreeMap::new();
        for cell in cells {
            let mod_id = petramond_world::registry::namespace(cell.shape_key)
                .unwrap_or_default()
                .to_owned();
            groups
                .entry((mod_id, cell.shape_kind))
                .or_default()
                .push(cell);
        }
        for ((mod_id, shape_kind), group) in groups {
            let Some(inst) = self.instance_by_id(&mod_id) else {
                continue;
            };
            let positions: Vec<petramond_math::math::IVec3> = group.iter().map(|c| c.pos).collect();
            let inputs: Vec<mod_api::CellInput> =
                group.iter().map(shape_bake::cell_input).collect();
            let call = GuestCall::BakeShapeSim {
                shape_kind,
                cells: inputs,
            };
            let reply = inst.lock().unwrap().call_guest(ctx, &call);
            let Some(GuestRet::BakedSim(baked)) = reply else {
                continue;
            };
            match shape_bake::ingest_sim_bake(&baked, positions.len()) {
                shape_bake::BakeIngest::Apply(cells) => {
                    for (pos, (boxes, aperture)) in positions.iter().zip(cells) {
                        ctx.world.set_custom_bake(*pos, &boxes);
                        ctx.world.set_custom_light_aperture(*pos, aperture);
                    }
                }
                shape_bake::BakeIngest::Fallback => {}
                shape_bake::BakeIngest::Disable(reason) => inst.lock().unwrap().disable(&reason),
            }
        }
    }

    /// Bake the SIM collision boxes a custom shape WOULD have at a
    /// not-yet-placed cell — the placement body-occupancy gate's input. The
    /// bake cache only holds PLACED cells and the row's static collision is
    /// the trapped-bake fallback, so without this the gate would test an
    /// empty box set and let a solid shape trap a body. The bake is a pure
    /// function of the cell input, so this equals what the post-placement
    /// pump caches. `None` = no reachable owner or a declined/invalid reply:
    /// the caller falls back to the row's static collision.
    pub fn bake_placement_sim_boxes(
        &self,
        ctx: &mut SimCtx,
        shape_key: &str,
        shape_kind: u16,
        input: mod_api::CellInput,
    ) -> Option<Vec<petramond_world::block::Aabb>> {
        self.bake_placement_sim_batch(ctx, shape_key, shape_kind, vec![input])?
            .into_iter()
            .next()
    }

    pub(crate) fn bake_placement_sim_batch(
        &self,
        ctx: &mut SimCtx,
        shape_key: &str,
        shape_kind: u16,
        inputs: Vec<mod_api::CellInput>,
    ) -> Option<Vec<Vec<petramond_world::block::Aabb>>> {
        let mod_id = petramond_world::registry::namespace(shape_key)?;
        let inst = self.instance_by_id(mod_id)?;
        let count = inputs.len();
        let call = GuestCall::BakeShapeSim {
            shape_kind,
            cells: inputs,
        };
        let reply = inst.lock().unwrap().call_guest(ctx, &call);
        let Some(GuestRet::BakedSim(baked)) = reply else {
            return None;
        };
        match shape_bake::ingest_sim_bake(&baked, count) {
            shape_bake::BakeIngest::Apply(cells) => {
                Some(cells.into_iter().map(|(boxes, _)| boxes).collect())
            }
            shape_bake::BakeIngest::Fallback => None,
            shape_bake::BakeIngest::Disable(reason) => {
                inst.lock().unwrap().disable(&reason);
                None
            }
        }
    }

    pub fn shape_placement_plan(
        &self,
        ctx: &mut SimCtx,
        shape_key: &str,
        shape_kind: u16,
        block_id: u16,
        inputs: mod_api::PlaceInputsView,
    ) -> Option<mod_api::ShapePlacementResult> {
        let mod_id = petramond_world::registry::namespace(shape_key)?;
        let inst = self.instance_by_id(mod_id)?;
        let call = GuestCall::ShapePlacementPlan {
            shape_kind,
            block_id: mod_api::BlockId(block_id),
            inputs,
        };
        match inst.lock().unwrap().call_guest_read_only(ctx, &call) {
            Some(GuestRet::ShapePlacement(result)) => Some(result),
            _ => None,
        }
    }

    fn instance_by_id(&self, id: &str) -> Option<&SharedInstance> {
        self.metas
            .iter()
            .position(|m| m.id == id)
            .map(|i| &self.instances[i])
    }

    pub fn hostile_spawn_kind(
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

    #[cfg(any(test, feature = "test-support"))]
    pub fn probe(&self, index: usize) -> (bool, u64, host::HostStats) {
        let inst = self.instances[index].lock().unwrap();
        (inst.disabled(), inst.dispatches(), inst.stats())
    }
}

fn hostile_kind_for_key(
    world: &ServerWorld,
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
    if petramond_world::registry::namespace(def.name)
        .is_some_and(|ns| world.data().disabled_mods().contains(ns))
    {
        return None;
    }
    let feet = IVec3::from(candidate.cell);
    crate::mob::spawn_body_fits_at(world, kind, feet).then_some(kind)
}

fn session_wasm_mods(
    packs: &[petramond_world::assets::Pack],
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
            filter,
        } => wire_event_handler(shared, event, priority, handler_id, filter, bus),
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

fn call_event(
    inst: &SharedInstance,
    filter: &EventFilter,
    ctx: &mut SimCtx,
    handler_id: u32,
    payload: EventPayload,
) -> Option<(mod_api::Outcome, Option<EventPayload>)> {
    if !filter.matches(&payload) {
        return None;
    }
    let call = GuestCall::HandleEvent {
        id: handler_id,
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
    filter: EventFilter,
    bus: &mut EventBus,
) {
    if let Some(kind) = convert::post_kind(event) {
        let inst = Arc::clone(shared);
        bus.on_post(kind, priority, move |ctx, ev| {
            call_event(&inst, &filter, ctx, handler_id, convert::post_event(ev));
        });
        return;
    }
    let inst = Arc::clone(shared);
    match event {
        EventKind::BlockPlacePre => {
            bus.on_block_place_pre(priority, move |ctx, ev| {
                match call_event(
                    &inst,
                    &filter,
                    ctx,
                    handler_id,
                    convert::block_place_pre(ev),
                ) {
                    Some((outcome, _)) => convert::outcome(outcome),
                    None => Outcome::Continue,
                }
            })
        }
        EventKind::CellsEditPre => {
            bus.on_cells_edit_pre(priority, move |ctx, ev| {
                match call_event(&inst, &filter, ctx, handler_id, convert::cells_edit_pre(ev)) {
                    Some((outcome, _)) => convert::outcome(outcome),
                    None => Outcome::Continue,
                }
            })
        }
        EventKind::BlockBreakPre => {
            bus.on_block_break_pre(priority, move |ctx, ev| {
                match call_event(
                    &inst,
                    &filter,
                    ctx,
                    handler_id,
                    convert::block_break_pre(ev),
                ) {
                    Some((outcome, echoed)) => {
                        if let Some(EventPayload::BlockBreakPre { drops, .. }) = echoed {
                            ev.drops = drops.map(|stacks| {
                                stacks.iter().filter_map(convert::item_stack_in).collect()
                            });
                        }
                        convert::outcome(outcome)
                    }
                    None => Outcome::Continue,
                }
            })
        }
        EventKind::InteractAttempt => bus.on_interact_attempt(priority, move |ctx, ev| {
            match call_event(
                &inst,
                &filter,
                ctx,
                handler_id,
                convert::interact_attempt(ev),
            ) {
                Some((outcome, _)) => convert::outcome(outcome),
                None => Outcome::Continue,
            }
        }),
        EventKind::UseUnclaimed => {
            bus.on_use_unclaimed(priority, move |ctx, ev| {
                match call_event(&inst, &filter, ctx, handler_id, convert::use_unclaimed(ev)) {
                    Some((outcome, _)) => convert::outcome(outcome),
                    None => Outcome::Continue,
                }
            })
        }
        EventKind::AttackAttempt => {
            bus.on_attack_attempt(priority, move |ctx, ev| {
                match call_event(&inst, &filter, ctx, handler_id, convert::attack_attempt(ev)) {
                    Some((outcome, _)) => convert::outcome(outcome),
                    None => Outcome::Continue,
                }
            })
        }
        EventKind::ItemUsePre => bus.on_item_use_pre(priority, move |ctx, ev| {
            match call_event(&inst, &filter, ctx, handler_id, convert::item_use_pre(ev)) {
                Some((outcome, _)) => convert::outcome(outcome),
                None => Outcome::Continue,
            }
        }),
        EventKind::ProjectileHit => {
            bus.on_projectile_hit(priority, move |ctx, ev| {
                match call_event(&inst, &filter, ctx, handler_id, convert::projectile_hit(ev)) {
                    Some((outcome, echoed)) => {
                        if let Some(EventPayload::ProjectileHit { fate, .. }) = echoed {
                            ev.fate = convert::fate_in(fate);
                        }
                        convert::outcome(outcome)
                    }
                    None => Outcome::Continue,
                }
            })
        }
        EventKind::MobDamagePre => {
            bus.on_mob_damage_pre(priority, move |ctx, ev| {
                match call_event(&inst, &filter, ctx, handler_id, convert::mob_damage_pre(ev)) {
                    Some((outcome, echoed)) => {
                        if let Some(EventPayload::MobDamagePre {
                            amount, feedback, ..
                        }) = echoed
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
            match call_event(
                &inst,
                &filter,
                ctx,
                handler_id,
                convert::player_damage_pre(ev),
            ) {
                Some((outcome, echoed)) => {
                    if let Some(EventPayload::PlayerDamagePre { amount, .. }) = echoed {
                        ev.amount = amount;
                    }
                    convert::outcome(outcome)
                }
                None => Outcome::Continue,
            }
        }),
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
        mod_api::MobDamageFeedbackComponent::Immunity { ticks } => {
            MobDamageFeedbackComponent::Immunity {
                ticks: ticks.min(1200),
            }
        }
    }
}

fn finite_nonnegative(value: f32, fallback: f32) -> f32 {
    if value.is_finite() {
        value.max(0.0)
    } else {
        fallback
    }
}

#[cfg(any(test, feature = "test-support"))]
pub fn wat_abi_exports() -> String {
    instance::wat_abi_exports(mod_api::ABI_VERSION)
}

#[cfg(test)]
pub mod tests;

pub(crate) fn resolve_bone_poses(bones: Vec<mod_api::BonePoseData>) -> Option<Vec<BonePose>> {
    if !bones.iter().all(mod_api::BonePoseData::is_finite) {
        return None;
    }
    Some(
        bones
            .into_iter()
            .filter_map(|b| {
                Some(BonePose {
                    bone: crate::player::model::bone_id(&b.bone)?,
                    rotation: b.rotation,
                    translation: b.translation,
                    hold: b.mode == mod_api::BonePoseMode::Replace,
                })
            })
            .collect(),
    )
}

pub(crate) const BONE_POSE_REFUSAL: &str =
    "SetPlayerBonePose: non-finite rotation/translation component";
