use mod_api::{BodyCall, HostRet, PlayerCall, PlayerSnapshot};

use crate::events::{DeferredAction, SimCtx};
use crate::player::PlayerId;
use petramond_world::item::variant::{self, VariantMap};
use petramond_world::item::{ItemStack, ItemType};

use super::entities::give_item_to;
use super::guards::{
    actor_for, batch_guard, finite3, item_by_name, sim_mutate, sim_query, sim_read,
};
use super::intern_mod_id;

fn pose_anchor_of(world: &crate::world::ServerWorld, id: u8) -> Option<[f64; 3]> {
    match world.riding().mount_of(id)?.target {
        crate::mob::riding::MountTarget::Anchor(a) => Some(a.pos.to_array()),
        crate::mob::riding::MountTarget::Mob(_) => None,
    }
}

fn player_snapshot(ctx: &mut SimCtx<'_>, id: PlayerId, mod_id: &str) -> Option<PlayerSnapshot> {
    let (sneak, use_held, swing) = ctx
        .world
        .player_roster()
        .iter()
        .find(|r| r.id == id.0)
        .map(|r| (r.sneak, r.use_held, r.swing))
        .unwrap_or_default();
    let pose_anchor = pose_anchor_of(ctx.world, id.0);
    ctx.with_player(id, |p| PlayerSnapshot {
        id: Some(mod_api::PlayerId(id.0)),
        pos: p.pos.to_array(),
        vel: p.vel.to_array(),
        yaw: p.yaw,
        pitch: p.pitch,
        health: p.health(),
        on_ground: p.on_ground,
        spectator: p.is_spectator(),
        sneak,
        use_held,
        holds_use: p.use_gesture.held_by(mod_id),
        held: p.held().map(|st| mod_api::ItemId(st.item.id())),
        held_count: p.held().map_or(0, |st| st.count),
        off_held: p
            .inventory
            .off_hand()
            .map(|st| mod_api::ItemId(st.item.id())),
        pose_anchor,
        swing,
        half_width: crate::player::HALF_W,
        height: crate::player::HEIGHT,
        eye_height: crate::player::EYE,
        entombed: p.entombed(),
        conditions: crate::exposure::condition_data(p.conditions()),
    })
}

fn effects_of(ctx: &mut SimCtx<'_>, id: PlayerId) -> Option<Vec<mod_api::EffectStateData>> {
    ctx.with_player(id, |p| {
        p.effects()
            .iter()
            .map(|e| mod_api::EffectStateData {
                key: e.effect.def().name.to_owned(),
                remaining: e.remaining,
            })
            .collect()
    })
}

fn apply_effect(ctx: &mut SimCtx<'_>, mod_id: &str, id: PlayerId, key: &str, ticks: u32) -> bool {
    let Some(effect) = petramond_world::effect::by_name(key) else {
        log::warn!("[mod {mod_id}] EffectApply: unknown effect '{key}'");
        return false;
    };
    ctx.with_player(id, |p| p.apply_effect(effect, ticks))
        .is_some()
}

fn consume_held(ctx: &mut SimCtx<'_>, id: PlayerId, item: mod_api::ItemId, count: u32) -> bool {
    ctx.with_player(id, |p| {
        let hand = p.acting_hand;
        let holds = count > 0
            && p.held()
                .is_some_and(|st| st.item.0 == item.0 && st.count as u32 >= count);
        if holds {
            for _ in 0..count {
                p.inventory.decrement_held(hand);
            }
        }
        holds
    })
    .unwrap_or(false)
}

fn replace_held_one(
    ctx: &mut SimCtx<'_>,
    mod_id: &str,
    id: PlayerId,
    item: mod_api::ItemId,
    replacement: &str,
) -> bool {
    let Some(replacement_ty) = item_by_name(replacement) else {
        log::warn!("[mod {mod_id}] ReplaceHeldOne: unknown item '{replacement}'");
        return false;
    };
    ctx.with_player(id, |p| {
        let hand = p.acting_hand;
        p.held()
            .is_some_and(|st| st.item.0 == item.0 && st.count >= 1)
            && p.inventory
                .replace_held_one(hand, ItemStack::new(replacement_ty, 1))
    })
    .unwrap_or(false)
}

fn player_input(ctx: &SimCtx<'_>, player: mod_api::PlayerId) -> Option<mod_api::PlayerInputData> {
    ctx.world
        .player_input(player.0)
        .map(|i| mod_api::PlayerInputData {
            forward: i.forward,
            strafe: i.strafe,
            jump: i.jump,
            sneak: i.sneak,
            yaw: i.yaw,
            pitch: i.pitch,
        })
}

