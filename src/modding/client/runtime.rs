//! Presentation-only client WASM instances.
//!
//! A pack opts in with `client_wasm`; this runtime is separate from the
//! deterministic server/worldgen instances. It can sample final cells from
//! the client's replica, publish document state/images, receive registered
//! key, GUI, and canvas events, and persist namespaced blobs in a host-owned sandbox.

use std::collections::{BTreeSet, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use mod_api::{
    ClientFrameData, ClientUiEvent, EventKind, EventPayload, GuestCall, GuestRet, HeldPose,
    Outcome, PlayerSnapshot, RuntimeSide,
};
use petramond_world::inventory::Hand;

use crate::player::BonePose;

use crate::world::World;

use super::state::{ClientCommand, ClientImageData};
use crate::modding::instance::ModInstance;

struct ClientMod {
    id: String,
    instance: ModInstance,
}

/// One client-registered event handler: the kind it asked for during
/// `mod_init`, and where to dispatch it. Serves both client routes — the
/// speculative PRE kinds ([`ClientModRuntime::predict_claim`]) and the mod
/// cues the server addressed at this client ([`ClientModRuntime::mod_event`]).
struct Handler {
    kind: EventKind,
    mod_index: usize,
    handler_id: u32,
}

/// The kinds a client instance may register, and they are two different
/// things. The PRE kinds are dispatched speculatively against the replica, so
/// a mod consumer is as predictable as an engine one; `ModEvent` is not a
/// prediction at all but the DELIVERY of a cue the server addressed here
/// (`EmitEventTo`), which is the only way a client mod hears about something
/// local input does not imply. Anything else registered on a client instance
/// is a mistake — logged and ignored, never dispatched.
fn client_dispatchable(kind: EventKind) -> bool {
    matches!(
        kind,
        EventKind::InteractAttempt
            | EventKind::BlockPlacePre
            | EventKind::ItemUsePre
            | EventKind::UseUnclaimed
            | EventKind::ModEvent
    )
}

#[derive(Clone)]
pub struct ClientUiView {
    pub state: Arc<std::collections::BTreeMap<String, mod_api::GuiValue>>,
    pub images: Vec<ClientImageData>,
}

pub struct ClientCanvasElementView {
    pub element: mod_api::ClientCanvasElement,
    pub image: ClientImageData,
}

pub struct ClientCanvasView {
    pub offset: [f32; 2],
    pub elements: Vec<ClientCanvasElementView>,
}

/// One mod-registered remappable key action, resolved for this session.
pub struct ModKeyAction {
    /// Namespaced identity (`mod_id:action`): the remap-persistence key, the
    /// dispatch handle, and the controls-screen row id.
    pub full_id: String,
    pub label: String,
    /// Controls-screen category: the owning pack's display name.
    pub category: String,
    /// The registered DEFAULT key (the player may remap it away).
    pub default_code: petramond_world::keycode::KeyCode,
    mod_index: usize,
    action_id: u32,
}

pub struct ClientModRuntime {
    mods: Vec<ClientMod>,
    /// Indices into [`mods`](Self::mods) in MOD-ID order — the sequence the
    /// server's `BodyClaims` folds body claims in. Load order is a dependency
    /// topo sort and is deliberately NOT it.
    fold_order: Vec<usize>,
    /// Client event handlers in dispatch order: `(priority, load order,
    /// registration order)` — the same ordering contract as the server bus.
    /// Filtered by kind at each dispatch site.
    handlers: Vec<Handler>,
    actions: Vec<ModKeyAction>,
    overlays: Vec<super::state::ClientOverlayRegistration>,
    /// Currently-down action `full_id`s — the edge filter for `ClientKey`
    /// dispatch, whatever input the player bound.
    pressed: HashSet<String>,
    /// Test-only scripted answer for [`Self::placement_plan`]: lets prediction
    /// tests drive the custom-shape placement arm without a wasm instance.
    #[cfg(any(test, feature = "test-support"))]
    pub scripted_shape_plan: Option<mod_api::ShapePlacementResult>,
}

impl ClientModRuntime {
    /// Load the session's client mods. `enabled` is the session's
    /// mod-enablement AUTHORITY: locally the installed packs minus the
    /// world's disabled set; on a remote join the server's
    /// handshake-reported mod set. A locally installed client mod the
    /// server does not run therefore never activates.
    pub fn load(world_seed: u32, session_key: &str, enabled: &BTreeSet<String>) -> Self {
        let mut mods = Vec::new();
        let mut handler_rows: Vec<(i32, Handler)> = Vec::new();
        let session = session_client_mods(petramond_world::assets::packs(), enabled);
        crate::modding::host::module_cache::prewarm(session.iter().map(|(_, path)| path.clone()));
        for (id, path) in session {
            let module = match crate::modding::host::module_for(&path) {
                Ok(module) => module,
                Err(e) => {
                    log::error!("client mod '{id}' disabled: {e}");
                    continue;
                }
            };
            let storage = client_storage_dir(session_key, &id);
            let mut instance = match ModInstance::from_module_side(
                &id,
                &module,
                world_seed,
                RuntimeSide::Client,
                Some(storage),
            ) {
                Ok(instance) => instance,
                Err(e) => {
                    log::error!("client mod '{id}' disabled: {e}");
                    continue;
                }
            };
            instance.call_init_detached();
            if instance.disabled() {
                continue;
            }
            // Client registrations live in ClientStoreData; of the
            // simulation registrations only the client-dispatchable event
            // handlers are meaningful here — the rest are irrelevant to this
            // isolated instance (a dual-side wasm branches its init on
            // RuntimeSide).
            let registrations = instance.take_registrations();
            let mod_index = mods.len();
            for reg in registrations {
                if let crate::modding::host::Registration::EventHandler {
                    event,
                    priority,
                    handler_id,
                } = reg
                {
                    if client_dispatchable(event) {
                        handler_rows.push((
                            priority,
                            Handler {
                                kind: event,
                                mod_index,
                                handler_id,
                            },
                        ));
                    } else {
                        log::warn!(
                            "client mod '{id}': event kind {event:?} is not dispatched on a client instance; handler ignored"
                        );
                    }
                }
            }
            mods.push(ClientMod { id, instance });
        }

        let mut actions = Vec::new();
        let mut overlays = Vec::new();
        for (index, loaded) in mods.iter().enumerate() {
            let Some(data) = loaded.instance.client_data() else {
                continue;
            };
            // Overlay image keys are namespace-guarded at registration (and
            // per-mod duplicates rejected there), so keys are already unique
            // across mods.
            overlays.extend(data.overlays.iter().cloned());
            // Category = the pack's display name; the id keys the category
            // when a pack somehow has no display row.
            let category = petramond_world::assets::packs()
                .iter()
                .find(|p| p.id.as_deref() == Some(loaded.id.as_str()))
                .map(|p| p.name.clone())
                .unwrap_or_else(|| loaded.id.clone());
            for binding in &data.key_bindings {
                // The DEFAULT may not shadow an engine default — the player
                // could no longer tell who owns the key out of the box.
                // (Remaps are the player's own choice and are not policed.)
                if reserved_key(&binding.key) {
                    log::error!(
                        "client mod '{}': default key '{}' conflicts with an engine binding; \
                         action '{}' ignored",
                        loaded.id,
                        binding.key,
                        binding.id
                    );
                    continue;
                }
                let Some(default_code) = key_code_for_name(&binding.key) else {
                    log::error!(
                        "client mod '{}': unknown default key '{}'; action '{}' ignored",
                        loaded.id,
                        binding.key,
                        binding.id
                    );
                    continue;
                };
                actions.push(ModKeyAction {
                    full_id: format!("{}:{}", loaded.id, binding.id),
                    label: binding.label.clone(),
                    category: category.clone(),
                    default_code,
                    mod_index: index,
                    action_id: binding.action_id,
                });
            }
        }
        // Stable sort: ties keep (load order, registration order).
        handler_rows.sort_by_key(|(priority, _)| *priority);
        let handlers = handler_rows.into_iter().map(|(_, h)| h).collect();

        // Body claims fold in MOD-ID order, not load order — see `fold_order`.
        let mut fold_order: Vec<usize> = (0..mods.len()).collect();
        fold_order.sort_by(|&a, &b| mods[a].id.cmp(&mods[b].id));

        let mut rt = Self {
            mods,
            fold_order,
            handlers,
            actions,
            overlays,
            pressed: HashSet::new(),
            #[cfg(any(test, feature = "test-support"))]
            scripted_shape_plan: None,
        };
        rt.bake_item_geometry();
        rt
    }

    /// One-time load pass: bake every custom-shape block's ITEM geometry
    /// (`BakeShapeItem`) on its owning client mod and cache it for the item
    /// renderer. Detached (no world) — the item form is a pure function of the
    /// block. A block whose owner ships no client wasm (or a trapped bake) is
    /// simply skipped, and its item draws as a plain cube.
    fn bake_item_geometry(&mut self) {
        use petramond_world::block::ShapeFamily;
        for &block in petramond_world::block::Block::all() {
            if block.shape_family() != ShapeFamily::Custom {
                continue;
            }
            let key = block.shape_kind().key();
            let shape_kind = block.shape_kind().0;
            let block_id = block.id();
            let Some(loaded) = self.owner_mod_mut(key) else {
                continue;
            };
            let call = GuestCall::BakeShapeItem {
                shape_kind,
                block_id: mod_api::BlockId(block_id),
            };
            if let Some(GuestRet::BakedItem(geo)) = loaded.instance.call_guest_detached(&call) {
                // Sanitize the guest boxes; a breach falls back to the cube icon.
                if let Ok(boxes) = crate::world::ingest_shape_boxes(&geo.boxes) {
                    if !boxes.is_empty() {
                        petramond_world::block::item_shape_bake::set_item_bake(block_id, boxes);
                    }
                }
            }
        }
    }

    /// Speculatively dispatch a predicted pre event to every client
    /// predictor registered for its kind, in bus order, with `actor`
    /// published as the `PlayerState` snapshot and the REPLICA as the world
    /// scope. Returns whether any predictor answered Cancel — "I predict I
    /// claim (interact/use) or veto (place_pre) this attempt". Prediction is
    /// presentation-only: handlers must not mutate (mutating host calls are
    /// capability-blocked on client instances anyway).
    pub fn predict_claim(
        &mut self,
        world: &World,
        actor: &PlayerSnapshot,
        payload: &EventPayload,
    ) -> bool {
        let kind = payload.kind();
        for p in &self.handlers {
            if p.kind != kind {
                continue;
            }
            let loaded = &mut self.mods[p.mod_index];
            if loaded.instance.disabled() {
                continue;
            }
            let call = GuestCall::HandleEvent {
                id: p.handler_id,
                payload: payload.clone(),
            };
            let mut mine = actor.clone();
            mine.holds_use = loaded.instance.client_data().is_some_and(|d| d.holds_use);
            let ret =
                super::scope::enter_actor(mine, || loaded.instance.call_guest_client(world, &call));
            match ret {
                Some(GuestRet::Event {
                    outcome: Outcome::Cancel,
                    ..
                }) => return true,
                None | Some(GuestRet::Event { .. }) => {}
                Some(_) => loaded
                    .instance
                    .disable("returned a non-event reply to a prediction dispatch"),
            }
        }
        false
    }

    /// Deliver a mod cue the server addressed at this client (`EmitEventTo`)
    /// to the pack that OWNS the key's namespace, as an ordinary `ModEvent`
    /// dispatch.
    ///
    /// Only the owner is dispatched, unlike the server bus where every
    /// `ModEvent` handler sees every key: this lane is addressed twice over —
    /// at a player AND at a pack — and a cross-pack broadcast is what
    /// `EmitEvent` on the server already is. An `actor` snapshot rides along
    /// so the handler reads `player_state()` exactly as the frame hook does;
    /// a cue about the local player that could not name them would be useless
    /// for the pose calls it exists to drive.
    pub fn mod_event(&mut self, world: &World, actor: &PlayerSnapshot, key: &str, data: &[u8]) {
        let Some(mod_index) = self.owner_index(key) else {
            return;
        };
        let payload = EventPayload::ModEvent {
            key: key.to_owned(),
            data: data.to_vec(),
        };
        let ids: Vec<u32> = self
            .handlers
            .iter()
            .filter(|h| h.kind == EventKind::ModEvent && h.mod_index == mod_index)
            .map(|h| h.handler_id)
            .collect();
        for id in ids {
            let loaded = &mut self.mods[mod_index];
            if loaded.instance.disabled() {
                return;
            }
            let call = GuestCall::HandleEvent {
                id,
                payload: payload.clone(),
            };
            super::scope::enter_actor(actor.clone(), || {
                // The verdict is meaningless here — nothing downstream is
                // cancellable — so any well-formed event reply is accepted.
                match loaded.instance.call_guest_client(world, &call) {
                    None | Some(GuestRet::Event { .. }) => {}
                    Some(_) => loaded
                        .instance
                        .disable("returned a non-event reply to a mod-event dispatch"),
                }
            });
        }
    }

    /// The client twin of the server's custom-shape placement dispatch: ask
    /// the shape's owning CLIENT instance for its placement plan against the
    /// replica (`GuestCall::ShapePlacementPlan`), with `actor` published as
    /// the `PlayerState` snapshot. The plan is deterministic, so the two
    /// sides compute the same write — the ghost presents it and the
    /// authoritative delta confirms. `None` = no reachable owner; the caller
    /// falls through to the ordinary ghost, the server's fall-through twin.
    pub fn placement_plan(
        &mut self,
        world: &World,
        actor: &PlayerSnapshot,
        shape_key: &str,
        shape_kind: u8,
        block_id: u16,
        inputs: mod_api::PlaceInputsView,
    ) -> Option<mod_api::ShapePlacementResult> {
        #[cfg(any(test, feature = "test-support"))]
        if let Some(plan) = self.scripted_shape_plan.clone() {
            return Some(plan);
        }
        let loaded = self.owner_mod_mut(shape_key)?;
        let call = GuestCall::ShapePlacementPlan {
            shape_kind,
            block_id: mod_api::BlockId(block_id),
            inputs,
        };
        match super::scope::enter_actor(actor.clone(), || {
            loaded.instance.call_guest_client(world, &call)
        }) {
            Some(GuestRet::ShapePlacement(result)) => Some(result),
            _ => None,
        }
    }

    /// The client twin of `ModHost::bake_placement_sim_boxes`: the would-be
    /// SIM and RENDER boxes of a not-yet-placed custom cell, baked against
    /// the replica. The ghost installs them eagerly so a predicted placement
    /// collides and draws exactly from frame 0; the per-tick pump re-bakes
    /// the same pure result when the delta dirties the cell.
    pub fn bake_placement_geometry(
        &mut self,
        world: &World,
        shape_key: &str,
        shape_kind: u8,
        input: mod_api::CellInput,
    ) -> (
        Option<Vec<petramond_world::block::Aabb>>,
        Option<Box<[petramond_world::block::ShapeRenderBox]>>,
    ) {
        let Some(loaded) = self.owner_mod_mut(shape_key) else {
            return (None, None);
        };
        let sim_call = GuestCall::BakeShapeSim {
            shape_kind,
            cells: vec![input.clone()],
        };
        let sim = match loaded.instance.call_guest_client(world, &sim_call) {
            Some(GuestRet::BakedSim(baked)) => {
                match crate::modding::shape_bake::ingest_sim_bake(&baked, 1) {
                    crate::modding::shape_bake::BakeIngest::Apply(cells) => {
                        cells.into_iter().next().map(|(boxes, _)| boxes)
                    }
                    crate::modding::shape_bake::BakeIngest::Fallback => None,
                    crate::modding::shape_bake::BakeIngest::Disable(reason) => {
                        loaded.instance.disable(&reason);
                        return (None, None);
                    }
                }
            }
            _ => None,
        };
        let render_call = GuestCall::BakeShapeRender {
            shape_kind,
            cells: vec![input],
        };
        let render = match loaded.instance.call_guest_client(world, &render_call) {
            Some(GuestRet::BakedRender(baked)) => {
                match crate::modding::shape_bake::ingest_render_bake(&baked, 1) {
                    crate::modding::shape_bake::BakeIngest::Apply(cells) => {
                        cells.into_iter().next()
                    }
                    crate::modding::shape_bake::BakeIngest::Fallback => None,
                    crate::modding::shape_bake::BakeIngest::Disable(reason) => {
                        loaded.instance.disable(&reason);
                        return (sim, None);
                    }
                }
            }
            _ => None,
        };
        (sim, render)
    }

    /// Bake the SIM geometry of any dirty custom-shape cell on the
    /// CLIENT (each shape's own `client_wasm` `bake_shape_sim`), so the client's
    /// physics/prediction sees the same collision the server does — otherwise a
    /// custom shape would fall back to its (often empty) static boxes and desync.
    /// A missing owner / disabled mod / wrong reply leaves cells uncached
    /// (static fallback), the failure policy.
    pub fn bake_custom_shapes(&mut self, world: &mut World) {
        let cells = world.drain_custom_bake_dirty();
        if cells.is_empty() {
            return;
        }
        // A BTreeMap (not HashMap) over the position-sorted drain gives the same
        // dispatch order the server uses (C1) — the SIM bake is cross-checked
        // against the server, so the two sides must dispatch identically.
        let mut groups: std::collections::BTreeMap<
            (&'static str, u8),
            Vec<crate::world::CustomBakeCell>,
        > = std::collections::BTreeMap::new();
        for cell in cells {
            groups
                .entry((cell.shape_key, cell.shape_kind))
                .or_default()
                .push(cell);
        }
        // Dispatch under an immutable world borrow, collect, then populate.
        let mut baked_sim: Vec<(
            petramond_math::math::IVec3,
            Vec<petramond_world::block::Aabb>,
            mod_api::LightAperture,
        )> = Vec::new();
        let mut baked_render: Vec<(
            petramond_math::math::IVec3,
            Box<[petramond_world::block::ShapeRenderBox]>,
        )> = Vec::new();
        for ((shape_key, shape_kind), group) in &groups {
            let Some(loaded) = self.owner_mod_mut(shape_key) else {
                continue;
            };
            let inputs: Vec<mod_api::CellInput> = group
                .iter()
                .map(crate::modding::shape_bake::cell_input)
                .collect();
            // SIM bake → collision (also cross-checked against the server). A
            // sanitation/protocol breach disables the mod; skip the render bake
            // explicitly rather than relying on the no-op-on-disabled path.
            let sim_call = GuestCall::BakeShapeSim {
                shape_kind: *shape_kind,
                cells: inputs.clone(),
            };
            if let Some(GuestRet::BakedSim(baked)) =
                loaded.instance.call_guest_client(world, &sim_call)
            {
                match crate::modding::shape_bake::ingest_sim_bake(&baked, group.len()) {
                    crate::modding::shape_bake::BakeIngest::Apply(cells) => {
                        for (c, (boxes, aperture)) in group.iter().zip(cells) {
                            baked_sim.push((c.pos, boxes, aperture));
                        }
                    }
                    crate::modding::shape_bake::BakeIngest::Fallback => {}
                    crate::modding::shape_bake::BakeIngest::Disable(reason) => {
                        loaded.instance.disable(&reason);
                        continue;
                    }
                }
            }
            // RENDER bake → mesh geometry (client presentation only).
            let render_call = GuestCall::BakeShapeRender {
                shape_kind: *shape_kind,
                cells: inputs,
            };
            if let Some(GuestRet::BakedRender(baked)) =
                loaded.instance.call_guest_client(world, &render_call)
            {
                match crate::modding::shape_bake::ingest_render_bake(&baked, group.len()) {
                    crate::modding::shape_bake::BakeIngest::Apply(cells) => {
                        for (c, boxes) in group.iter().zip(cells) {
                            baked_render.push((c.pos, boxes));
                        }
                    }
                    crate::modding::shape_bake::BakeIngest::Fallback => {}
                    crate::modding::shape_bake::BakeIngest::Disable(reason) => {
                        loaded.instance.disable(&reason)
                    }
                }
            }
        }
        for (pos, boxes, aperture) in baked_sim {
            world.set_custom_bake(pos, &boxes);
            world.set_custom_light_aperture(pos, aperture);
        }
        for (pos, boxes) in baked_render {
            world.set_custom_render_bake(pos, boxes);
        }
    }

    /// The session's mod-registered remappable actions, for the app's action
    /// table and the controls screen.
    pub fn key_actions(&self) -> &[ModKeyAction] {
        &self.actions
    }

    /// The live (non-disabled) mod owning a namespaced `mod_id:name` key.
    /// THE ownership rule for every keyed dispatch — images, canvases, shapes,
    /// mod cues — so a key routes to one place however it arrived.
    fn owner_index(&self, key: &str) -> Option<usize> {
        let owner = key.split_once(':')?.0;
        self.mods
            .iter()
            .position(|loaded| loaded.id == owner && !loaded.instance.disabled())
    }

    fn owner_mod(&self, key: &str) -> Option<&ClientMod> {
        self.owner_index(key).map(|i| &self.mods[i])
    }

    /// [`owner_mod`](Self::owner_mod) for dispatching into the owner.
    fn owner_mod_mut(&mut self, key: &str) -> Option<&mut ClientMod> {
        self.owner_index(key).map(|i| &mut self.mods[i])
    }

    /// Drive every client mod's per-frame hook, with `actor` published as the
    /// `PlayerState` snapshot — the same query-the-snapshot vocabulary the
    /// prediction dispatches and the whole server side use, so a mod's rule
    /// reads identically wherever it runs. This is what lets a rule derived
    /// from local input (a raised guard) present on the frame the button goes
    /// down instead of a round trip later.
    pub fn frame(&mut self, world: &World, actor: &PlayerSnapshot, frame: ClientFrameData) {
        let call = GuestCall::ClientFrame { frame };
        for loaded in &mut self.mods {
            // Each mod sees ITS OWN answer to "is this press mine", the same
            // way the server resolves the field per caller.
            let mut mine = actor.clone();
            mine.holds_use = loaded.instance.client_data().is_some_and(|d| d.holds_use);
            super::scope::enter_actor(mine, || {
                dispatch_unit(&mut loaded.instance, world, &call, "client frame");
            });
        }
    }

    /// The mod holding the local player's use gesture, if any.
    pub fn use_holder(&self) -> Option<&str> {
        self.mods
            .iter()
            .find(|m| {
                !m.instance.disabled() && m.instance.client_data().is_some_and(|d| d.holds_use)
            })
            .map(|m| m.id.as_str())
    }

    /// The button came up: every hold ends with it.
    pub fn release_use(&mut self) {
        for loaded in &mut self.mods {
            if let Some(data) = loaded.instance.client_data_mut() {
                data.holds_use = false;
            }
        }
    }

    /// Every enabled client mod's store, in MOD-ID order.
    ///
    /// The order is the whole point: the server folds body claims through one
    /// `BodyClaims` keyed by pack id, so a prediction that folded in LOAD order
    /// (a dependency topo sort, which is a different sequence) could pick a
    /// different winner than the authority for a contested hand. Same input,
    /// same order, same answer.
    fn claim_stores(&self) -> impl Iterator<Item = &crate::modding::client::ClientStoreData> {
        self.fold_order.iter().filter_map(|&i| {
            let m = &self.mods[i];
            (!m.instance.disabled())
                .then(|| m.instance.client_data())
                .flatten()
        })
    }

    /// The local player's hand poses: this client's own PREDICTION for any
    /// hand a client mod poses, and `replicated` for the rest.
    ///
    /// A hand a client mod poses is client-authoritative from its first pose
    /// onward — including when that mod RELEASES it, which is the edge the
    /// replicated answer is a round trip late on. A hand no client mod has
    /// ever posed keeps the server's answer untouched, so a pack that poses
    /// only on the server still reaches the local hands.
    pub fn local_held_poses(
        &self,
        replicated: (Option<HeldPose>, Option<HeldPose>),
    ) -> (Option<HeldPose>, Option<HeldPose>) {
        let mut claimed = [false; 2];
        let mut predicted: [Option<HeldPose>; 2] = [None, None];
        for data in self.claim_stores() {
            for (h, hand) in [Hand::Main, Hand::Off].into_iter().enumerate() {
                claimed[h] |= data.poses_hands[h];
                // Later mods in MOD-ID order win a contested hand — the
                // server's own rule, run over the same sequence.
                predicted[h] = data.body.held_pose(hand).or(predicted[h]);
            }
        }
        let pick = |h: usize, replicated| if claimed[h] { predicted[h] } else { replicated };
        (pick(0, replicated.0), pick(1, replicated.1))
    }

    /// The local player's rig-bone offsets: this client's own PREDICTION for
    /// every bone a client mod poses, and `replicated` for the rest — the body
    /// twin of [`Self::local_held_poses`], with the same latch and the same
    /// reason for it.
    ///
    /// The latch is per BONE. Bone offsets compose (each is a rotation about
    /// its own joint), so one pack predicting a shoulder must leave another
    /// pack's server-side head tilt standing; only the bone actually being
    /// predicted may be taken over.
    ///
    /// A predicted offset takes the replicated one's PLACE in the list rather
    /// than being appended: offsets on an ancestor bone and its descendant do
    /// not commute (a shoulder delta carries the elbow with it), so predicting
    /// one must not reorder the set and quietly pose the body differently from
    /// the authority.
    pub fn local_bone_poses(&self, replicated: &[BonePose]) -> Vec<BonePose> {
        let mut claimed: BTreeSet<u16> = BTreeSet::new();
        let mut predicted: Vec<BonePose> = Vec::new();
        for data in self.claim_stores() {
            claimed.extend(data.poses_bones.iter().copied());
            predicted.extend(data.body.bone_poses());
        }
        if claimed.is_empty() {
            return replicated.to_vec();
        }
        let mut taken = vec![false; predicted.len()];
        let mut out: Vec<BonePose> = Vec::with_capacity(replicated.len() + predicted.len());
        for r in replicated {
            if !claimed.contains(&r.bone) {
                out.push(*r);
                continue;
            }
            if let Some((i, p)) = predicted
                .iter()
                .enumerate()
                .find(|(i, p)| p.bone == r.bone && !taken[*i])
            {
                out.push(*p);
                taken[i] = true;
            }
        }
        // Whatever the authority is not carrying yet — the raise that has not
        // round-tripped, and the bones only a client mod ever bends.
        out.extend(
            predicted
                .iter()
                .zip(&taken)
                .filter(|(_, done)| !**done)
                .map(|(p, _)| *p),
        );
        out
    }

    /// Dispatch one bound-action edge to its owning mod, by the action's
    /// namespaced `full_id`. Returns whether a live mod owns the action.
    pub fn action(&mut self, world: &World, full_id: &str, pressed: bool) -> bool {
        let Some((index, action_id)) = self
            .actions
            .iter()
            .find(|a| a.full_id == full_id)
            .map(|a| (a.mod_index, a.action_id))
        else {
            return false;
        };
        if self.mods[index].instance.disabled() {
            self.pressed.remove(full_id);
            return false;
        }
        let was_pressed = self.pressed.contains(full_id);
        if was_pressed == pressed {
            return true;
        }
        if pressed {
            self.pressed.insert(full_id.to_owned());
        } else {
            self.pressed.remove(full_id);
        }
        let call = GuestCall::ClientKey { action_id, pressed };
        dispatch_unit(&mut self.mods[index].instance, world, &call, "client key");
        true
    }

    pub fn ui_event(&mut self, world: &World, kind_key: &str, event: ClientUiEvent) {
        let call = GuestCall::ClientUi {
            kind_key: kind_key.to_owned(),
            event,
        };
        let Some(loaded) = self.owner_mod_mut(kind_key) else {
            return;
        };
        dispatch_unit(&mut loaded.instance, world, &call, "client UI event");
    }

    pub fn canvas_event(
        &mut self,
        world: &World,
        canvas_key: &str,
        event: mod_api::ClientCanvasEvent,
    ) {
        let call = GuestCall::ClientCanvas {
            canvas_key: canvas_key.to_owned(),
            event,
        };
        let Some(loaded) = self.owner_mod_mut(canvas_key) else {
            return;
        };
        dispatch_unit(&mut loaded.instance, world, &call, "client canvas event");
    }

    pub fn canvas_scroll(&mut self, world: &World, canvas_key: &str, x: f32, y: f32, delta: f32) {
        let call = GuestCall::ClientCanvasScroll {
            canvas_key: canvas_key.to_owned(),
            x,
            y,
            delta,
        };
        let Some(loaded) = self.owner_mod_mut(canvas_key) else {
            return;
        };
        dispatch_unit(&mut loaded.instance, world, &call, "client canvas scroll");
    }

    pub fn release_all_keys(&mut self, world: &World) {
        let pressed: Vec<_> = self.pressed.drain().collect();
        for full_id in pressed {
            let Some((index, action_id)) = self
                .actions
                .iter()
                .find(|a| a.full_id == full_id)
                .map(|a| (a.mod_index, a.action_id))
            else {
                continue;
            };
            let call = GuestCall::ClientKey {
                action_id,
                pressed: false,
            };
            dispatch_unit(
                &mut self.mods[index].instance,
                world,
                &call,
                "client key release",
            );
        }
    }

    pub fn overlays(&self) -> &[super::state::ClientOverlayRegistration] {
        &self.overlays
    }

    pub fn image(&self, image_key: &str) -> Option<ClientImageData> {
        self.owner_mod(image_key)?
            .instance
            .client_data()?
            .images
            .get(image_key)
            .cloned()
    }

    pub fn canvas_view(&self, canvas_key: &str) -> Option<ClientCanvasView> {
        let data = self.owner_mod(canvas_key)?.instance.client_data()?;
        let scene = data.canvas_scenes.get(canvas_key)?;
        let elements = scene
            .elements
            .iter()
            .filter_map(|element| {
                let image_key = match element {
                    mod_api::ClientCanvasElement::Image { image_key, .. }
                    | mod_api::ClientCanvasElement::Sprite { image_key, .. } => image_key,
                };
                data.images
                    .get(image_key)
                    .cloned()
                    .map(|image| ClientCanvasElementView {
                        element: element.clone(),
                        image,
                    })
            })
            .collect();
        Some(ClientCanvasView {
            offset: scene.offset,
            elements,
        })
    }

    pub fn view_for(&self, kind_key: &str) -> Option<ClientUiView> {
        let data = self.owner_mod(kind_key)?.instance.client_data()?;
        Some(ClientUiView {
            state: data.ui_state.clone(),
            images: data.images.values().cloned().collect(),
        })
    }

    /// Every mod's current ambient-volume targets, per (mod id, bundle):
    /// `(intensity, wind)` rows the presentation layer pushes into its
    /// ambient drives. A disabled (trapped/watchdogged) mod must not freeze
    /// its last weather on for the rest of the session: its targets read as
    /// zero intensity so the drives ease out and retire (mirrors
    /// `take_commands`).
    pub fn ambient_targets(&self) -> impl Iterator<Item = (&str, u8, f32, [f32; 2])> + '_ {
        self.mods.iter().flat_map(|m| {
            let disabled = m.instance.disabled();
            m.instance.client_data().into_iter().flat_map(move |data| {
                data.ambient_sets
                    .iter()
                    .map(move |(&bundle, &(intensity, wind))| {
                        (
                            m.id.as_str(),
                            bundle,
                            if disabled { 0.0 } else { intensity },
                            wind,
                        )
                    })
            })
        })
    }

    /// Every mod's looping-sound gains: `(sound, gain)`, mods in load order.
    /// The audio side keys its loop table on the resolved sound, so two mods
    /// driving one sound key resolve last-writer-wins there. Disabled mods
    /// contribute nothing — their loops sweep to silence.
    pub fn sound_loops(&self, out: &mut Vec<(petramond_world::sound_registry::Sound, f32)>) {
        out.clear();
        for m in &self.mods {
            if m.instance.disabled() {
                continue;
            }
            let Some(data) = m.instance.client_data() else {
                continue;
            };
            out.extend(data.sound_loops.iter().map(|(&s, &g)| (s, g)));
        }
    }

    /// The combined post-process mood: component-wise MAX over enabled mods
    /// (disabled mods contribute nothing — their mood dies with them).
    pub fn mood(&self) -> [f32; 2] {
        let mut mood = [0.0f32, 0.0];
        for m in &self.mods {
            if m.instance.disabled() {
                continue;
            }
            if let Some(data) = m.instance.client_data() {
                mood[0] = mood[0].max(data.mood[0]);
                mood[1] = mood[1].max(data.mood[1]);
            }
        }
        mood
    }

    pub fn take_commands(&mut self) -> Vec<ClientCommand> {
        let mut out = Vec::new();
        for loaded in &mut self.mods {
            if loaded.instance.disabled() {
                if let Some(data) = loaded.instance.client_data_mut() {
                    data.commands.clear();
                }
                continue;
            }
            if let Some(data) = loaded.instance.client_data_mut() {
                out.append(&mut data.commands);
            }
        }
        out
    }
}

/// Bake the ITEM geometry of every INSTALLED custom-shape block into the item
/// cache, using a detached client instance per owning pack. Run ONCE at client
/// startup, BEFORE the icon atlas bakes (`render::renderer::construct`), so a
/// custom block's inventory icon shows its real baked shape (a chair) instead of
/// the plain cube fallback (which reads a plank). Side-effect-free (no storage,
/// no registration); the per-world runtime re-bakes the enabled subset at join,
/// idempotently. A headless server has no icon atlas and never calls this.
pub fn bake_installed_custom_item_geometry() {
    use petramond_world::block::{Block, ShapeFamily};

    let all: BTreeSet<String> = petramond_world::assets::packs()
        .iter()
        .filter_map(|p| p.id.clone())
        .collect();
    for (id, path) in session_client_mods(petramond_world::assets::packs(), &all) {
        let blocks: Vec<Block> = Block::all()
            .iter()
            .copied()
            .filter(|b| {
                b.shape_family() == ShapeFamily::Custom
                    && petramond_world::registry::namespace(b.shape_kind().key())
                        == Some(id.as_str())
            })
            .collect();
        if blocks.is_empty() {
            continue;
        }
        let Ok(module) = crate::modding::host::module_for(&path) else {
            continue;
        };
        let Ok(mut instance) =
            ModInstance::from_module_side(&id, &module, 0, RuntimeSide::Client, None)
        else {
            continue;
        };
        instance.call_init_detached();
        if instance.disabled() {
            continue;
        }
        for block in blocks {
            let call = GuestCall::BakeShapeItem {
                shape_kind: block.shape_kind().0,
                block_id: mod_api::BlockId(block.id()),
            };
            if let Some(GuestRet::BakedItem(geo)) = instance.call_guest_detached(&call) {
                // Sanitize like the sim/render pumps; a breach just means the
                // item draws its cube fallback (this detached pass has no mod to
                // disable for the session).
                if let Ok(boxes) = crate::world::ingest_shape_boxes(&geo.boxes) {
                    if !boxes.is_empty() {
                        petramond_world::block::item_shape_bake::set_item_bake(block.id(), boxes);
                    }
                }
            }
        }
    }
}

/// The `(mod id, client wasm path)` pairs a session activates: every
/// installed id-bearing pack that ships `client_wasm` AND is in the
/// session's enabled set. Pure — the client-side enablement contract,
/// unit-tested against synthetic pack lists (the client twin of
/// `session_wasm_mods` in `modding/mod.rs`).
fn session_client_mods(
    packs: &[petramond_world::assets::Pack],
    enabled: &BTreeSet<String>,
) -> Vec<(String, PathBuf)> {
    packs
        .iter()
        .filter_map(|pack| {
            let id = pack.id.clone()?;
            let wasm = pack.client_wasm.clone()?;
            if !enabled.contains(&id) {
                log::info!("client mod '{id}' is not enabled for this session; not loading");
                return None;
            }
            Some((id, wasm))
        })
        .collect()
}

fn dispatch_unit(instance: &mut ModInstance, world: &World, call: &GuestCall, what: &str) {
    match instance.call_guest_client(world, call) {
        None | Some(GuestRet::Unit) => {}
        Some(_) => instance.disable(&format!("returned a non-unit reply to {what}")),
    }
}

/// Whether a physical key is already bound by an engine gameplay control:
/// the fixed (non-remappable) table plus every DEFAULT action binding. The
/// player's live remaps deliberately don't move this set — a mod key that was
/// valid at pack load must not turn invalid because the player rebound Sneak.
fn reserved_key(key: &str) -> bool {
    let defaults = petramond_world::controls::BindingSet::default();
    let default_bound = |code: petramond_world::keycode::KeyCode| {
        petramond_world::controls::BindableAction::ALL
            .iter()
            .any(|a| defaults.binding(*a).input == petramond_world::controls::BoundInput::Key(code))
    };
    PHYSICAL_KEYS.iter().any(|(code, name)| {
        *name == key
            && (petramond_world::controls::fixed_control_from_key_code(*code).is_some()
                || default_bound(*code))
    })
}

/// The client-storage identity of a LOCAL world: its save-DIRECTORY name,
/// never the display name. A world's directory never changes after creation
/// (renames rewrite only `world.json`), so personal mod data — minimap
/// exploration, waypoints — follows a renamed world with zero migration.
pub fn local_session_key(world_dir_name: &str) -> String {
    format!("local:{world_dir_name}")
}

/// The client-storage identity of a remote server (its address string).
pub fn remote_session_key(server_identity: &str) -> String {
    format!("remote:{server_identity}")
}

/// The ONE bucket holding every client mod's sandboxed storage for a session
/// identity: `<base>/client_mod_data/<fnv1a64(session_key)>/<mod_id>/...` —
/// this is the `<mod_id>`s' parent, the unit world deletion removes.
fn session_storage_bucket(base: &Path, session_key: &str) -> PathBuf {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in session_key.bytes() {
        hash = (hash ^ byte as u64).wrapping_mul(0x1_0000_0000_01b3);
    }
    base.join("client_mod_data").join(format!("{hash:016x}"))
}

fn client_storage_dir(session_key: &str, mod_id: &str) -> PathBuf {
    session_storage_bucket(&petramond_util::paths::base_data_dir(), session_key).join(mod_id)
}

#[cfg(any(test, feature = "test-support"))]
pub fn client_storage_dir_for_test(session_key: &str, mod_id: &str) -> PathBuf {
    client_storage_dir(session_key, mod_id)
}

/// Test seeding: write entries into a mod's session storage bucket through
/// the ordered worker, flushed before return — perf harnesses fabricate a
/// large explored world without driving real exploration.
#[cfg(any(test, feature = "test-support"))]
pub fn seed_client_storage_for_test(
    session_key: &str,
    mod_id: &str,
    mut entries: Vec<(String, Vec<u8>)>,
) {
    let mut storage = super::storage::ClientStorage::new(client_storage_dir(session_key, mod_id));
    while !entries.is_empty() {
        // Stay under the per-batch byte cap whatever the value sizes.
        let mut take = 0usize;
        let mut bytes = 0usize;
        while take < entries.len() && take < 512 && bytes < 8 << 20 {
            bytes += entries[take].0.len() + entries[take].1.len();
            take += 1;
        }
        let rest = entries.split_off(take);
        // The worker drains asynchronously; when a large seed outruns the
        // pending-byte cap, wait for it rather than fail.
        let batch = entries;
        loop {
            match storage.set_many(batch.clone()) {
                Ok(()) => break,
                Err(error) if error.contains("too many queued writes") => {
                    std::thread::sleep(std::time::Duration::from_millis(2));
                }
                Err(error) => panic!("seed client storage batch: {error}"),
            }
        }
        entries = rest;
    }
    // Drop flushes the worker, so files exist when the test proceeds.
}

/// Delete every client mod's sandboxed storage for a LOCAL world — the
/// world-deletion hook. Exploration maps and waypoints live OUTSIDE the save
/// (personal data, keyed on the world's directory name); without this, a
/// future world reusing the deleted world's directory name would inherit
/// them — explored terrain from a dead seed on a supposed-to-be-black map.
/// Safe against in-flight writes: the storage worker drains synchronously on
/// session drop, and deletion is only reachable from the world-select menu.
pub fn delete_local_world_storage(world_dir_name: &str) -> std::io::Result<()> {
    delete_local_world_storage_at(&petramond_util::paths::base_data_dir(), world_dir_name)
}

fn delete_local_world_storage_at(base: &Path, world_dir_name: &str) -> std::io::Result<()> {
    let bucket = session_storage_bucket(base, &local_session_key(world_dir_name));
    match std::fs::remove_dir_all(bucket) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

/// The bindable physical keys and their stable ABI names — the one table
/// behind [`key_code_for_name`] and [`reserved_key`].
const PHYSICAL_KEYS: &[(petramond_world::keycode::KeyCode, &str)] = {
    use petramond_world::keycode::KeyCode;
    &[
        (KeyCode::KeyA, "key_a"),
        (KeyCode::KeyB, "key_b"),
        (KeyCode::KeyC, "key_c"),
        (KeyCode::KeyD, "key_d"),
        (KeyCode::KeyE, "key_e"),
        (KeyCode::KeyF, "key_f"),
        (KeyCode::KeyG, "key_g"),
        (KeyCode::KeyH, "key_h"),
        (KeyCode::KeyI, "key_i"),
        (KeyCode::KeyJ, "key_j"),
        (KeyCode::KeyK, "key_k"),
        (KeyCode::KeyL, "key_l"),
        (KeyCode::KeyM, "key_m"),
        (KeyCode::KeyN, "key_n"),
        (KeyCode::KeyO, "key_o"),
        (KeyCode::KeyP, "key_p"),
        (KeyCode::KeyQ, "key_q"),
        (KeyCode::KeyR, "key_r"),
        (KeyCode::KeyS, "key_s"),
        (KeyCode::KeyT, "key_t"),
        (KeyCode::KeyU, "key_u"),
        (KeyCode::KeyV, "key_v"),
        (KeyCode::KeyW, "key_w"),
        (KeyCode::KeyX, "key_x"),
        (KeyCode::KeyY, "key_y"),
        (KeyCode::KeyZ, "key_z"),
        (KeyCode::Digit0, "digit_0"),
        (KeyCode::Digit1, "digit_1"),
        (KeyCode::Digit2, "digit_2"),
        (KeyCode::Digit3, "digit_3"),
        (KeyCode::Digit4, "digit_4"),
        (KeyCode::Digit5, "digit_5"),
        (KeyCode::Digit6, "digit_6"),
        (KeyCode::Digit7, "digit_7"),
        (KeyCode::Digit8, "digit_8"),
        (KeyCode::Digit9, "digit_9"),
    ]
};

/// The `KeyCode` behind a registered default-key name (`"key_m"` → `KeyM`).
pub fn key_code_for_name(name: &str) -> Option<petramond_world::keycode::KeyCode> {
    PHYSICAL_KEYS
        .iter()
        .find(|(_, bindable)| *bindable == name)
        .map(|(code, _)| *code)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Deleting a world's client-mod storage removes that world's WHOLE
    /// bucket — every mod's data, nothing of any other identity — and a
    /// world that never stored anything deletes cleanly. Guards the
    /// world-deletion hook against key-derivation drift: a renamed hash or
    /// session-key format that no longer matches what the runtime writes
    /// would silently orphan (or worse, miss) the data again.
    #[test]
    fn world_deletion_removes_exactly_its_own_storage_bucket() {
        let base = std::env::temp_dir().join(format!(
            "petramond-client-storage-delete-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&base);
        let dir_for = |session_key: &str, mod_id: &str| {
            session_storage_bucket(&base, session_key).join(mod_id)
        };
        for (key, mod_id) in [
            (local_session_key("doomed"), "minimap"),
            (local_session_key("doomed"), "othermod"),
            (local_session_key("kept"), "minimap"),
            (remote_session_key("play.example.org"), "minimap"),
        ] {
            let dir = dir_for(&key, mod_id);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("blob"), b"tile").unwrap();
        }

        delete_local_world_storage_at(&base, "doomed").unwrap();
        assert!(
            !session_storage_bucket(&base, &local_session_key("doomed")).exists(),
            "the deleted world's bucket goes whole — every mod's data"
        );
        assert!(
            dir_for(&local_session_key("kept"), "minimap")
                .join("blob")
                .exists(),
            "another world's bucket is untouched"
        );
        assert!(
            dir_for(&remote_session_key("play.example.org"), "minimap")
                .join("blob")
                .exists(),
            "server buckets are untouched"
        );
        // Idempotent: a world with no client-mod data deletes cleanly.
        delete_local_world_storage_at(&base, "doomed").unwrap();
        let _ = std::fs::remove_dir_all(&base);
    }

    /// Client mods activate ONLY for packs in the session's enabled set —
    /// on a remote join that is the server's handshake-reported mod list,
    /// so a locally installed client mod the server does not run (e.g. the
    /// minimap against a server without it) stays inactive.
    #[test]
    fn unlisted_packs_contribute_no_client_instance() {
        let pack = |name: &str, id: Option<&str>, client_wasm: Option<&str>| {
            petramond_world::assets::Pack {
                dir: PathBuf::from(format!("/fixture/{name}")),
                name: name.to_owned(),
                id: id.map(str::to_owned),
                version: None,
                description: String::new(),
                summary: None,
                icon: None,
                wasm: None,
                client_wasm: client_wasm.map(PathBuf::from),
            }
        };
        let packs = [
            pack(
                "minimap",
                Some("minimap"),
                Some("/fixture/minimap/client.wasm"),
            ),
            pack("radar", Some("radar"), Some("/fixture/radar/client.wasm")),
            pack("content_only", None, None),
        ];

        let server_reported: BTreeSet<String> = ["radar".to_owned()].into();
        let ids: Vec<String> = session_client_mods(&packs, &server_reported)
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        assert_eq!(ids, ["radar"], "only enabled packs activate");

        assert!(
            session_client_mods(&packs, &BTreeSet::new()).is_empty(),
            "a server reporting no mods enables no client mods"
        );
        let all: BTreeSet<String> = ["minimap".to_owned(), "radar".to_owned()].into();
        assert_eq!(session_client_mods(&packs, &all).len(), 2);
    }

    /// The reservation rule is DERIVED from the engine's binding table:
    /// engine-bound keys are refused to client mods, unbound keys are free.
    #[test]
    fn engine_bound_keys_are_reserved_for_client_mods() {
        assert!(reserved_key("key_w"));
        assert!(reserved_key("digit_1"));
        assert!(!reserved_key("key_m"));
        assert!(!reserved_key("key_n"));
    }
}
