//! Player calls: state snapshot, the damage funnel, knockback, items,
//! health, teleports, status effects, and chat delivery. (There is no kill
//! call: `DamagePlayer` with current health is the kill, same funnel.)

use mod_api::{HostCall, HostRet, PlayerSnapshot};

use crate::events::DeferredAction;
use petramond_world::item::variant::{self, VariantMap};
use petramond_world::item::ItemStack;

use super::entities::{give_item, give_item_to};
use super::guards::{batch_guard, finite3, item_by_name, sim_call, sim_mutate, sim_query};
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
            // Sneak, use-held and the swing facts live on the session, not
            // the `Player` body: read the acting session's published roster
            // row (same tick, same intent latches). No roster published (mod
            // init, unit fixtures) reads as neither.
            let (sneak, use_held, swing) = ctx
                .acting_player_id()
                .and_then(|id| {
                    ctx.world
                        .player_roster()
                        .iter()
                        .find(|r| r.id == id.0)
                        .map(|r| (r.sneak, r.use_held, r.swing))
                })
                .unwrap_or((false, false, Default::default()));
            let id = ctx.acting_player_id().map(|id| mod_api::PlayerId(id.0));
            let p = &*ctx.player;
            HostRet::Player(PlayerSnapshot {
                id,
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
                // The ACTING hand's stack: during the use-click ladder's
                // off-hand pass this answers the off-hand item, so a handler
                // gating on "what am I holding" acts for whichever hand the
                // dispatch is offering — with no hand on the ABI.
                held: p.held().map(|st| mod_api::ItemId(st.item.id())),
                held_count: p.held().map_or(0, |st| st.count),
                // The literal OFF-HAND slot, whichever hand is acting — the
                // "both hands at once" read beside `held`/`off_held`.
                off_held: p
                    .inventory
                    .off_hand()
                    .map(|st| mod_api::ItemId(st.item.id())),
                pose_anchor: ctx
                    .acting_player_id()
                    .and_then(|id| pose_anchor_of(ctx.world, id.0)),
                swing,
                half_width: crate::player::HALF_W,
                height: crate::player::HEIGHT,
                eye_height: crate::player::EYE,
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
                            id: Some(mod_api::PlayerId(p.id)),
                            pos: p.pos,
                            vel: p.vel,
                            yaw: p.yaw,
                            pitch: p.pitch,
                            health: p.health,
                            on_ground: p.on_ground,
                            spectator: p.spectator,
                            sneak: p.sneak,
                            use_held: p.use_held,
                            holds_use: p.use_gesture.held_by(mod_id),
                            held: p.held.map(|i| mod_api::ItemId(i.id())),
                            held_count: p.held_count,
                            off_held: p.off_held.map(|i| mod_api::ItemId(i.id())),
                            pose_anchor: pose_anchor_of(ctx.world, p.id),
                            swing: p.swing,
                            half_width: crate::player::HALF_W,
                            height: crate::player::HEIGHT,
                            eye_height: crate::player::EYE,
                        },
                    })
                    .collect(),
            )
        }),
        HostCall::DamagePlayer {
            player,
            amount,
            origin,
            attacker,
        } => match origin
            .map(|p| finite3(p, "DamagePlayer.origin"))
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
        HostCall::ApplyKnockback { impulse } => match finite3(impulse, "ApplyKnockback.impulse") {
            Err(e) => e,
            Ok(impulse) => sim_call(|ctx| ctx.player.apply_knockback(impulse)),
        },
        HostCall::GiveItem { item, count, data } => {
            let variant = match super::guards::intern_abi_data("GiveItem", &data) {
                Ok(v) => v,
                Err(e) => return e,
            };
            sim_query(|ctx| {
                let Some(item_ty) = item_by_name(&item) else {
                    log::warn!("[mod {mod_id}] GiveItem: unknown item '{item}'");
                    return HostRet::Bool(false);
                };
                give_item(ctx, item_ty, count, variant);
                HostRet::Bool(true)
            })
        }
        HostCall::GiveItemTo {
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
        // The per-player, per-stack held read: the stack's instance data
        // rides along, which the row-level `PlayerState.held` cannot carry.
        // Resolvable sessions come from the published sessions view (the
        // acting session always resolves through the ctx's own borrow);
        // an unresolvable id answers `None`, like every id-addressed read.
        HostCall::PlayerHeld { player } => sim_query(move |ctx| {
            let id = crate::player::PlayerId(player.0);
            HostRet::HeldStack(
                // The acting hand's stack (non-acting sessions are always
                // outside a dispatch, so theirs reads the selected slot).
                ctx.with_player(id, |p| p.held().copied())
                    .flatten()
                    .map(super::guards::item_stack_data),
            )
        }),
        // The compare-and-set write onto the HELD stack: the mod names both
        // the item and the exact data it computed the new map against, so a
        // hand swapped OR a stack another writer re-stamped in between
        // refuses rather than clobbers. Explicit player addressing like
        // GiveItemTo; the slot write bumps the inventory revision, so
        // replication is automatic.
        HostCall::SetPlayerHeldData {
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
        // Atomic: only an acting-hand stack holding at least `count` of `item`
        // consumes — the held stack IS the validation, so no registry check.
        // During the ladder's off-hand pass this spends the off-hand.
        HostCall::ConsumeHeld { item, count } => sim_query(|ctx| {
            let hand = ctx.player.acting_hand;
            let holds = count > 0
                && ctx
                    .player
                    .held()
                    .is_some_and(|st| st.item.0 == item.0 && st.count as u32 >= count);
            if !holds {
                return HostRet::Bool(false);
            }
            for _ in 0..count {
                ctx.player.inventory.decrement_held(hand);
            }
            HostRet::Bool(true)
        }),
        HostCall::ReplaceHeldOne { item, replacement } => sim_query(|ctx| {
            let hand = ctx.player.acting_hand;
            let holds = ctx
                .player
                .held()
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
                .replace_held_one(hand, ItemStack::new(replacement_ty, 1));
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
            let Some(effect) = petramond_world::effect::by_name(&key) else {
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
        // Body-level player-state primitives like SetHealth: direct mutation
        // of the named session, no events. Both are per-player writes a tick
        // system re-states, so they address the player EXPLICITLY (the
        // addressing doctrine) and route through the sessions view — never
        // the acting-session shortcut. `BodyClaims` owns the invariants: the
        // claim is keyed by THIS mod, non-finite is refused whole, and finite
        // values clamp.
        HostCall::SetPlayerAttribute {
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
                        HostRet::Error(format!("SetPlayerAttribute: {scale} is not finite"))
                    }
                }
            })
        }
        HostCall::SetPlayerHeldPose { player, main, off } => {
            let mod_id = mod_id.to_owned();
            sim_query(move |ctx| {
                match ctx.with_player(crate::player::PlayerId(player.0), |p| {
                    p.claims.set_held_pose(&mod_id, main, off)
                }) {
                    None => HostRet::Bool(false),
                    Some(true) => HostRet::Bool(true),
                    Some(false) => HostRet::Error(
                        "SetPlayerHeldPose: non-finite rotation/translation component".into(),
                    ),
                }
            })
        }
        // The action-denial claim. Infallible below the ABI (a set of enum
        // values has no malformed form), so the only answer is whether the
        // addressed session is reachable.
        // Taking the use gesture is a body write like the claims beside it:
        // addressed at a session, keyed by the caller, transient.
        HostCall::HoldUse { player } => {
            let claimant = mod_id.to_owned();
            sim_query(move |ctx| {
                let wrote = ctx.with_player(crate::player::PlayerId(player.0), |p| {
                    p.use_gesture = crate::player::UseGesture::Held(claimant.as_str().into());
                });
                HostRet::Bool(wrote.is_some())
            })
        }
        HostCall::SetPlayerDeniedActions { player, actions } => {
            let mod_id = mod_id.to_owned();
            let denied = crate::player::DeniedActions::of(actions);
            sim_query(move |ctx| {
                let wrote = ctx.with_player(crate::player::PlayerId(player.0), |p| {
                    p.claims.set_denied_actions(&mod_id, denied);
                });
                HostRet::Bool(wrote.is_some())
            })
        }
        HostCall::SetPlayerBonePose { player, bones } => {
            // Names resolve to rig ids HERE, once, so nothing below this
            // boundary carries a string.
            let Some(bones) = crate::modding::resolve_bone_poses(bones) else {
                return HostRet::Error(crate::modding::BONE_POSE_REFUSAL.into());
            };
            let mod_id = mod_id.to_owned();
            sim_query(move |ctx| {
                // The claim's own validation cannot fail here — `resolve` has
                // already rejected the one thing it refuses — so the only
                // answer left is whether the addressed session exists.
                let wrote = ctx.with_player(crate::player::PlayerId(player.0), |p| {
                    p.claims.set_bone_poses(&mod_id, bones);
                });
                HostRet::Bool(wrote.is_some())
            })
        }
        // The motion-ownership claim: which of each hand's engine motions
        // the CLAIMING mod animates itself, silencing the engine's own copy.
        // Infallible like the denial claim (a set of enum values), transient,
        // and addressed explicitly through the sessions view. Enum-to-bits
        // happens HERE, once, like bone names resolving to rig ids.
        HostCall::SetPlayerHandMotions { player, main, off } => {
            let mod_id = mod_id.to_owned();
            let (main, off) = (
                crate::player::HandMotions::of(main),
                crate::player::HandMotions::of(off),
            );
            sim_query(move |ctx| {
                let wrote = ctx.with_player(crate::player::PlayerId(player.0), |p| {
                    p.claims.set_hand_motions(&mod_id, main, off);
                });
                HostRet::Bool(wrote.is_some())
            })
        }
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
            ctx.queue
                .push_action(DeferredAction::ChatSend { text, targets });
            HostRet::Bool(true)
        }),
        // Progression is per-player state, so both arms address a player
        // explicitly (the addressing doctrine) and route through the sessions
        // view — never the acting-session shortcut.
        HostCall::UnlockRecipe { player, recipe } => sim_query(|ctx| {
            // A key no catalog row owns would sit in the player's record
            // forever, unlocking nothing and never failing — refuse it, so a
            // typo shows up as a `false` the mod can log.
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
                // No such session — or a dispatch site that publishes no
                // sessions roster (the pre-event sites that still dispatch
                // without `with_sessions_view`). Silence here would look
                // exactly like "already unlocked", so say which.
                None => {
                    log::warn!(
                        "[mod {mod_id}] UnlockRecipe '{recipe}': player {} is not reachable from \
                         this dispatch (no such session, or this site publishes no sessions view)",
                        player.0
                    );
                    HostRet::Bool(false)
                }
            }
        }),
        HostCall::RecipeUnlocked { player, recipe } => sim_query(|ctx| {
            let unlocked = ctx
                .with_player(crate::player::PlayerId(player.0), |p| {
                    p.progression.is_unlocked(&recipe)
                })
                .unwrap_or(false);
            HostRet::Bool(unlocked)
        }),
        other => HostRet::Error(format!(
            "non-player call {other:?} mis-routed to handle_player_call (host bug)"
        )),
    }
}