pub(super) fn handle_player_call(mod_id: &str, call: PlayerCall) -> HostRet {
    match call {
        PlayerCall::PlayerStateOf { player } => sim_query(|ctx| {
            HostRet::PlayerOf(player_snapshot(ctx, PlayerId(player.0), mod_id).map(Box::new))
        }),
        PlayerCall::PlayerIdentity { player } => sim_read(|ctx| {
            HostRet::Identity(
                ctx.world
                    .player_roster()
                    .iter()
                    .find(|p| p.id == player.0)
                    .map(|p| mod_api::PlayerIdentityData {
                        name: p.name.clone(),
                        operator: p.operator,
                    }),
            )
        }),
        PlayerCall::Players => sim_query(|ctx| {
            let entries = ctx
                .player_ids()
                .into_iter()
                .filter_map(|id| {
                    player_snapshot(ctx, id, mod_id).map(|state| mod_api::PlayerListEntry {
                        id: mod_api::PlayerId(id.0),
                        state,
                    })
                })
                .collect();
            HostRet::Players(entries)
        }),
        PlayerCall::DamagePlayer {
            player,
            amount,
            origin,
            attacker,
        } => match origin
            .map(|p| super::guards::finite_pos(p, "DamagePlayer.origin"))
            .transpose()
        {
            Err(e) => e,
            Ok(origin) => {
                let mod_id = intern_mod_id(mod_id);
                sim_mutate(|ctx| {
                    let source =
                        super::entities::attack_source(ctx, mod_id, attacker, "DamagePlayer")?;
                    ctx.queue.push_action(DeferredAction::DamagePlayer {
                        player: crate::player::PlayerId(player.0),
                        amount,
                        source,
                        origin,
                    });
                    Ok(())
                })
            }
        },
        PlayerCall::ApplyKnockback { impulse } => {
            match finite3(impulse, "ApplyKnockback.impulse") {
                Err(e) => e,
                Ok(impulse) => sim_mutate(|ctx| {
                    let id = actor_for(ctx, "ApplyKnockback", "ApplyKnockbackTo")?;
                    ctx.with_player(id, |p| p.apply_knockback(impulse));
                    Ok(())
                }),
            }
        }
        PlayerCall::ApplyKnockbackTo { player, impulse } => {
            match finite3(impulse, "ApplyKnockbackTo.impulse") {
                Err(e) => e,
                Ok(impulse) => sim_query(|ctx| {
                    let hit = ctx.with_player(PlayerId(player.0), |p| p.apply_knockback(impulse));
                    HostRet::Bool(hit.is_some())
                }),
            }
        }
        PlayerCall::GiveItem { item, count, data } => {
            let variant = match super::guards::intern_abi_data("GiveItem", &data) {
                Ok(v) => v,
                Err(e) => return e,
            };
            sim_query(|ctx| {
                let id = match actor_for(ctx, "GiveItem", "GiveItemTo") {
                    Ok(id) => id,
                    Err(e) => return e,
                };
                let Some(item_ty) = item_by_name(&item) else {
                    log::warn!("[mod {mod_id}] GiveItem: unknown item '{item}'");
                    return HostRet::Bool(false);
                };
                give_item_to(ctx, id, item_ty, count, variant);
                HostRet::Bool(true)
            })
        }
        PlayerCall::GiveItemTo {
            player,
            item,
            count,
            data,
        } => {
            let variant = match super::guards::intern_abi_data("GiveItemTo", &data) {
                Ok(v) => v,
                Err(e) => return e,
            };
            sim_query(move |ctx| {
                let Some(item_ty) = item_by_name(&item) else {
                    log::warn!("[mod {mod_id}] GiveItemTo: unknown item '{item}'");
                    return HostRet::Bool(false);
                };
                let id = crate::player::PlayerId(player.0);
                HostRet::Bool(give_item_to(ctx, id, item_ty, count, variant))
            })
        }
        PlayerCall::PlayerHeld { player } => sim_query(move |ctx| {
            let id = crate::player::PlayerId(player.0);
            HostRet::HeldStack(
                ctx.with_player(id, |p| p.held().copied())
                    .flatten()
                    .map(super::guards::item_stack_data),
            )
        }),
        PlayerCall::SetPlayerHeldData {
            player,
            expect_item,
            expect_data,
            data,
        } => {
            let variant = match super::guards::intern_abi_data("SetPlayerHeldData", &data) {
                Ok(v) => v,
                Err(e) => return e,
            };
            // The EXPECTATION is compared, never interned. The two maps are
            // deliberately treated differently: the replacement above is one
            // the mod means to write, so a malformed one is a loud error
            // whatever the compare answers, and re-interning it is free (rows
            // are keyed by blob, so a retry reuses the row). The expectation
            // is whatever the stack happened to hold and VARIES per attempt —
            // interning that would let a caller losing a CAS in a loop mint a
            // fresh permanent row every time, and the table never evicts.
            let expect_map: VariantMap = expect_data.into_iter().collect();
            sim_query(move |ctx| {
                let Some(expect_ty) = item_by_name(&expect_item) else {
                    log::warn!("[mod {mod_id}] SetPlayerHeldData: unknown item '{expect_item}'");
                    return HostRet::Bool(false);
                };
                let id = crate::player::PlayerId(player.0);
                let ok = ctx
                    .with_player(id, |p| {
                        let hand = p.acting_hand;
                        match p.held().copied() {
                            Some(st)
                                if st.item == expect_ty
                                    && variant::matches(st.variant, &expect_map) =>
                            {
                                let stamped = ItemStack::with_variant(st.item, st.count, variant);
                                match hand {
                                    petramond_world::inventory::Hand::Main => {
                                        let active = p.inventory.active_slot() as usize;
                                        p.inventory
                                            .slot_mut(active)
                                            .map(|slot| {
                                                *slot = Some(stamped);
                                                true
                                            })
                                            .unwrap_or(false)
                                    }
                                    petramond_world::inventory::Hand::Off => {
                                        *p.inventory.off_hand_mut() = Some(stamped);
                                        true
                                    }
                                }
                            }
                            _ => false,
                        }
                    })
                    .unwrap_or(false);
                HostRet::Bool(ok)
            })
        }
        PlayerCall::ConsumeHeld { item, count } => {
            sim_query(|ctx| match actor_for(ctx, "ConsumeHeld", "ConsumeHeldBy") {
                Ok(id) => HostRet::Bool(consume_held(ctx, id, item, count)),
                Err(e) => e,
            })
        }
        PlayerCall::ConsumeHeldBy {
            player,
            item,
            count,
        } => sim_query(|ctx| HostRet::Bool(consume_held(ctx, PlayerId(player.0), item, count))),
        PlayerCall::ReplaceHeldOne { item, replacement } => {
            sim_query(
                |ctx| match actor_for(ctx, "ReplaceHeldOne", "ReplaceHeldOneBy") {
                    Ok(id) => HostRet::Bool(replace_held_one(ctx, mod_id, id, item, &replacement)),
                    Err(e) => e,
                },
            )
        }
        PlayerCall::ReplaceHeldOneBy {
            player,
            item,
            replacement,
        } => sim_query(|ctx| {
            let id = PlayerId(player.0);
            HostRet::Bool(replace_held_one(ctx, mod_id, id, item, &replacement))
        }),
        PlayerCall::PlayerInput { player_id } => {
            sim_read(|ctx| HostRet::PlayerInput(player_input(ctx, player_id)))
        }
        PlayerCall::PlayerInputs { player_ids } => {
            if let Some(err) = batch_guard("PlayerInputs player", player_ids.len()) {
                return err;
            }
            sim_read(|ctx| {
                HostRet::PlayerInputs(player_ids.iter().map(|&id| player_input(ctx, id)).collect())
            })
        }
        PlayerCall::SetHealth { value } => sim_mutate(|ctx| {
            let id = actor_for(ctx, "SetHealth", "SetHealthOf")?;
            ctx.with_player(id, |p| p.set_health(value));
            Ok(())
        }),
        PlayerCall::SetHealthOf { player, value } => sim_query(|ctx| {
            let hit = ctx.with_player(PlayerId(player.0), |p| p.set_health(value));
            HostRet::Bool(hit.is_some())
        }),
        PlayerCall::Teleport { pos } => match super::guards::finite_pos(pos, "Teleport.pos") {
            Err(e) => e,
            Ok(pos) => sim_mutate(|ctx| {
                let id = actor_for(ctx, "Teleport", "TeleportPlayer")?;
                ctx.with_player(id, |p| p.teleport(pos));
                Ok(())
            }),
        },
        PlayerCall::TeleportPlayer { player, pos } => {
            match super::guards::finite_pos(pos, "TeleportPlayer.pos") {
                Err(e) => e,
                Ok(pos) => sim_query(|ctx| {
                    let hit = ctx.with_player(PlayerId(player.0), |p| p.teleport(pos));
                    HostRet::Bool(hit.is_some())
                }),
            }
        }
        PlayerCall::EffectApply { key, ticks } => {
            sim_query(|ctx| match actor_for(ctx, "EffectApply", "EffectApplyTo") {
                Ok(id) => HostRet::Bool(apply_effect(ctx, mod_id, id, &key, ticks)),
                Err(e) => e,
            })
        }
        PlayerCall::EffectApplyTo { player, key, ticks } => sim_query(|ctx| {
            HostRet::Bool(apply_effect(ctx, mod_id, PlayerId(player.0), &key, ticks))
        }),
        PlayerCall::EffectsActive => {
            sim_query(
                |ctx| match actor_for(ctx, "EffectsActive", "EffectsActiveOf") {
                    Ok(id) => HostRet::Effects(effects_of(ctx, id).unwrap_or_default()),
                    Err(e) => e,
                },
            )
        }
        PlayerCall::EffectsActiveOf { player } => {
            sim_query(|ctx| HostRet::EffectsOf(effects_of(ctx, PlayerId(player.0))))
        }
        PlayerCall::SetPlayerAttribute {
            player,
            attribute,
            scale,
        } => {
            let mod_id = mod_id.to_owned();
            sim_query(move |ctx| {
                match ctx.with_player(crate::player::PlayerId(player.0), |p| {
                    p.claims.set_attribute(&mod_id, attribute, scale)
                }) {
                    None => HostRet::Bool(false),
                    Some(true) => HostRet::Bool(true),
                    Some(false) => {
                        HostRet::invalid(format!("SetPlayerAttribute: {scale} is not finite"))
                    }
                }
            })
        }
        PlayerCall::TakeItem {
            player,
            item,
            count,
            data,
        } => {
            let data = match data {
                None => None,
                Some(data) => match super::guards::abi_data_map("TakeItem", &data) {
                    Ok(map) => Some(map),
                    Err(e) => return e,
                },
            };
            sim_query(move |ctx| {
                let Some(item_ty) = item_by_name(&item) else {
                    log::warn!("[mod {mod_id}] TakeItem: unknown item '{item}'");
                    return HostRet::ItemStack(None);
                };
                let taken = ctx
                    .with_player(crate::player::PlayerId(player.0), |p| {
                        p.inventory.take(item_ty, count, data.as_ref())
                    })
                    .flatten();
                HostRet::ItemStack(taken.map(super::guards::item_stack_data))
            })
        }
        PlayerCall::SetPlayerDeniedActions { player, actions } => {
            let mod_id = mod_id.to_owned();
            let denied = crate::player::DeniedActions::of(actions);
            sim_query(move |ctx| {
                let wrote = ctx.with_player(crate::player::PlayerId(player.0), |p| {
                    p.claims.set_denied_actions(&mod_id, denied);
                });
                HostRet::Bool(wrote.is_some())
            })
        }
        PlayerCall::ChatSend { text, targets } => sim_query(|ctx| {
            if let Some(err) = batch_guard("ChatSend target", targets.as_ref().map_or(0, Vec::len))
            {
                return err;
            }
            if text.trim().is_empty() {
                return HostRet::Bool(false);
            }
            let targets = targets.map(|ids| ids.into_iter().map(|p| p.0).collect());
            ctx.queue
                .push_action(DeferredAction::ChatSend { text, targets });
            HostRet::Bool(true)
        }),
        PlayerCall::UnlockRecipe { player, recipe } => sim_query(|ctx| {
            let known = crate::modding::active_recipes()
                .is_some_and(|recipes| recipes.crafting().get(&recipe).is_some());
            if !known {
                log::warn!("[mod {mod_id}] UnlockRecipe: no crafting recipe '{recipe}'");
                return HostRet::Bool(false);
            }
            match ctx.with_player(crate::player::PlayerId(player.0), |p| {
                p.progression.unlock(&recipe)
            }) {
                Some(unlocked) => HostRet::Bool(unlocked),
                None => {
                    log::warn!(
                        "[mod {mod_id}] UnlockRecipe '{recipe}': player {} is not connected",
                        player.0
                    );
                    HostRet::Bool(false)
                }
            }
        }),
        PlayerCall::RecipeUnlocked { player, recipe } => sim_query(|ctx| {
            let unlocked = ctx
                .with_player(crate::player::PlayerId(player.0), |p| {
                    p.progression.is_unlocked(&recipe)
                })
                .unwrap_or(false);
            HostRet::Bool(unlocked)
        }),
    }
}

