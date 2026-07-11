//! Calls available on every instance (any [`RuntimeSide`]): logging, the
//! tick clock, seeded RNG streams, plus the `mod_init` registration window
//! (tick systems, event handlers, spawners, block behaviors, AI nodes) and
//! shader parameters.

use mod_api::{AttachSide, EventKind, HostCall, HostRet, RuntimeSide, Stage};

// Imported for intra-doc links only.
#[allow(unused_imports)]
use crate::Mod;

use crate::__rt;

/// Log a line through the engine's logger.
pub fn log(msg: &str) {
    __rt::host_call(&HostCall::Log { msg: msg.into() });
}

/// The current game tick (20 per second).
pub fn current_tick() -> u64 {
    match __rt::host_call(&HostCall::CurrentTick) {
        HostRet::U64(tick) => tick,
        other => panic!("CurrentTick returned {other:?}"),
    }
}

/// Next value of the named deterministic RNG stream (seeded per world seed +
/// mod id + key; use distinct keys for independent streams).
pub fn rng_u64(stream_key: &str) -> u64 {
    let call = HostCall::RngU64 {
        stream_key: stream_key.into(),
    };
    match __rt::host_call(&call) {
        HostRet::U64(v) => v,
        other => panic!("RngU64 returned {other:?}"),
    }
}

/// Which isolated module runtime is executing this code.
pub fn runtime_side() -> RuntimeSide {
    match __rt::host_call(&HostCall::RuntimeSide) {
        HostRet::RuntimeSide(side) => side,
        other => panic!("RuntimeSide returned {other:?}"),
    }
}

/// Attach a tick system at a stage seam. Only legal during [`Mod::init`];
/// `system_id` is echoed to [`Mod::tick_system`]. Systems at one seam run in
/// `(priority ascending, registration order)`.
pub fn register_tick_system(stage: Stage, attach: AttachSide, priority: i32, system_id: u32) {
    __rt::expect_unit(
        "RegisterTickSystem",
        __rt::host_call(&HostCall::RegisterTickSystem {
            stage,
            attach,
            priority,
            system_id,
        }),
    );
}

/// Register an event handler. Only legal during [`Mod::init`]; `handler_id`
/// is echoed to [`Mod::handle_event`].
pub fn register_event_handler(event: EventKind, priority: i32, handler_id: u32) {
    __rt::expect_unit(
        "RegisterEventHandler",
        __rt::host_call(&HostCall::RegisterEventHandler {
            event,
            priority,
            handler_id,
        }),
    );
}

/// Register a callback that core may ask for hostile spawns. Only legal during
/// [`Mod::init`]; callbacks run in `(priority ascending, registration order)`.
pub fn register_hostile_spawner(priority: i32, callback_id: u32) {
    __rt::expect_unit(
        "RegisterHostileSpawner",
        __rt::host_call(&HostCall::RegisterHostileSpawner {
            callback_id,
            priority,
        }),
    );
}

/// Register the behavior handler for block rows whose `blocks.json`
/// `behavior` field is `key` (must be this mod's own `mod_id:name`). Only
/// legal during [`Mod::init`]; the engine echoes `callback_id` to
/// [`Mod::block_hook`] for every hook that fires on such a block.
pub fn register_block_behavior(key: &str, callback_id: u32) {
    __rt::expect_unit(
        "RegisterBlockBehavior",
        __rt::host_call(&HostCall::RegisterBlockBehavior {
            key: key.to_owned(),
            callback_id,
        }),
    );
}

/// Register the scripted AI node for `mobs.json` brain rows whose `node` key
/// is `key` (must be this mod's own `mod_id:name`). Only legal during
/// [`Mod::init`]; the engine echoes `callback_id` to [`Mod::ai_node`] once
/// per owning mob per game tick.
pub fn register_ai_node(key: &str, callback_id: u32) {
    __rt::expect_unit(
        "RegisterAiNode",
        __rt::host_call(&HostCall::RegisterAiNode {
            key: key.to_owned(),
            callback_id,
        }),
    );
}

/// Set one named visual shader parameter (`vec4<f32>`). `key` must be in this
/// mod's namespace (`mod_id:name`) or an exposed engine `petramond:*` key. Shader
/// packs map names onto fixed GPU slots.
pub fn shader_set_param(key: &str, value: [f32; 4]) {
    __rt::expect_unit(
        "ShaderSetParam",
        __rt::host_call(&HostCall::ShaderSetParam {
            key: key.into(),
            value,
        }),
    );
}
