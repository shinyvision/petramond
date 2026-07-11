//! Guest-side SDK for petramond mods.
//!
//! A mod implements [`Mod`], calls [`register_mod!`], and builds with plain
//! `cargo build --target wasm32-unknown-unknown` (see `mods-src/`). The SDK
//! owns the raw ABI — the `mod_alloc`/`mod_free`/`mod_dispatch` exports, the
//! `host_dispatch` import, postcard framing, pointer packing — so mod code
//! only ever sees `mod-api` types (re-exported here) and the safe wrappers
//! ([`log`], [`current_tick`], [`rng_u64`], [`register_tick_system`],
//! [`register_event_handler`]).
//!
//! Determinism contract (WIKI/modding.md): mod code runs only inside
//! `mod_init`, tick systems, and event handlers; randomness only through
//! [`rng_u64`]'s seeded host streams; no clock, no filesystem, no entropy.
//! A panic aborts the guest (the host logs it, disables the mod for the
//! session, and keeps ticking).

pub use mod_api::*;

mod client;
mod containers;
mod core_calls;
mod entities;
mod gui;
mod kv;
mod player;
mod sounds;
mod world;
mod worldgen;

#[doc(hidden)]
pub mod __rt;

pub use client::*;
pub use containers::*;
pub use core_calls::*;
pub use entities::*;
pub use gui::*;
pub use kv::*;
pub use player::*;
pub use sounds::*;
pub use world::*;
pub use worldgen::*;

/// A mod's logic. One instance lives for the whole session (state persists
/// between dispatches — in-memory only; persistent storage is Phase 3).
///
/// # Worldgen instances are separate
///
/// If the mod registers worldgen hooks, the engine ALSO instantiates it on
/// each worldgen worker thread (and lazily on any thread that generates
/// terrain). Those instances share NOTHING with the tick instance — separate
/// wasm memories, separate `Self` state. Their `init` runs too, so keep `init`
/// PURE: registrations are accepted (and simply ignored off the main
/// instance), [`resolve_block`]/[`log`]/[`rng_u64`] work everywhere, but any
/// sim-scoped call (world/entity/player/KV/env) returns an error there — and
/// the SDK wrappers turn that into a panic that disables the instance.
pub trait Mod: Default {
    /// The registration window: call [`register_tick_system`] /
    /// [`register_event_handler`] / [`register_worldgen_feature`] /
    /// [`register_stage_replacement`] / [`register_generator`] here (they are
    /// rejected anywhere else).
    fn init(&mut self);

    /// A tick system registered under `system_id` is due. Runs once per game
    /// tick (20/s) at its registered stage attachment.
    fn tick_system(&mut self, _system_id: u32) {}

    /// An event the mod registered for. For pre events the payload is echoed
    /// back mutated — the engine applies the taxonomy's mutable fields (e.g.
    /// damage `amount`) — and the returned [`Outcome`] can cancel; post events
    /// are observe-only (the outcome is ignored).
    fn handle_event(&mut self, _handler_id: u32, _payload: &mut EventPayload) -> Outcome {
        Outcome::Continue
    }

    /// A worldgen feature registered under `feature_id`, dispatched once per
    /// generated 16³ section. Return the feature's block writes in world
    /// coordinates; the engine clips them to the dispatched section. MUST be a
    /// pure function of `ctx` — see [`GenCtx`] for the full seam/determinism
    /// contract and the helpers that get it right by default.
    fn gen_feature(&mut self, _feature_id: u32, _ctx: &GenCtx) -> Vec<GenWrite> {
        Vec::new()
    }

    /// A registered `Climate` stage replacement: return the 256-entry column
    /// biome map (`z*16 + x`; `ctx.biomes()` carries the engine's proposal).
    /// Anything but exactly 256 valid biome ids disables the mod and the
    /// engine's climate runs instead.
    fn gen_climate(&mut self, _callback_id: u32, _ctx: &GenCtx) -> Vec<u8> {
        Vec::new()
    }

    /// A registered `Terrain` stage replacement: return the section's complete
    /// 4096-block fill (layout `y*256 + z*16 + x`). Anything but exactly 4096
    /// registered block ids disables the mod and the engine terrain runs
    /// instead.
    fn gen_terrain(&mut self, _callback_id: u32, _ctx: &GenCtx) -> Vec<u8> {
        Vec::new()
    }