pub(super) fn handle_body_call(mod_id: &str, call: BodyCall) -> HostRet {
    match call {
        BodyCall::ActingPlayer => {
            sim_read(|ctx| HostRet::ActingPlayer(ctx.actor.map(|id| mod_api::PlayerId(id.0))))
        }
        BodyCall::PlayerState => sim_query(|ctx| {
            let id = match actor_for(ctx, "PlayerState", "PlayerStateOf") {
                Ok(id) => id,
                Err(e) => return e,
            };
            match player_snapshot(ctx, id, mod_id) {
                Some(snapshot) => HostRet::Player(Box::new(snapshot)),
                None => HostRet::invalid(format!("PlayerState: actor {} is not connected", id.0)),
            }
        }),
        BodyCall::SetPlayerHeldPose { player, main, off } => {
            let mod_id = mod_id.to_owned();
            sim_query(move |ctx| {
                match ctx.with_player(crate::player::PlayerId(player.0), |p| {
                    p.claims.set_held_pose(&mod_id, main, off)
                }) {
                    None => HostRet::Bool(false),
                    Some(true) => HostRet::Bool(true),
                    Some(false) => HostRet::invalid(
                        "SetPlayerHeldPose: non-finite rotation/translation component".into(),
                    ),
                }
            })
        }
        BodyCall::SetPlayerAnimatorParams { player, params } => {
            let params = match crate::player::animator::resolve_params(params) {
                Ok(params) => params,
                Err(e) => return HostRet::invalid(format!("SetPlayerAnimatorParams: {e}")),
            };
            let mod_id = mod_id.to_owned();
            sim_query(move |ctx| {
                match ctx.with_player(crate::player::PlayerId(player.0), |p| {
                    p.claims.set_animator_params(&mod_id, params)
                }) {
                    None => HostRet::Bool(false),
                    Some(true) => HostRet::Bool(true),
                    Some(false) => {
                        HostRet::invalid("SetPlayerAnimatorParams: non-finite value".into())
                    }
                }
            })
        }
        BodyCall::SetPlayerAnimatorPlays { player, plays } => {
            let plays = match crate::player::animator::resolve_plays(plays) {
                Ok(plays) => plays,
                Err(e) => return HostRet::invalid(format!("SetPlayerAnimatorPlays: {e}")),
            };
            let mod_id = mod_id.to_owned();
            sim_query(move |ctx| {
                match ctx.with_player(crate::player::PlayerId(player.0), |p| {
                    p.claims.set_animator_plays(&mod_id, plays)
                }) {
                    None => HostRet::Bool(false),
                    Some(true) => HostRet::Bool(true),
                    Some(false) => HostRet::invalid(
                        "SetPlayerAnimatorPlays: non-finite progress or rate".into(),
                    ),
                }
            })
        }
        BodyCall::FirePlayerAnimatorEvent { player, rig, event } => {
            let (rig, event) = match crate::player::animator::resolve_event(&rig, &event) {
                Ok(resolved) => resolved,
                Err(e) => return HostRet::invalid(format!("FirePlayerAnimatorEvent: {e}")),
            };
            sim_query(move |ctx| {
                let Some(s) = ctx.session_index(crate::player::PlayerId(player.0)) else {
                    return HostRet::Bool(false);
                };
                ctx.feed.player(s).animator_events.push((rig, event));
                HostRet::Bool(true)
            })
        }
        BodyCall::AnimationClip { rig, clip } => {
            HostRet::AnimationClip(crate::player::animator::clip_info(&rig, &clip))
        }
        BodyCall::HoldUse { player } => {
            let claimant = mod_id.to_owned();
            sim_query(move |ctx| {
                let wrote = ctx.with_player(crate::player::PlayerId(player.0), |p| {
                    p.use_gesture = crate::player::UseGesture::Held(claimant.as_str().into());
                });
                HostRet::Bool(wrote.is_some())
            })
        }
        BodyCall::SetPlayerHeldDisplay { player, main, off } => {
            let mod_id = mod_id.to_owned();
            let (main, off) = match (display_item(&main), display_item(&off)) {
                (Ok(main), Ok(off)) => (main, off),
                (Err(e), _) | (_, Err(e)) => return e,
            };
            sim_query(move |ctx| {
                let wrote = ctx.with_player(crate::player::PlayerId(player.0), |p| {
                    p.claims.set_held_display(&mod_id, main, off);
                });
                HostRet::Bool(wrote.is_some())
            })
        }
        BodyCall::PlayerInventory { player } => sim_query(move |ctx| {
            HostRet::ContainerSlots(ctx.with_player(crate::player::PlayerId(player.0), |p| {
                carried_slots(&p.inventory)
            }))
        }),
        BodyCall::SetPlayerBonePose { player, bones } => {
            let Some(bones) = crate::modding::resolve_bone_poses(bones) else {
                return HostRet::invalid(crate::modding::BONE_POSE_REFUSAL.into());
            };
            let mod_id = mod_id.to_owned();
            sim_query(move |ctx| {
                let wrote = ctx.with_player(crate::player::PlayerId(player.0), |p| {
                    p.claims.set_bone_poses(&mod_id, bones);
                });
                HostRet::Bool(wrote.is_some())
            })
        }
    }
}

