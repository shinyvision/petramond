//! Player calls: state snapshot, the damage funnel, knockback, items,
//! health, teleports, status effects, and chat delivery. (There is no kill
//! call: `DamagePlayer` with current health is the kill, same funnel.)

use mod_api::{HostCall, HostRet, PlayerSnapshot};

use crate::events::ModAction;
use crate::item::ItemStack;

use super::entities::give_item;
use super::guards::{batch_guard, finite3, item_by_name, sim_call, sim_query};
use super::intern_mod_id;

/// The pose anchor a player is pinned at, read LIVE from the riding registry
/// (not the start-of-tick roster) so an occupancy check made right after a
/// same-tick `PlayerPoseSet` already sees the seat taken.
fn pose_anchor_of(world: &crate::world::World, id: u8) -> Option<[f32; 3]> {
    match world.riding().mount_of(id)?.target {
        crate::mob::riding::MountTarget::Anchor(a) => Some(a.pos.to_array()),
        crate::mob::riding::MountTarget::Mob(_) => None,
    }
}

/// Player calls (snapshot, damage/kill through the funnel, inventory,
/// movement primitives).
pub(super) fn handle_player_call(mod_id: &str, call: HostCall) -> HostRet {
    match call {
        HostCall::PlayerState => sim_query(|ctx| {
            // Sneak lives on the session, not the `Player` body: read the
            // acting session's published roster row (same tick, same intent
            // latches). No roster published (mod init, unit fixtures) reads
            // as not sneaking.
            let sneak = ctx
                .acting_player_id()
                .and_then(|id| {
                    ctx.world
                        .player_roster()
                        .iter()
                        .find(|r| r.id == id.0)
                        .map(|r| r.sneak)
                })
                .unwrap_or(false);
            let p = &*ctx.player;
            HostRet::Player(PlayerSnapshot {
                pos: p.pos.to_array(),
                vel: p.vel.to_array(),
                yaw: p.yaw,
                pitch: p.pitch,
                health: p.health(),
                on_ground: p.on_ground,
                spectator: p.is_spectator(),
                sneak,
                held: p
                    .inventory
                    .selected()
                    .map(|st| mod_api::ItemId(st.item.id())),
                held_count: p.inventory.selected().map_or(0, |st| st.count),
                pose_anchor: ctx
                    .acting_player_id()
                    .and_then(|id| pose_anchor_of(ctx.world, id.0)),
            })
        }),
        HostCall::Players => sim_query(|ctx| {
            HostRet::Players(
                ctx.world
                    .player_roster()
                    .iter()
                    .map(|p| mod_api::PlayerListEntry {
                        id: mod_api::PlayerId(p.id),
                        state: PlayerSnapshot {
                            pos: p.pos,
                            vel: p.vel,
                            yaw: p.yaw,
                            pitch: p.pitch,
                            health: p.health,
                            on_ground: p.on_ground,
                            spectator: p.spectator,
                            sneak: p.sneak,
                            held: p.held.map(|i| mod_api::ItemId(i.id())),
                            held_count: p.held_count,
                            pose_anchor: pose_anchor_of(ctx.world, p.id),
                        },
                    })
                    .collect(),
            )
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
        HostCall::GiveItem { item, count } => sim_query(|ctx| {
            let Some(item_ty) = item_by_name(&item) else {
                log::warn!("[mod {mod_id}] GiveItem: unknown item '{item}'");
                return HostRet::Bool(false);
            };
            give_item(ctx, item_ty, count);
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
        HostCall::ReplaceHeldOne { item, replacement } => sim_query(|ctx| {
            let holds = ctx
                .player
                .inventory
                .selected()
                .is_some_and(|st| st.item.0 == item.0 && st.count >= 1);
            if !holds {
                return HostRet::Bool(false);
            }
            let Some(replacement_ty) = item_by_name(&replacement) else {
                log::warn!("[mod {mod_id}] ReplaceHeldOne: unknown item '{replacement}'");
                return HostRet::Bool(false);
            };
            let ok = ctx
                .player
                .inventory
                .replace_selected_one(ItemStack::new(replacement_ty, 1));
            HostRet::Bool(ok)
        }),
        HostCall::PlayerInput { player_id } => sim_query(|ctx| {
            HostRet::PlayerInput(ctx.world.player_input(player_id.0).map(|i| {
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
            if let Some(err) = batch_guard("ChatSend target", targets.as_ref().map_or(0, Vec::len))
            {
                return err;
            }
            // Empty / whitespace-only text is rejected at delivery time too;
            // report it here so the mod can tell a no-op from a queued send.
            if text.trim().is_empty() {
                return HostRet::Bool(false);
            }
            let targets = targets.map(|ids| ids.into_iter().map(|p| p.0).collect());
            ctx.queue.push_action(ModAction::ChatSend { text, targets });
            HostRet::Bool(true)
        }),
        other => HostRet::Error(format!(
            "non-player call {other:?} mis-routed to handle_player_call (host bug)"
        )),
    }
}
