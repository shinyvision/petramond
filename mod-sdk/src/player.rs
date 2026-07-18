//! Sim-scoped player calls: state, input, the damage funnel, knockback,
//! items, health, teleports, status effects, and chat delivery.

use mod_api::{EffectStateData, PlayerId, PlayerInputData, PlayerSnapshot};

use crate::__rt::host_fn;

/// The horizontal direction a player yaw faces — PLAYER convention: yaw `0`
/// faces `+Z` (π apart from the mob convention, [`crate::mob_facing_xz`]);
/// a mount aligned with its rider takes `player_yaw + π` as its mob yaw.
pub fn player_facing_xz(yaw: f32) -> [f32; 2] {
    let (s, c) = yaw.sin_cos();
    [s, c]
}

host_fn! {
    /// The player's current state (position, velocity, look, health, flags).
    pub fn player_state() -> PlayerSnapshot => PlayerState => Player
}

host_fn! {
    /// One player's movement intent this tick (forward/strafe in their own yaw
    /// frame, jump/sneak, look) — how a vehicle mod reads what its driver is
    /// pressing. `None` = no such player connected.
    pub fn player_input(player_id: PlayerId) -> Option<PlayerInputData>
        => PlayerInput { player_id } => PlayerInput
}

host_fn! {
    /// Consume `count` units of the ACTING player's held stack, atomically, only
    /// when it holds `item` with at least `count` — the spend primitive for item
    /// uses that place no block (spawning an entity from an `item_use_pre`
    /// handler). `false` = consumed nothing.
    pub fn consume_held(item: mod_api::ItemId, count: u32) -> bool
        => ConsumeHeld { item, count } => Bool
}

host_fn! {
    /// Damage the player through the engine funnel. The victim's global
    /// engine-owned i-frames and `player_damage_pre` apply, with
    /// `DamageSource::Mod` carrying this mod's id. Queued; applied at the next
    /// in-tick drain point.
    ///
    /// To KILL the player, pass their current health ([`player_state`]) as
    /// `amount` — same funnel; i-frames or a pre-event handler can still
    /// reject it. There is no separate kill call.
    pub fn damage_player(amount: i32) => DamagePlayer { amount }
}

host_fn! {
    /// Add a knockback impulse to the player's velocity (spectator no-op).
    pub fn apply_knockback(impulse: [f32; 3]) => ApplyKnockback { impulse }
}

host_fn! {
    /// Give the player items (by registry NAME) through the normal inventory
    /// fill; overflow drops at the player's feet. `false` = unknown item name.
    pub fn give_item(item: &str, count: u8) -> bool
        => GiveItem { item: item.into(), count } => Bool
}

host_fn! {
    /// Overwrite the player's health (clamped to `0..=20` half-hearts), bypassing
    /// the damage funnel — the heal/set primitive, no events fire.
    pub fn set_health(value: i32) => SetHealth { value }
}

host_fn! {
    /// Move the player's feet to `pos`; fall tracking is cleared so a teleport can
    /// never land as fall damage.
    pub fn teleport(pos: [f32; 3]) => Teleport { pos }
}

host_fn! {
    /// Grant the player the status effect `key` (an `effects.json` row — engine
    /// `petramond:*` rows and every pack's rows alike) for `ticks` game ticks. An
    /// already-active effect is overwritten with the new duration; `0` removes it.
    /// A state primitive like [`set_health`] — no events fire. `false` = unknown
    /// effect key.
    pub fn effect_apply(key: &str, ticks: u32) -> bool
        => EffectApply { key: key.into(), ticks } => Bool
}

host_fn! {
    /// Remove the status effect `key` from the player if active. `false` =
    /// unknown effect key.
    pub fn effect_remove(key: &str) -> bool => EffectRemove { key: key.into() } => Bool
}

host_fn! {
    /// The player's active status effects, in application order.
    pub fn effects_active() -> Vec<EffectStateData> => EffectsActive => Effects
}

host_fn! {
    /// Deliver one server-authored chat line. `targets: None` broadcasts to every
    /// currently connected client; `Some(ids)` sends only to those player ids
    /// (unknown / left ids are ignored). Markup `$[fg=color]` is parsed by the
    /// server. Empty / whitespace-only text returns `false`.
    pub fn chat_send(text: &str, targets: Option<&[PlayerId]>) -> bool
        => ChatSend {
            text: text.into(),
            targets: targets.map(|ids| ids.to_vec()),
        } => Bool
}

host_fn! {
    /// Every connected player this tick, in session-id order (single player =
    /// one entry) — the multiplayer-aware roster for spawn/ambience/weather
    /// policy. Address a specific player through the entry's `id`.
    pub fn players() -> Vec<mod_api::PlayerListEntry> => Players => Players
}