fn display_item(name: &Option<String>) -> Result<Option<ItemType>, HostRet> {
    match name {
        None => Ok(None),
        Some(name) => item_by_name(name).map(Some).ok_or_else(|| {
            HostRet::invalid(format!("SetPlayerHeldDisplay: unknown item '{name}'"))
        }),
    }
}

pub(in crate::modding) fn carried_slots(
    inventory: &petramond_world::inventory::Inventory,
) -> Vec<Option<mod_api::ItemStackData>> {
    inventory
        .carried()
        .map(|slot| slot.copied().map(super::guards::item_stack_data))
        .collect()
}

#[cfg(test)]
mod tests {
    use mod_api::{calls, HostCall, HostRet};

    use crate::events::tick::TickEvents;
    use crate::events::{PostQueue, RosterRefs, SessionPlayerRef, SimCtx};
    use crate::modding::host::{handle_host_call, ModStoreData};
    use crate::modding::scope;
    use crate::player::Player;
    use crate::world::ServerWorld;
    use petramond_math::world_pos::WorldPos;

    #[test]
    fn held_data_write_refuses_a_stack_restamped_under_it() {
        use petramond_world::item::{variant, ItemStack, ItemType};

        let stamp = |n: u8| variant::VariantMap::from([("m:cond".to_owned(), vec![n])]);
        let abi = |m: &variant::VariantMap| -> Vec<(String, Vec<u8>)> {
            m.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
        };
        let write = |data: &mut ModStoreData, expect: &variant::VariantMap| {
            handle_host_call(
                data,
                HostCall::from(calls::SetPlayerHeldData {
                    player: mod_api::PlayerId(0),
                    expect_item: "petramond:stick".into(),
                    expect_data: abi(expect),
                    data: abi(&stamp(1)),
                }),
            )
        };

        let mut data = ModStoreData::new("alpha", 1);
        let mut world = ServerWorld::new(1, 1);
        let mut acting = Player::new(WorldPos::new(0.0, 80.0, 0.0));
        let held = variant::intern(&stamp(2)).expect("the fixture map interns");
        let active = acting.inventory.active_slot() as usize;
        *acting.inventory.slot_mut(active).expect("hotbar slot") =
            Some(ItemStack::with_variant(ItemType::Stick, 1, held));
        let mut feed = TickEvents::default();
        let mut queue = PostQueue::default();
        let mut gui = petramond_world::gui_state::empty_gui_state();

        {
            let mut players = RosterRefs::new(vec![SessionPlayerRef {
                id: crate::player::PlayerId(0),
                player: &mut acting,
                gui_state: &mut gui,
                gui: None,
            }]);
            let mut ctx = SimCtx {
                world: &mut world,
                actor: Some(crate::player::PlayerId(0)),
                players: &mut players,
                feed: &mut feed,
                queue: &mut queue,
            };
            scope::enter(&mut ctx, || {
                assert_eq!(
                    write(&mut data, &stamp(9)),
                    HostRet::Bool(false),
                    "data the stack no longer carries loses the compare"
                );
                assert_eq!(
                    write(&mut data, &variant::VariantMap::new()),
                    HostRet::Bool(false),
                    "an empty expectation means PLAIN, not 'any data'"
                );
                assert_eq!(write(&mut data, &stamp(2)), HostRet::Bool(true));
            });
        }
        let after = acting.inventory.selected().expect("the stack survives");
        assert_eq!(*variant::get(after.variant).expect("data"), stamp(1));
    }