#[cfg(test)]
mod tests {
    use mod_api::{HostCall, HostRet};

    use crate::events::tick::TickEvents;
    use crate::events::{PostQueue, SimCtx};
    use crate::modding::host::{handle_host_call, ModStoreData};
    use crate::modding::scope;
    use crate::player::Player;
    use crate::world::World;
    use petramond_math::math::Vec3;

    /// The held-data write compares the VALUE it is replacing, not just the
    /// item: a stack that another handler re-stamped between the mod's read
    /// and its write refuses instead of silently dropping that other write.
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
                HostCall::SetPlayerHeldData {
                    player: mod_api::PlayerId(0),
                    expect_item: "petramond:stick".into(),
                    expect_data: abi(expect),
                    data: abi(&stamp(1)),
                },
            )
        };

        let mut data = ModStoreData::new("alpha", 1);
        let mut world = World::new(1, 1);
        let mut acting = Player::new(Vec3::new(0.0, 80.0, 0.0));
        let held = variant::intern(&stamp(2)).expect("the fixture map interns");
        let active = acting.inventory.active_slot() as usize;
        *acting.inventory.slot_mut(active).expect("hotbar slot") =
            Some(ItemStack::with_variant(ItemType::Stick, 1, held));
        let mut feed = TickEvents::default();
        let mut queue = PostQueue::default();
        let mut gui = petramond_world::gui_state::empty_gui_state();

        crate::events::with_sessions_scope(crate::player::PlayerId(0), None, Vec::new(), || {
            let mut ctx = SimCtx {
                world: &mut world,
                player: &mut acting,
                gui_state: &mut gui,
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
        });
        let after = acting.inventory.selected().expect("the stack survives");
        assert_eq!(*variant::get(after.variant).expect("data"), stamp(1));
    }

    /// The progression arms: unlocking is per-player, idempotent (`true` only
    /// on the call that changed it), refuses a key no catalog row owns, and
    /// the query reads back what the write stored. Reaching the player needs
    /// the dispatch site's sessions view — an unreachable player answers
    /// `false` rather than silently unlocking the acting one.
    #[test]
    fn unlocking_is_per_player_idempotent_and_refuses_unknown_keys() {
        use crate::player::PlayerId;

        // The validation reads the process-wide installed catalog (the same
        // snapshot `RecipeResult` answers from), and every session build
        // installs the REAL one — so the fixture installs that and takes a key
        // FROM it. A synthetic catalog would be stomped by any concurrently
        // building session; naming a specific engine recipe would pin editable
        // data. Neither is this test's subject.
        let recipes = petramond_world::crafting::load_recipes_for(&Default::default());
        let key = recipes
            .crafting()
            .iter()
            .next()
            .expect("the shipped catalog has recipes")
            .key()
            .to_owned();
        crate::modding::install_recipes(std::sync::Arc::new(recipes));

        let mut data = ModStoreData::new("alpha", 1);
        let mut world = World::new(1, 1);
        let mut acting = Player::new(Vec3::new(0.0, 80.0, 0.0));
        let mut other = Player::new(Vec3::new(4.0, 80.0, 0.0));
        let mut feed = TickEvents::default();
        let mut queue = PostQueue::default();
        let mut gui = petramond_world::gui_state::empty_gui_state();

        let unlock = |data: &mut ModStoreData, id: u8| {
            handle_host_call(
                data,
                HostCall::UnlockRecipe {
                    player: mod_api::PlayerId(id),
                    recipe: key.clone(),
                },
            )
        };
        let query = |data: &mut ModStoreData, id: u8| {
            handle_host_call(
                data,
                HostCall::RecipeUnlocked {
                    player: mod_api::PlayerId(id),
                    recipe: key.clone(),
                },
            )
        };

        let mut other_gui = petramond_world::gui_state::empty_gui_state();
        let others = vec![crate::events::SessionPlayerRef {
            id: PlayerId(1),
            player: &mut other,
            gui_state: &mut other_gui,
            gui: None,
        }];
        crate::events::with_sessions_scope(PlayerId(0), None, others, || {
            let mut ctx = SimCtx {
                world: &mut world,
                player: &mut acting,
                gui_state: &mut gui,
                feed: &mut feed,
                queue: &mut queue,
            };
            scope::enter(&mut ctx, || {
                assert_eq!(query(&mut data, 0), HostRet::Bool(false), "starts locked");
                assert_eq!(unlock(&mut data, 0), HostRet::Bool(true), "first unlock");
                assert_eq!(unlock(&mut data, 0), HostRet::Bool(false), "idempotent");
                assert_eq!(query(&mut data, 0), HostRet::Bool(true));
                // Per PLAYER: the other session is untouched until addressed.
                assert_eq!(query(&mut data, 1), HostRet::Bool(false));
                assert_eq!(unlock(&mut data, 1), HostRet::Bool(true));
                assert_eq!(query(&mut data, 1), HostRet::Bool(true));
                // No such session.
                assert_eq!(unlock(&mut data, 9), HostRet::Bool(false));
                // A key no catalog row owns is refused rather than stored.
                assert_eq!(
                    handle_host_call(
                        &mut data,
                        HostCall::UnlockRecipe {
                            player: mod_api::PlayerId(0),
                            recipe: "alpha:typo".into(),
                        },
                    ),
                    HostRet::Bool(false)
                );
            });
        });
        assert_eq!(acting.progression.unlocked(), std::slice::from_ref(&key));
        assert_eq!(other.progression.unlocked(), [key]);
    }
    /// The body-claim primitives (`SetPlayerAttribute` /
    /// `SetPlayerHeldPose`): per-player writes through the sessions view — an
    /// unreachable player answers `false` and the ADDRESSED body is written,
    /// not whoever the tick happens to run as. The claim is keyed by the
    /// CALLING mod, so the addressing doctrine survives the fold.
    #[test]
    fn body_state_writes_address_a_player_and_credit_the_calling_mod() {
        use crate::player::PlayerId;
        use petramond_world::inventory::Hand;

        let mut alpha = ModStoreData::new("alpha", 1);
        let mut beta = ModStoreData::new("beta", 1);
        let mut world = World::new(1, 1);
        let mut acting = Player::new(Vec3::new(0.0, 80.0, 0.0));
        let mut other = Player::new(Vec3::new(4.0, 80.0, 0.0));
        let mut feed = TickEvents::default();
        let mut queue = PostQueue::default();
        let mut gui = petramond_world::gui_state::empty_gui_state();

        let scale = |data: &mut ModStoreData, id: u8, v: f32| {
            handle_host_call(
                data,
                HostCall::SetPlayerAttribute {
                    player: mod_api::PlayerId(id),
                    attribute: mod_api::PlayerAttribute::MoveSpeed,
                    scale: v,
                },
            )
        };
        let pose = |data: &mut ModStoreData, id: u8, main: Option<mod_api::HeldPose>| {
            handle_host_call(
                data,
                HostCall::SetPlayerHeldPose {
                    player: mod_api::PlayerId(id),
                    main,
                    off: None,
                },
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
        let others = vec![crate::events::SessionPlayerRef {
            id: PlayerId(1),
            player: &mut other,
            gui_state: &mut other_gui,
            gui: None,
        }];
        crate::events::with_sessions_scope(PlayerId(0), None, others, || {
            let mut ctx = SimCtx {
                world: &mut world,
                player: &mut acting,
                gui_state: &mut gui,
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

                // Two mods claiming the same body compose rather than race.
                assert_eq!(scale(&mut alpha, 0, 0.5), HostRet::Bool(true));
                assert_eq!(scale(&mut beta, 0, 0.5), HostRet::Bool(true));
                assert_eq!(pose(&mut alpha, 0, Some(guard)), HostRet::Bool(true));

                // A non-finite claim is a loud error, not a stored value.
                let mut nan = guard;
                nan.first_person.translation[0] = f32::NAN;
                assert!(matches!(pose(&mut alpha, 0, Some(nan)), HostRet::Error(_)));
                assert!(matches!(
                    scale(&mut alpha, 0, f32::INFINITY),
                    HostRet::Error(_)
                ));

                // A clear addressed at the OTHER session is a valid write —
                // per-player, never the acting shortcut.
                assert_eq!(pose(&mut alpha, 1, None), HostRet::Bool(true));
            });
        });
        assert_eq!(acting.move_scale(), 0.25, "two mods' claims multiply");
        assert_eq!(
            acting.claims.held_pose(Hand::Main),
            Some(guard),
            "the refused writes left the good pose standing"
        );
        assert_eq!(acting.claims.held_pose(Hand::Off), None);
        // The other session stayed at its defaults (checked after the scope:
        // its borrow lives in `others`).
        assert_eq!(other.move_scale(), crate::player::MOVE_SCALE_DEFAULT);
        assert!(other.claims.is_empty());
    }
}
