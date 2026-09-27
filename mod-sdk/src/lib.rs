pub use mod_api::*;

mod abi;
mod bytes;
mod cadence;
pub mod capture;
mod change_log;
mod client;
mod construction;
mod containers;
mod core_calls;
mod data_rows;
mod entities;
mod fast_hash;
mod files;
mod gui;
mod id_shards;
mod instance_data;
mod interact;
pub use mod_api::json;
mod kv;
mod loot;
mod media;
mod memo;
mod mob_sweep;
mod pack_keys;
mod paged;
mod panel_state;
mod player;
mod records;
mod registry;
mod schematics;
mod sounds;
mod structures;
mod tags;
mod view;
mod world;
mod world_view;
mod worldgen;

#[doc(hidden)]
pub mod __rt;
mod handlers;
#[cfg(not(target_arch = "wasm32"))]
pub mod testing;

pub use abi::*;
pub use bytes::*;
pub use cadence::*;
pub use capture::{
    client_presentation_apply, client_presentation_cancel, client_presentation_close,
    client_presentation_open, client_presentation_queue, client_presentation_state,
    client_presentation_time, client_presentation_viewer, client_world_events_begin,
    client_world_events_end, client_world_events_poll, client_world_state_write,
};
pub use change_log::*;
pub use client::*;
pub use construction::*;
pub use containers::*;
pub use core_calls::*;
pub use data_rows::*;
pub use entities::*;
pub use fast_hash::*;
pub use files::*;
pub use gui::*;
pub use handlers::*;
pub use id_shards::*;
pub use instance_data::*;
pub use interact::*;
pub use kv::*;
pub use loot::*;
pub use media::*;
pub use memo::*;
pub use mob_sweep::*;
pub use pack_keys::*;
pub use paged::*;
pub use panel_state::*;
pub use player::*;
pub use records::*;
pub use registry::*;
pub use schematics::*;
pub use sounds::*;
pub use structures::*;
pub use tags::*;
pub use view::*;
pub use world::*;
pub use world_view::*;
pub use worldgen::*;

/// A mod's logic. One instance lives for the whole session (state persists
/// between dispatches — in-memory only; persistent storage is the
/// world/section/mob KV host calls).
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
    const REQUIRES: Capabilities = Capabilities::NONE;

    fn init(&mut self);

    fn tick_system(&mut self, _system_id: u32) {}

    fn handle_event(&mut self, _handler_id: u32, _payload: &mut EventPayload) -> Outcome {
        Outcome::Continue
    }

    fn gen_feature(&mut self, _feature_id: u32, _ctx: &GenCtx) -> GenOutput {
        GenOutput::default()
    }

    fn gen_climate(&mut self, _callback_id: u32, _ctx: &GenCtx) -> Vec<u8> {
        Vec::new()
    }

    fn gen_terrain(&mut self, _callback_id: u32, _ctx: &GenCtx) -> Vec<u16> {
        Vec::new()
    }

    fn gen_stage(&mut self, _callback_id: u32, _stage: WorldgenStage, _ctx: &GenCtx) -> GenOutput {
        GenOutput::default()
    }

    fn gui_click(&mut self, _kind_key: &str, _widget_id: &str, _at: Option<ContainerAddress>) {}

    fn hostile_spawn_candidate(
        &mut self,
        _callback_id: u32,
        _candidate: &HostileSpawnCandidate,
    ) -> Option<String> {
        None
    }

    fn block_hook(&mut self, _callback_id: u32, _kind: BlockHookKind, _pos: [i32; 3]) {}

    /// One AI decision per mob per tick, for the node key registered via [`register_ai_node`].
    /// Decision only. Runs mid-mob-tick with no sim scope, so world edits, spawns and player state
    /// error here. [`current_tick`], [`rng_u64`] and [`log`] still work, and `ctx.tick` is already
    /// set. Extra fields only land in `ctx` if the brain row lists them under `"inputs"` (see
    /// [`AiNodeCtx`]). Return `None`, or leave fields default, for no opinion. The engine picks a
    /// winner per [`DecisionChannel`] by brain row priority. [`AiNodeDecision::claims`] holds a
    /// channel empty against lower nodes, e.g. `Attack` while fleeing.
    fn ai_node(&mut self, _callback_id: u32, _ctx: &AiNodeCtx) -> Option<AiNodeDecision> {
        None
    }

    fn client_frame(&mut self, _frame: &ClientFrameData) {}

    fn client_key(&mut self, _action_id: u32, _pressed: bool) {}

    fn client_ui(&mut self, _kind_key: &str, _event: &ClientUiEvent) {}

    fn client_canvas(&mut self, _canvas_key: &str, _event: &ClientCanvasEvent) {}

    fn client_canvas_scroll(&mut self, _canvas_key: &str, _x: f32, _y: f32, _delta: f32) {}

    fn bake_shape_sim(&mut self, _shape_kind: u16, _cells: &[CellInput]) -> Vec<BakedSimCell> {
        Vec::new()
    }

    fn bake_shape_render(
        &mut self,
        _shape_kind: u16,
        _cells: &[CellInput],
    ) -> Vec<BakedRenderCell> {
        Vec::new()
    }

    fn bake_shape_item(&mut self, _shape_kind: u16, _block: BlockId) -> BakedItemGeometry {
        BakedItemGeometry { boxes: Vec::new() }
    }

    /// Computes a custom shape's placement for one click. Reads world via [`get_block`], and any
    /// mutating host call errors here. Placement is single-cell and stateless.
    ///
    /// Runs on the server and on the client for the place ghost, so it must be a pure function of
    /// `inputs` and world reads. Then both sides compute the same write.
    ///
    /// Default accepts the click cell. Override to refuse, or to orient: a directional shape picks
    /// its sibling row off `inputs.normal` and returns it via [`ShapePlacementResult::block`]. The
    /// host only ever writes a row of this shape kind.
    fn shape_placement_plan(
        &mut self,
        _shape_kind: u16,
        _block: BlockId,
        inputs: &PlaceInputsView,
    ) -> ShapePlacementResult {
        ShapePlacementResult {
            accepted: true,
            anchor: inputs.place_pos,
            cells: vec![inputs.place_pos],
            block: None,
        }
    }
}

#[macro_export]
macro_rules! register_mod {
    ($ty:ty) => {
        static __PETRAMOND_MOD: $crate::__rt::ModSlot<$ty> = $crate::__rt::ModSlot::new();

        #[no_mangle]
        pub extern "C" fn mod_abi_version() -> u32 {
            $crate::ABI_VERSION.pack()
        }

        #[no_mangle]
        pub extern "C" fn mod_abi_requires() -> u64 {
            <$ty as $crate::Mod>::REQUIRES.bits()
        }

        #[no_mangle]
        pub extern "C" fn mod_init(host_abi: u32, host_caps: u64) {
            $crate::__rt::init(&__PETRAMOND_MOD, host_abi, host_caps)
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