    #[test]
    fn take_item_spends_one_variant_and_interns_nothing() {
        use petramond_world::item::{variant, ItemStack, ItemType};

        let stamp =
            |n: u8| variant::VariantMap::from([("test:take_item_variant".to_owned(), vec![n])]);
        let abi = |m: &variant::VariantMap| -> Vec<(String, Vec<u8>)> {
            m.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
        };
        let take = |data: &mut ModStoreData, count: u8, filter: Option<&variant::VariantMap>| {
            handle_host_call(
                data,
                HostCall::from(calls::TakeItem {
                    player: mod_api::PlayerId(0),
                    item: "petramond:stick".into(),
                    count,
                    data: filter.map(abi),
                }),
            )
        };

        let mut data = ModStoreData::new("alpha", 1);
        let mut world = ServerWorld::new(1, 1);
        let mut acting = Player::new(WorldPos::new(0.0, 80.0, 0.0));
        let tinted = variant::intern(&stamp(2)).expect("the fixture map interns");
        *acting.inventory.slot_mut(0).expect("slot") = Some(ItemStack::new(ItemType::Stick, 2));
        *acting.inventory.slot_mut(1).expect("slot") =
            Some(ItemStack::with_variant(ItemType::Stick, 3, tinted));
        let rejected = stamp(9);
        assert!(variant::is_interned_for_test(&stamp(2)));
        assert!(!variant::is_interned_for_test(&rejected));
        let mut feed = TickEvents::default();
        let mut queue = PostQueue::default();
        let mut gui = petramond_world::gui_state::empty_gui_state();

        {
            let mut players = RosterRefs::new(vec![SessionPlayerRef {
                id: crate::player::PlayerId(0),
                player: &mut acting,
                gui_state: &mut gui,
                gui: None,
            }]);
            let mut ctx = SimCtx {
                world: &mut world,
                actor: Some(crate::player::PlayerId(0)),
                players: &mut players,
                feed: &mut feed,
                queue: &mut queue,
            };
            scope::enter(&mut ctx, || {
                assert_eq!(
                    take(&mut data, 1, Some(&rejected)),
                    HostRet::ItemStack(None),
                    "a filter nothing carries takes nothing"
                );
                assert!(
                    !variant::is_interned_for_test(&rejected),
                    "the losing filter minted a variant row"
                );
                let HostRet::ItemStack(Some(took)) = take(&mut data, 3, Some(&stamp(2))) else {
                    panic!("three tinted sticks are carried");
                };
                assert_eq!((took.count, took.data), (3, abi(&stamp(2))));
                let HostRet::ItemStack(Some(took)) = take(&mut data, 2, None) else {
                    panic!("two plain sticks remain");
                };
                assert!(took.data.is_empty(), "no filter takes the first variant");
                assert_eq!(take(&mut data, 1, None), HostRet::ItemStack(None));
            });
        }
    }

