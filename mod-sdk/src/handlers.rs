//! Typed handler registration: register CLOSURES, not hand-numbered ids.
//!
//! The raw [`Mod`] surface links a registration to its dispatch through a
//! `u32` the mod invents twice — once in `register_*`, once in the `match`
//! of the hook that receives it — so a typo or a copy-pasted id silently
//! drops events. [`Handlers`] allocates the ids itself and dispatches each one
//! straight to the closure registered with it:
//!
//! ```ignore
//! #[derive(Default)]
//! struct Farming { grown: u32 }
//!
//! impl mod_sdk::Mod for Farming {
//!     fn init(&mut self) {}
//! }
//!
//! impl mod_sdk::TypedMod for Farming {
//!     fn register(&mut self, on: &mut mod_sdk::Registrar<Self>) {
//!         on.on_tick(Stage::Mining, AttachSide::After, 0, |s| s.grown += 1);
//!         on.on::<mod_sdk::event_types::BlockPlaced>(0, |s, ev| {
//!             if let EventPayload::BlockPlaced { .. } = ev { /* ... */ }
//!             Outcome::Continue
//!         });
//!         on.ai_node("farming:tend", |s, ctx| None);
//!     }
//! }
//!
//! mod_sdk::register_typed_mod!(Farming);
//! ```
//!
//! The raw trait stays underneath for power users: a [`TypedMod`] is still a
//! [`Mod`], every other hook (worldgen, bakes, client, GUI) is its own, and
//! an id the mod registered by hand (below [`TYPED_ID_BASE`]) still reaches
//! its raw hook. Mistakes surface instead of vanishing: registering one AI
//! node or block behavior key twice panics at registration (inside
//! `mod_init`, so the load fails with the key named), and a dispatch the
//! registrar has no closure for — or an event whose payload is not the kind
//! it was registered for — is logged once instead of dropped silently.

use crate::{
    AiNodeCtx, AiNodeDecision, AttachSide, BlockHookKind, EventKind, EventPayload, Outcome, Stage,
};

use crate::Mod;

pub const TYPED_ID_BASE: u32 = 0x8000_0000;

pub trait EventType {
    const KIND: EventKind;
}

pub mod event_types {
    use super::{EventKind, EventType};

    macro_rules! event_types {
        ($($name:ident),* $(,)?) => {
            $(
                pub struct $name;

                impl EventType for $name {
                    const KIND: EventKind = EventKind::$name;
                }
            )*
        };
    }

    event_types!(
        BlockPlacePre,
        BlockBreakPre,
        InteractAttempt,
        ItemUsePre,
        MobDamagePre,
        PlayerDamagePre,
        BlockPlaced,
        BlockBroken,
        ItemUsed,
        MobDied,
        MobSpawned,
        PlayerDamaged,
        PlayerDied,
        ContainerOpened,
        ContainerClosed,
        SectionGenerated,
        SectionLoaded,
        PlayerDismounted,
        MobTagAdded,
        MobTagRemoved,
        ItemPickedUp,
        ItemObtained,
        MobDamaged,
        Interacted,
        ModEvent,
        UseUnclaimed,
        AttackAttempt,
        ProjectileHit,
        ActorActed,
        SchematicChosen,
        SchematicPositioned,
        CellsEditPre,
        ClientEvent,
    );
}

type TickFn<S> = Box<dyn FnMut(&mut S)>;
type EventFn<S> = Box<dyn FnMut(&mut S, &mut EventPayload) -> Outcome>;
type AiNodeFn<S> = Box<dyn FnMut(&mut S, &AiNodeCtx) -> Option<AiNodeDecision>>;
type BlockHookFn<S> = Box<dyn FnMut(&mut S, BlockHookKind, [i32; 3])>;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Lane {
    Tick,
    Event,
    AiNode,
    BlockHook,
}

pub struct Handlers<S> {
    ticks: Vec<TickFn<S>>,
    events: Vec<(EventKind, EventFn<S>)>,
    ai_nodes: Vec<(String, AiNodeFn<S>)>,
    block_hooks: Vec<(String, BlockHookFn<S>)>,
    warned: Vec<(Lane, u32)>,
}

pub type Registrar<S> = Handlers<S>;

impl<S> Default for Handlers<S> {
    fn default() -> Self {
        Self {
            ticks: Vec::new(),
            events: Vec::new(),
            ai_nodes: Vec::new(),
            block_hooks: Vec::new(),
            warned: Vec::new(),
        }
    }
}

fn typed_id(index: usize) -> u32 {
    u32::try_from(index)
        .ok()
        .and_then(|i| TYPED_ID_BASE.checked_add(i))
        .expect("too many typed handlers")
}