    /// A registered `Underground`/`Vegetation`/`Trees` stage replacement:
    /// like [`Mod::gen_feature`], but the write list runs INSTEAD of the
    /// engine stage.
    fn gen_stage(
        &mut self,
        _callback_id: u32,
        _stage: WorldgenStage,
        _ctx: &GenCtx,
    ) -> Vec<GenWrite> {
        Vec::new()
    }

    /// A button of this mod's own GUI was clicked (dispatched on the tick, in
    /// click order). `kind_key` is the GUI's registered kind, `widget_id` the
    /// manifest button id, and `pos` the block the GUI was opened from
    /// (`None` for a programmatic [`gui_open`]). Typical handling: update the
    /// session's state map via [`gui_state_set`] so the GUI's `label` /
    /// `rotimage` widgets redraw.
    fn gui_click(&mut self, _kind_key: &str, _widget_id: &str, _pos: Option<[i32; 3]>) {}

    /// Core is asking whether this candidate should spawn one of this mod's
    /// hostile species. Return a mob registry key to request a spawn, or `None`
    /// to let core keep searching. Core still validates category, caps, and
    /// physical body fit before spawning.
    fn hostile_spawn_candidate(
        &mut self,
        _callback_id: u32,
        _candidate: &HostileSpawnCandidate,
    ) -> Option<String> {
        None
    }

    /// A behavior hook fired on a block whose row's `behavior` key this mod
    /// registered via [`register_block_behavior`]. Dispatched on the game
    /// tick right after the world's own scheduled/random ticks; edit the
    /// world through the sim host calls ([`set_block`], [`schedule_tick`], …).
    fn block_hook(&mut self, _callback_id: u32, _kind: BlockHookKind, _pos: [i32; 3]) {}

    /// One AI decision for one mob this tick, for a brain-row `node` key this
    /// mod registered via [`register_ai_node`]. DECISION-ONLY: the dispatch
    /// runs mid-mob-tick with no simulation scope, so sim host calls error
    /// here (RNG/log/tick work). Return `None` (or default fields) for "no
    /// opinion"; the engine merges by the brain row's priority.
    fn ai_node(&mut self, _callback_id: u32, _ctx: &AiNodeCtx) -> Option<AiNodeDecision> {
        None
    }

    /// Presentation-only client frame. This runs in a separate module
    /// instance from the deterministic server/worldgen instances.
    fn client_frame(&mut self, _frame: &ClientFrameData) {}

    /// A registered client key changed state. The host edge-filters physical
    /// events, so one press and one release arrive per gesture.
    fn client_key(&mut self, _action_id: u32, _pressed: bool) {}

    /// Event from one of this module's client GUI documents.
    fn client_ui(&mut self, _kind_key: &str, _event: &ClientUiEvent) {}

    /// Pointer event over this module's open physical-pixel canvas.
    fn client_canvas(&mut self, _canvas_key: &str, _event: &ClientCanvasEvent) {}
}

/// Define the raw wasm exports for a [`Mod`] implementation. Exactly one call
/// per mod crate:
///
/// ```ignore
/// #[derive(Default)]
/// struct MyMod;
/// impl mod_sdk::Mod for MyMod { /* ... */ }
/// mod_sdk::register_mod!(MyMod);
/// ```
#[macro_export]
macro_rules! register_mod {
    ($ty:ty) => {
        static __PETRAMOND_MOD: $crate::__rt::ModSlot<$ty> = $crate::__rt::ModSlot::new();

        #[no_mangle]
        pub extern "C" fn mod_init() {
            $crate::__rt::init(&__PETRAMOND_MOD)
        }

        #[no_mangle]
        pub extern "C" fn mod_alloc(len: u32) -> u32 {
            $crate::__rt::alloc(len)
        }

        #[no_mangle]
        pub extern "C" fn mod_free(ptr: u32, len: u32) {
            $crate::__rt::free(ptr, len)
        }

        #[no_mangle]
        pub extern "C" fn mod_dispatch(ptr: u32, len: u32) -> u64 {
            $crate::__rt::dispatch(&__PETRAMOND_MOD, ptr, len)
        }
    };
}