    #[test]
    fn unlocking_is_per_player_idempotent_and_refuses_unknown_keys() {
        use crate::player::PlayerId;

        let recipes = petramond_world::crafting::load_recipes_for(&Default::default())
            .expect("the shipped recipes catalog loads");
        let key = recipes
            .crafting()
            .iter()
            .next()
            .expect("the shipped catalog has recipes")
            .key()
            .to_owned();
        crate::modding::install_recipes(std::sync::Arc::new(recipes));

        let mut data = ModStoreData::new("alpha", 1);
        let mut world = ServerWorld::new(1, 1);
        let mut acting = Player::new(WorldPos::new(0.0, 80.0, 0.0));
        let mut other = Player::new(WorldPos::new(4.0, 80.0, 0.0));
        let mut feed = TickEvents::default();
        let mut queue = PostQueue::default();
        let mut gui = petramond_world::gui_state::empty_gui_state();

        let unlock = |data: &mut ModStoreData, id: u8| {
            handle_host_call(
                data,
                HostCall::from(calls::UnlockRecipe {
                    player: mod_api::PlayerId(id),
                    recipe: key.clone(),
                }),
            )
        };
        let query = |data: &mut ModStoreData, id: u8| {
            handle_host_call(
                data,
                HostCall::from(calls::RecipeUnlocked {
                    player: mod_api::PlayerId(id),
                    recipe: key.clone(),
                }),
            )
        };

        let mut other_gui = petramond_world::gui_state::empty_gui_state();
        {
            let mut players = RosterRefs::new(vec![
                SessionPlayerRef {
                    id: PlayerId(0),
                    player: &mut acting,
                    gui_state: &mut gui,
                    gui: None,
                },
                SessionPlayerRef {
                    id: PlayerId(1),
                    player: &mut other,
                    gui_state: &mut other_gui,
                    gui: None,
                },
            ]);
            let mut ctx = SimCtx {
                world: &mut world,
                actor: Some(PlayerId(0)),
                players: &mut players,
                feed: &mut feed,
                queue: &mut queue,
            };
            scope::enter(&mut ctx, || {
                assert_eq!(query(&mut data, 0), HostRet::Bool(false), "starts locked");
                assert_eq!(unlock(&mut data, 0), HostRet::Bool(true), "first unlock");
                assert_eq!(unlock(&mut data, 0), HostRet::Bool(false), "idempotent");
                assert_eq!(query(&mut data, 0), HostRet::Bool(true));
                assert_eq!(query(&mut data, 1), HostRet::Bool(false));
                assert_eq!(unlock(&mut data, 1), HostRet::Bool(true));
                assert_eq!(query(&mut data, 1), HostRet::Bool(true));
                assert_eq!(unlock(&mut data, 9), HostRet::Bool(false));
                assert_eq!(
                    handle_host_call(
                        &mut data,
                        HostCall::from(calls::UnlockRecipe {
                            player: mod_api::PlayerId(0),
                            recipe: "alpha:typo".into(),
                        }),
                    ),
                    HostRet::Bool(false)
                );
            });
        }
        assert_eq!(acting.progression.unlocked(), std::slice::from_ref(&key));
        assert_eq!(other.progression.unlocked(), [key]);
    }
    #[test]
    fn body_state_writes_address_a_player_and_credit_the_calling_mod() {
        use crate::player::PlayerId;
        use petramond_world::inventory::Hand;

        let mut alpha = ModStoreData::new("alpha", 1);
        let mut beta = ModStoreData::new("beta", 1);
        let mut world = ServerWorld::new(1, 1);
        let mut acting = Player::new(WorldPos::new(0.0, 80.0, 0.0));
        let mut other = Player::new(WorldPos::new(4.0, 80.0, 0.0));
        let mut feed = TickEvents::default();
        let mut queue = PostQueue::default();
        let mut gui = petramond_world::gui_state::empty_gui_state();

        let scale = |data: &mut ModStoreData, id: u8, v: f32| {
            handle_host_call(
                data,
                HostCall::from(calls::SetPlayerAttribute {
                    player: mod_api::PlayerId(id),
                    attribute: mod_api::PlayerAttribute::MoveSpeed,
                    scale: v,
                }),
            )
        };
        let pose = |data: &mut ModStoreData, id: u8, main: Option<mod_api::HeldPose>| {
            handle_host_call(
                data,
                HostCall::from(calls::SetPlayerHeldPose {
                    player: mod_api::PlayerId(id),
                    main,
                    off: None,
                }),
            )
        };
        let guard = mod_api::HeldPose {
            first_person: mod_api::HeldPoseData {
                rotation: [0.0, 2.5, 0.0],
                translation: [1.0, 2.0, -3.0],
            },
            third_person: mod_api::HeldPoseData::IDENTITY,
        };

        let mut other_gui = petramond_world::gui_state::empty_gui_state();
        {
            let mut players = RosterRefs::new(vec![
                SessionPlayerRef {
                    id: PlayerId(0),
                    player: &mut acting,
                    gui_state: &mut gui,
                    gui: None,
                },
                SessionPlayerRef {
                    id: PlayerId(1),
                    player: &mut other,
                    gui_state: &mut other_gui,
                    gui: None,
                },
            ]);
            let mut ctx = SimCtx {
                world: &mut world,
                actor: Some(PlayerId(0)),
                players: &mut players,
                feed: &mut feed,
                queue: &mut queue,
            };
            scope::enter(&mut ctx, || {
                assert_eq!(
                    scale(&mut alpha, 9, 0.5),
                    HostRet::Bool(false),
                    "no such session"
                );
                assert_eq!(pose(&mut alpha, 9, None), HostRet::Bool(false));

                assert_eq!(scale(&mut alpha, 0, 0.5), HostRet::Bool(true));
                assert_eq!(scale(&mut beta, 0, 0.5), HostRet::Bool(true));
                assert_eq!(pose(&mut alpha, 0, Some(guard)), HostRet::Bool(true));

                let mut nan = guard;
                nan.first_person.translation[0] = f32::NAN;
                assert!(matches!(pose(&mut alpha, 0, Some(nan)), HostRet::Err(_)));
                assert!(matches!(
                    scale(&mut alpha, 0, f32::INFINITY),
                    HostRet::Err(_)
                ));

                assert_eq!(pose(&mut alpha, 1, None), HostRet::Bool(true));
            });
        }
        assert_eq!(acting.move_scale(), 0.25, "two mods' claims multiply");
        assert_eq!(
            acting.claims.held_pose(Hand::Main),
            Some(guard),
            "the refused writes left the good pose standing"
        );
        assert_eq!(acting.claims.held_pose(Hand::Off), None);
        assert_eq!(other.move_scale(), crate::player::MOVE_SCALE_DEFAULT);
        assert!(other.claims.is_empty());
    }

