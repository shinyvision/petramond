//! Player calls: state snapshot, the damage funnel, knockback, items,
//! health, teleports, status effects, and chat delivery.

use mod_api::{HostCall, HostRet, PlayerSnapshot};

use crate::events::ModAction;

use super::entities::give_item;
use super::guards::{finite3, item_by_key, sim_call, sim_query};
use super::intern_mod_id;

/// Phase 3b: player (snapshot, damage/kill through the funnel, inventory,
/// movement primitives).
pub(super) fn handle_player_call(mod_id: &str, call: HostCall) -> HostRet {
    match call {
        HostCall::PlayerState => sim_query(|ctx| {
            let p = &*ctx.player;
            HostRet::Player(PlayerSnapshot {
                pos: p.pos.to_array(),
                vel: p.vel.to_array(),
                yaw: p.yaw,
                pitch: p.pitch,
                health: p.health(),
                on_ground: p.on_ground,
                spectator: p.is_spectator(),
            })
        }),
        HostCall::DamagePlayer { amount } => {
            let mod_id = intern_mod_id(mod_id);
            sim_call(|ctx| {
                ctx.queue
                    .push_action(ModAction::DamagePlayer { amount, mod_id })
            })
        }
        HostCall::ApplyKnockback { impulse } => match finite3(impulse, "ApplyKnockback.impulse") {
            Err(e) => e,
            Ok(impulse) => sim_call(|ctx| ctx.player.apply_knockback(impulse)),
        },
        HostCall::GiveItem { item_key, count } => sim_query(|ctx| {
            let Some(item) = item_by_key(&item_key) else {
                log::warn!("[mod {mod_id}] GiveItem: unknown item '{item_key}'");
                return HostRet::Bool(false);
            };
            give_item(ctx, item, count);
            HostRet::Bool(true)
        }),
        // Atomic: only a selected stack holding at least `count` of `item`
        // consumes — the held stack IS the validation, so no registry check.
        HostCall::ConsumeHeld { item, count } => sim_query(|ctx| {
            let holds = count > 0
                && ctx
                    .player
                    .inventory
                    .selected()
                    .is_some_and(|st| st.item.0 == item.0 && st.count as u32 >= count);
            if !holds {
                return HostRet::Bool(false);
            }
            for _ in 0..count {
                ctx.player.inventory.decrement_selected();
            }
            HostRet::Bool(true)
        }),
        HostCall::PlayerInput { player_id } => sim_query(|ctx| {
            HostRet::PlayerInput(ctx.world.player_input(player_id).map(|i| {
                mod_api::PlayerInputData {
                    forward: i.forward,
                    strafe: i.strafe,
                    jump: i.jump,
                    sneak: i.sneak,
                    yaw: i.yaw,
                    pitch: i.pitch,
                }
            }))
        }),
        HostCall::KillPlayer => {
            let mod_id = intern_mod_id(mod_id);
            sim_call(|ctx| ctx.queue.push_action(ModAction::KillPlayer { mod_id }))
        }
        HostCall::SetHealth { value } => sim_call(|ctx| ctx.player.set_health(value)),
        HostCall::Teleport { pos } => match finite3(pos, "Teleport.pos") {
            Err(e) => e,
            Ok(pos) => sim_call(|ctx| ctx.player.teleport(pos)),
        },
        // Status effects are player-state primitives like SetHealth: direct
        // mutation, no events. Unknown keys are forgiving (Bool(false)) — a
        // typo'd key is not a protocol break.
        HostCall::EffectApply { key, ticks } => sim_query(|ctx| {
            let Some(effect) = crate::effect::by_name(&key) else {
                log::warn!("[mod {mod_id}] EffectApply: unknown effect '{key}'");
                return HostRet::Bool(false);
            };
            ctx.player.apply_effect(effect, ticks);
            HostRet::Bool(true)
        }),
        HostCall::EffectRemove { key } => sim_query(|ctx| {
            let Some(effect) = crate::effect::by_name(&key) else {
                log::warn!("[mod {mod_id}] EffectRemove: unknown effect '{key}'");
                return HostRet::Bool(false);
            };
            ctx.player.remove_effect(effect);
            HostRet::Bool(true)
        }),
        HostCall::EffectsActive => sim_query(|ctx| {
            HostRet::Effects(
                ctx.player
                    .effects()
                    .iter()
                    .map(|e| mod_api::EffectStateData {
                        key: e.effect.def().name.to_owned(),
                        remaining: e.remaining,
                    })
                    .collect(),
            )
        }),
        HostCall::ChatSend { text, targets } => sim_query(|ctx| {
            // Empty / whitespace-only text is rejected at delivery time too;
            // report it here so the mod can tell a no-op from a queued send.
            if text.trim().is_empty() {
                return HostRet::Bool(false);
            }
            ctx.queue.push_action(ModAction::ChatSend { text, targets });
            HostRet::Bool(true)
        }),
        other => HostRet::Error(format!(
            "non-player call {other:?} mis-routed to handle_player_call (host bug)"
        )),
    }
}
