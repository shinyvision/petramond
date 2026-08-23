//! Calls available on every instance (any [`RuntimeSide`]): logging, the
//! tick clock, seeded RNG streams, plus the `mod_init` registration window
//! (tick systems, event handlers, spawners, block behaviors, AI nodes) and
//! shader parameters.

use mod_api::{AttachSide, EventKind, HostCall, RuntimeSide, Stage};

// Imported for intra-doc links only.
#[allow(unused_imports)]
use crate::Mod;

use crate::__rt;
use crate::__rt::host_fn;

/// Log a line through the engine's logger.
pub fn log(msg: &str) {
    __rt::host_call(&HostCall::Log { msg: msg.into() });
}

host_fn! {
    /// The current game tick (20 per second).
    pub fn current_tick() -> u64 => CurrentTick => U64
}

host_fn! {
    /// Next value of the named deterministic RNG stream (seeded per world seed +
    /// mod id + key; use distinct keys for independent streams).
    pub fn rng_u64(stream_key: &str) -> u64 => RngU64 { stream_key: stream_key.into() } => U64
}

host_fn! {
    /// Which isolated module runtime is executing this code.
    pub fn runtime_side() -> RuntimeSide => RuntimeSide => RuntimeSide
}

host_fn! {
    /// Attach a tick system at a stage seam. Only legal during [`Mod::init`];
    /// `system_id` is echoed to [`Mod::tick_system`]. Systems at one seam run in
    /// `(priority ascending, registration order)`.
    pub fn register_tick_system(stage: Stage, attach: AttachSide, priority: i32, system_id: u32)
        => RegisterTickSystem { stage, attach, priority, system_id }
}

host_fn! {
    /// Register an event handler. Only legal during [`Mod::init`]; `handler_id`
    /// is echoed to [`Mod::handle_event`].
    pub fn register_event_handler(event: EventKind, priority: i32, handler_id: u32)
        => RegisterEventHandler { event, priority, handler_id }
}

host_fn! {
    /// Register a callback that core may ask for hostile spawns. Only legal during
    /// [`Mod::init`]; callbacks run in `(priority ascending, registration order)`.
    pub fn register_hostile_spawner(priority: i32, callback_id: u32)
        => RegisterHostileSpawner { callback_id, priority }
}

host_fn! {
    /// Register the behavior handler for block rows whose `blocks.json`
    /// `behavior` field is `key` (must be this mod's own `mod_id:name`). Only
    /// legal during [`Mod::init`]; the engine echoes `callback_id` to
    /// [`Mod::block_hook`] for every hook that fires on such a block.
    pub fn register_block_behavior(key: &str, callback_id: u32)
        => RegisterBlockBehavior { key: key.into(), callback_id }
}

host_fn! {
    /// Register the scripted AI node for `mobs.json` brain rows whose `node` key
    /// is `key` (must be this mod's own `mod_id:name`). Only legal during
    /// [`Mod::init`]; the engine echoes `callback_id` to [`Mod::ai_node`] once
    /// per owning mob per game tick.
    pub fn register_ai_node(key: &str, callback_id: u32)
        => RegisterAiNode { key: key.into(), callback_id }
}

host_fn! {
    /// Set one named visual shader parameter (`vec4<f32>`). `key` must be in this
    /// mod's namespace (`mod_id:name`) or an exposed engine `petramond:*` key. Shader
    /// packs map names onto fixed GPU slots.
    pub fn shader_set_param(key: &str, value: [f32; 4])
        => ShaderSetParam { key: key.into(), value }
}

host_fn! {
    /// Emit one of this mod's OWN events. `key` must carry this mod's
    /// `mod_id:` prefix; `data` is this mod's opaque payload. The event queues
    /// and dispatches at the next post-event drain point in the same tick —
    /// every handler registered for [`EventKind::ModEvent`] receives it, so
    /// the key is what a handler filters on.
    ///
    /// [`EventKind::ModEvent`]: mod_api::EventKind::ModEvent
    pub fn emit_event(key: &str, data: &[u8])
        => EmitEvent { key: key.into(), data: data.to_vec() }
}

host_fn! {
    /// Deliver one of this mod's OWN events to `player`'s CLIENT instance —
    /// [`emit_event`] across the wire. Same key rule, same payload cap; the
    /// client half receives it as an ordinary [`EventKind::ModEvent`]
    /// dispatch. `false` = no such reachable session.
    ///
    /// Reach for it when the CLIENT has to present something only the server
    /// could know: a client mod predicts what local input implies and nothing
    /// else, so a hit landing or a timer expiring is invisible to it. Send the
    /// EDGE and let the client run its own envelope. The cue arrives with the
    /// next batch, so keep the RULE on the server — this lane is presentation,
    /// never simulation.
    ///
    /// [`EventKind::ModEvent`]: mod_api::EventKind::ModEvent
    pub fn emit_event_to(player: mod_api::PlayerId, key: &str, data: &[u8]) -> bool
        => EmitEventTo { player, key: key.into(), data: data.to_vec() } => Bool
}