fn typed_index(id: u32) -> Option<usize> {
    id.checked_sub(TYPED_ID_BASE).map(|i| i as usize)
}

impl<S> Handlers<S> {
    pub fn on<E: EventType>(
        &mut self,
        priority: i32,
        f: impl FnMut(&mut S, &mut EventPayload) -> Outcome + 'static,
    ) {
        self.event(E::KIND, priority, f);
    }

    pub fn on_tick(
        &mut self,
        stage: Stage,
        attach: AttachSide,
        priority: i32,
        f: impl FnMut(&mut S) + 'static,
    ) {
        self.tick(stage, attach, priority, f);
    }

    pub fn tick(
        &mut self,
        stage: Stage,
        attach: AttachSide,
        priority: i32,
        f: impl FnMut(&mut S) + 'static,
    ) {
        let id = typed_id(self.ticks.len());
        crate::register_tick_system(stage, attach, priority, id);
        self.ticks.push(Box::new(f));
    }

    pub fn event(
        &mut self,
        event: EventKind,
        priority: i32,
        f: impl FnMut(&mut S, &mut EventPayload) -> Outcome + 'static,
    ) {
        let id = typed_id(self.events.len());
        crate::register_event_handler(event, priority, id);
        self.events.push((event, Box::new(f)));
    }

    pub fn ai_node(
        &mut self,
        key: &str,
        f: impl FnMut(&mut S, &AiNodeCtx) -> Option<AiNodeDecision> + 'static,
    ) {
        assert!(
            self.ai_nodes.iter().all(|(k, _)| k != key),
            "AI node '{key}' registered twice"
        );
        let id = typed_id(self.ai_nodes.len());
        crate::register_ai_node(key, id);
        self.ai_nodes.push((key.to_owned(), Box::new(f)));
    }

    pub fn block_hook(
        &mut self,
        key: &str,
        f: impl FnMut(&mut S, BlockHookKind, [i32; 3]) + 'static,
    ) {
        assert!(
            self.block_hooks.iter().all(|(k, _)| k != key),
            "block behavior '{key}' registered twice"
        );
        let id = typed_id(self.block_hooks.len());
        crate::register_block_behavior(key, id);
        self.block_hooks.push((key.to_owned(), Box::new(f)));
    }

    fn warn_unhandled(&mut self, lane: Lane, id: u32, what: &str) {
        if self.warned.contains(&(lane, id)) {
            return;
        }
        self.warned.push((lane, id));
        crate::log(&format!("mod-sdk: {what} (id {id:#x}); ignoring it"));
    }

    fn dispatch_tick(&mut self, state: &mut S, id: u32) -> bool {
        let Some(index) = typed_index(id) else {
            return false;
        };
        match self.ticks.get_mut(index) {
            Some(f) => f(state),
            None => self.warn_unhandled(Lane::Tick, id, "no typed tick system"),
        }
        true
    }

    fn dispatch_event(
        &mut self,
        state: &mut S,
        id: u32,
        payload: &mut EventPayload,
    ) -> Option<Outcome> {
        let index = typed_index(id)?;
        let outcome = match self.events.get_mut(index) {
            Some((kind, f)) if *kind == payload.kind() => f(state, payload),
            Some((kind, _)) => {
                let what = format!(
                    "a {:?} event reached the handler registered for {kind:?}",
                    payload.kind()
                );
                self.warn_unhandled(Lane::Event, id, &what);
                Outcome::Continue
            }
            None => {
                self.warn_unhandled(Lane::Event, id, "no typed event handler");
                Outcome::Continue
            }
        };
        Some(outcome)
    }

    fn dispatch_ai_node(
        &mut self,
        state: &mut S,
        id: u32,
        ctx: &AiNodeCtx,
    ) -> Option<Option<AiNodeDecision>> {
        let index = typed_index(id)?;
        Some(match self.ai_nodes.get_mut(index) {
            Some((_, f)) => f(state, ctx),
            None => {
                self.warn_unhandled(Lane::AiNode, id, "no typed AI node");
                None
            }
        })
    }

    fn dispatch_block_hook(
        &mut self,
        state: &mut S,
        id: u32,
        kind: BlockHookKind,
        pos: [i32; 3],
    ) -> bool {
        let Some(index) = typed_index(id) else {
            return false;
        };
        match self.block_hooks.get_mut(index) {
            Some((_, f)) => f(state, kind, pos),
            None => self.warn_unhandled(Lane::BlockHook, id, "no typed block behavior"),
        }
        true
    }
}

pub trait TypedMod: Mod + 'static {
    fn register(&mut self, on: &mut Handlers<Self>);
}

pub struct Typed<T: TypedMod> {
    state: T,
    handlers: Handlers<T>,
}

