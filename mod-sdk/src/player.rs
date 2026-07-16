//! Sim-scoped player calls: state, input, the damage funnel, knockback,
//! items, health, teleports, status effects, and chat delivery.

use mod_api::{EffectStateData, HostCall, HostRet, PlayerInputData, PlayerSnapshot};

use crate::__rt;

/// The horizontal direction a player yaw faces — PLAYER convention: yaw `0`
/// faces `+Z` (π apart from the mob convention, [`crate::mob_facing_xz`]);
/// a mount aligned with its rider takes `player_yaw + π` as its mob yaw.
pub fn player_facing_xz(yaw: f32) -> [f32; 2] {
    let (s, c) = yaw.sin_cos();
    [s, c]
}

/// The player's current state (position, velocity, look, health, flags).
pub fn player_state() -> PlayerSnapshot {
    match __rt::host_call(&HostCall::PlayerState) {
        HostRet::Player(p) => p,
        other => panic!("PlayerState returned {other:?}"),
    }
}

/// One player's movement intent this tick (forward/strafe in their own yaw
/// frame, jump/sneak, look) — how a vehicle mod reads what its driver is
/// pressing. `None` = no such player connected.
pub fn player_input(player_id: u8) -> Option<PlayerInputData> {
    match __rt::host_call(&HostCall::PlayerInput { player_id }) {
        HostRet::PlayerInput(input) => input,
        other => panic!("PlayerInput returned {other:?}"),
    }
}

/// Consume `count` units of the ACTING player's held stack, atomically, only
/// when it holds `item` with at least `count` — the spend primitive for item
/// uses that place no block (spawning an entity from an `item_use_pre`
/// handler). `false` = consumed nothing.
pub fn consume_held(item: mod_api::ItemId, count: u32) -> bool {
    match __rt::host_call(&HostCall::ConsumeHeld { item, count }) {
        HostRet::Bool(ok) => ok,
        other => panic!("ConsumeHeld returned {other:?}"),
    }
}

/// Damage the player through the engine funnel. The victim's global
/// engine-owned i-frames and `player_damage_pre` apply, with
/// `DamageSource::Mod` carrying this mod's id. Queued; applied at the next
/// in-tick drain point.
pub fn damage_player(amount: i32) {
    __rt::expect_unit(
        "DamagePlayer",
        __rt::host_call(&HostCall::DamagePlayer { amount }),
    );
}

/// Add a knockback impulse to the player's velocity (spectator no-op).
pub fn apply_knockback(impulse: [f32; 3]) {
    __rt::expect_unit(
        "ApplyKnockback",
        __rt::host_call(&HostCall::ApplyKnockback { impulse }),
    );
}

/// Give the player items through the normal inventory fill; overflow drops at
/// the player's feet. `false` = unknown item key.
pub fn give_item(item_key: &str, count: u8) -> bool {
    match __rt::host_call(&HostCall::GiveItem {
        item_key: item_key.into(),
        count,
    }) {
        HostRet::Bool(ok) => ok,
        other => panic!("GiveItem returned {other:?}"),
    }
}

/// Kill the player: current-health damage through the same funnel (and queue)
/// as [`damage_player`] — global i-frames or a pre-event handler can reject it.
pub fn kill_player() {
    __rt::expect_unit("KillPlayer", __rt::host_call(&HostCall::KillPlayer));
}

/// Overwrite the player's health (clamped to `0..=20` half-hearts), bypassing
/// the damage funnel — the heal/set primitive, no events fire.
pub fn set_health(value: i32) {
    __rt::expect_unit("SetHealth", __rt::host_call(&HostCall::SetHealth { value }));
}

/// Move the player's feet to `pos`; fall tracking is cleared so a teleport can
/// never land as fall damage.
pub fn teleport(pos: [f32; 3]) {
    __rt::expect_unit("Teleport", __rt::host_call(&HostCall::Teleport { pos }));
}

/// Grant the player the status effect `key` (an `effects.json` row — engine
/// `petramond:*` rows and every pack's rows alike) for `ticks` game ticks. An
/// already-active effect is overwritten with the new duration; `0` removes it.
/// A state primitive like [`set_health`] — no events fire. `false` = unknown
/// effect key.
pub fn effect_apply(key: &str, ticks: u32) -> bool {
    match __rt::host_call(&HostCall::EffectApply {
        key: key.into(),
        ticks,
    }) {
        HostRet::Bool(ok) => ok,
        other => panic!("EffectApply returned {other:?}"),
    }
}

/// Remove the status effect `key` from the player if active. `false` =
/// unknown effect key.
pub fn effect_remove(key: &str) -> bool {
    match __rt::host_call(&HostCall::EffectRemove { key: key.into() }) {
        HostRet::Bool(ok) => ok,
        other => panic!("EffectRemove returned {other:?}"),
    }
}

/// The player's active status effects, in application order.
pub fn effects_active() -> Vec<EffectStateData> {
    match __rt::host_call(&HostCall::EffectsActive) {
        HostRet::Effects(effects) => effects,
        other => panic!("EffectsActive returned {other:?}"),
    }
}

/// Deliver one server-authored chat line. `targets: None` broadcasts to every
/// currently connected client; `Some(ids)` sends only to those player ids
/// (unknown / left ids are ignored). Markup `$[fg=color]` is parsed by the
/// server. Empty / whitespace-only text returns `false`.
pub fn chat_send(text: &str, targets: Option<&[u8]>) -> bool {
    match __rt::host_call(&HostCall::ChatSend {
        text: text.into(),
        targets: targets.map(|ids| ids.to_vec()),
    }) {
        HostRet::Bool(ok) => ok,
        other => panic!("ChatSend returned {other:?}"),
    }
}