    #[test]
    fn implicit_calls_act_for_the_actor_and_refuse_an_actorless_dispatch() {
        use crate::player::PlayerId;

        let mut data = ModStoreData::new("alpha", 1);
        let mut world = ServerWorld::new(1, 1);
        let mut first = Player::new(WorldPos::new(0.0, 80.0, 0.0));
        let mut second = Player::new(WorldPos::new(4.0, 80.0, 0.0));
        let mut first_gui = petramond_world::gui_state::empty_gui_state();
        let mut second_gui = petramond_world::gui_state::empty_gui_state();
        let mut feed = TickEvents::default();
        let mut queue = PostQueue::default();
        let mut players = RosterRefs::new(vec![
            SessionPlayerRef {
                id: PlayerId(0),
                player: &mut first,
                gui_state: &mut first_gui,
                gui: None,
            },
            SessionPlayerRef {
                id: PlayerId(1),
                player: &mut second,
                gui_state: &mut second_gui,
                gui: None,
            },
        ]);

        let mut ctx = SimCtx {
            world: &mut world,
            actor: Some(PlayerId(1)),
            players: &mut players,
            feed: &mut feed,
            queue: &mut queue,
        };
        scope::enter(&mut ctx, || {
            assert_eq!(
                handle_host_call(&mut data, HostCall::from(calls::ActingPlayer)),
                HostRet::ActingPlayer(Some(mod_api::PlayerId(1)))
            );
            let HostRet::Player(state) =
                handle_host_call(&mut data, HostCall::from(calls::PlayerState))
            else {
                panic!("the actor's snapshot");
            };
            assert_eq!(state.id, Some(mod_api::PlayerId(1)));
            assert_eq!(state.pos[0], 4.0, "the actor's body, not session 0's");
            assert_eq!(
                handle_host_call(&mut data, HostCall::from(calls::SetHealth { value: 5 })),
                HostRet::Unit
            );
        });

        ctx.actor = None;
        scope::enter(&mut ctx, || {
            assert_eq!(
                handle_host_call(&mut data, HostCall::from(calls::ActingPlayer)),
                HostRet::ActingPlayer(None)
            );
            assert!(matches!(
                handle_host_call(&mut data, HostCall::from(calls::PlayerState)),
                HostRet::Err(_)
            ));
            assert!(matches!(
                handle_host_call(&mut data, HostCall::from(calls::SetHealth { value: 1 })),
                HostRet::Err(_)
            ));
            assert_eq!(
                handle_host_call(
                    &mut data,
                    HostCall::from(calls::SetHealthOf {
                        player: mod_api::PlayerId(0),
                        value: 7,
                    })
                ),
                HostRet::Bool(true)
            );
            assert_eq!(
                handle_host_call(
                    &mut data,
                    HostCall::from(calls::SetHealthOf {
                        player: mod_api::PlayerId(9),
                        value: 7,
                    })
                ),
                HostRet::Bool(false),
                "no such session"
            );
            let HostRet::PlayerOf(Some(state)) = handle_host_call(
                &mut data,
                HostCall::from(calls::PlayerStateOf {
                    player: mod_api::PlayerId(1),
                }),
            ) else {
                panic!("the named session's snapshot");
            };
            assert_eq!(state.health, 5);
        });
        assert_eq!(
            first.health(),
            7,
            "only the explicit write reached session 0"
        );
        assert_eq!(second.health(), 5);
    }
}