impl<T: TypedMod> Default for Typed<T> {
    fn default() -> Self {
        Self {
            state: T::default(),
            handlers: Handlers::default(),
        }
    }
}

impl<T: TypedMod> Mod for Typed<T> {
    const REQUIRES: crate::Capabilities = T::REQUIRES;

    fn init(&mut self) {
        self.state.init();
        self.state.register(&mut self.handlers);
    }

    fn tick_system(&mut self, system_id: u32) {
        if !self.handlers.dispatch_tick(&mut self.state, system_id) {
            self.state.tick_system(system_id);
        }
    }

    fn handle_event(&mut self, handler_id: u32, payload: &mut EventPayload) -> Outcome {
        match self
            .handlers
            .dispatch_event(&mut self.state, handler_id, payload)
        {
            Some(outcome) => outcome,
            None => self.state.handle_event(handler_id, payload),
        }
    }

    fn ai_node(&mut self, callback_id: u32, ctx: &AiNodeCtx) -> Option<AiNodeDecision> {
        match self
            .handlers
            .dispatch_ai_node(&mut self.state, callback_id, ctx)
        {
            Some(decision) => decision,
            None => self.state.ai_node(callback_id, ctx),
        }
    }

    fn block_hook(&mut self, callback_id: u32, kind: BlockHookKind, pos: [i32; 3]) {
        if !self
            .handlers
            .dispatch_block_hook(&mut self.state, callback_id, kind, pos)
        {
            self.state.block_hook(callback_id, kind, pos);
        }
    }

    fn gen_feature(&mut self, feature_id: u32, ctx: &crate::GenCtx) -> crate::GenOutput {
        self.state.gen_feature(feature_id, ctx)
    }

    fn gen_claims(&mut self, feature_id: u32, ctx: &crate::ClaimsCtx) -> crate::GenClaims {
        self.state.gen_claims(feature_id, ctx)
    }

    fn gen_climate(&mut self, callback_id: u32, ctx: &crate::GenCtx) -> Vec<u8> {
        self.state.gen_climate(callback_id, ctx)
    }

    fn gen_terrain(&mut self, callback_id: u32, ctx: &crate::GenCtx) -> Vec<u16> {
        self.state.gen_terrain(callback_id, ctx)
    }

    fn gen_stage(
        &mut self,
        callback_id: u32,
        stage: crate::WorldgenStage,
        ctx: &crate::GenCtx,
    ) -> crate::GenOutput {
        self.state.gen_stage(callback_id, stage, ctx)
    }

    fn gui_click(&mut self, kind_key: &str, widget_id: &str, at: Option<crate::ContainerAddress>) {
        self.state.gui_click(kind_key, widget_id, at);
    }

    fn hostile_spawn_candidate(
        &mut self,
        callback_id: u32,
        candidate: &crate::HostileSpawnCandidate,
    ) -> Option<String> {
        self.state.hostile_spawn_candidate(callback_id, candidate)
    }

    fn client_frame(&mut self, frame: &crate::ClientFrameData) {
        self.state.client_frame(frame);
    }

    fn client_key(&mut self, action_id: u32, pressed: bool) {
        self.state.client_key(action_id, pressed);
    }

    fn client_ui(&mut self, kind_key: &str, event: &crate::ClientUiEvent) {
        self.state.client_ui(kind_key, event);
    }

    fn client_canvas(&mut self, canvas_key: &str, event: &crate::ClientCanvasEvent) {
        self.state.client_canvas(canvas_key, event);
    }

    fn client_canvas_scroll(&mut self, canvas_key: &str, x: f32, y: f32, delta: f32) {
        self.state.client_canvas_scroll(canvas_key, x, y, delta);
    }

    fn bake_shape_sim(
        &mut self,
        shape_kind: u16,
        cells: &[crate::CellInput],
    ) -> Vec<crate::BakedSimCell> {
        self.state.bake_shape_sim(shape_kind, cells)
    }

    fn bake_shape_render(
        &mut self,
        shape_kind: u16,
        cells: &[crate::CellInput],
    ) -> Vec<crate::BakedRenderCell> {
        self.state.bake_shape_render(shape_kind, cells)
    }

    fn bake_shape_item(
        &mut self,
        shape_kind: u16,
        block: crate::BlockId,
    ) -> crate::BakedItemGeometry {
        self.state.bake_shape_item(shape_kind, block)
    }

    fn shape_placement_plan(
        &mut self,
        shape_kind: u16,
        block: crate::BlockId,
        inputs: &crate::PlaceInputsView,
    ) -> crate::ShapePlacementResult {
        self.state.shape_placement_plan(shape_kind, block, inputs)
    }
}

#[macro_export]
macro_rules! register_typed_mod {
    ($ty:ty) => {
        $crate::register_mod!($crate::Typed<$ty>);
    };
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests;
