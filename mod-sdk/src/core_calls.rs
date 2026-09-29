use mod_api::calls;
use mod_api::{AttachSide, EventFilter, EventKind, RuntimeSide, Stage};

#[allow(unused_imports)]
use crate::Mod;

use crate::__rt;
use crate::__rt::host_fn;
use crate::__rt::try_host_fn;

pub fn log(msg: &str) {
    let _ = __rt::call(&calls::Log { msg: msg.into() });
}

host_fn! {
    pub fn current_tick() -> u64 => CurrentTick => U64
}

host_fn! {
    pub fn rng_u64(stream_key: &str) -> u64 => RngU64 { stream_key: stream_key.into() } => U64
}

host_fn! {
    pub fn runtime_side() -> RuntimeSide => RuntimeSide => RuntimeSide
}

host_fn! {
    pub fn register_tick_system(stage: Stage, attach: AttachSide, priority: i32, system_id: u32)
        => RegisterTickSystem { stage, attach, priority, system_id }
}

pub fn register_event_handler(event: EventKind, priority: i32, handler_id: u32) {
    register_event_handler_filtered(event, priority, handler_id, EventFilter::default());
}

pub fn register_event_handler_filtered(
    event: EventKind,
    priority: i32,
    handler_id: u32,
    filter: EventFilter,
) {
    __rt::expect_value(
        "RegisterEventHandler",
        __rt::call(&calls::RegisterEventHandler {
            event,
            priority,
            handler_id,
            filter,
        })
        .decode_as(mod_api::ret_decode::Unit),
    );
}

host_fn! {
    pub fn register_hostile_spawner(priority: i32, callback_id: u32)
        => RegisterHostileSpawner { callback_id, priority }
}

host_fn! {
    pub fn register_block_behavior(key: &str, callback_id: u32)
        => RegisterBlockBehavior { key: key.into(), callback_id }
}

host_fn! {
    pub fn register_ai_node(key: &str, callback_id: u32)
        => RegisterAiNode { key: key.into(), callback_id }
}

host_fn! {
    pub fn shader_set_param(key: &str, value: [f32; 4])
        => ShaderSetParam { key: key.into(), value }
}

host_fn! {
    pub fn emit_event(key: &str, data: &[u8])
        => EmitEvent { key: key.into(), data: data.to_vec() }
}

try_host_fn! {
    pub fn try_emit_event(key: &str, data: &[u8])
        => EmitEvent { key: key.into(), data: data.to_vec() }
}

host_fn! {
    pub fn emit_event_to(player: mod_api::PlayerId, key: &str, data: &[u8]) -> bool
        => EmitEventTo { player, key: key.into(), data: data.to_vec() } => Bool
}
