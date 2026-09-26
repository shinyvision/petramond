//! Mod GUI calls: the per-session state map plus queued open/close requests.
//!
//! Every map belongs to ONE session (the GUI it has open). The implicit calls
//! (`GuiStateSet`/`GuiStateGet`/`GuiOpen`/`GuiClose`) address the dispatch's
//! actor and refuse an actor-less dispatch; their `...For` twins name the
//! session — a machine's gauges belong to whoever is looking at it.

use mod_api::{GuiCall, HostRet};

use crate::events::{DeferredAction, SimCtx};
use crate::player::PlayerId;

use super::guards::{actor_for, sim_mutate, sim_query};

/// Queue opening the mod GUI `kind_key` (anchored at `at`) for `player`.
/// `false` = unknown or non-mod kind, an anchor no longer present, or no
/// such session.
fn open_gui(
    ctx: &mut SimCtx<'_>,
    mod_id: &str,
    player: PlayerId,
    kind_key: &str,
    at: Option<mod_api::ContainerAddress>,
) -> bool {
    // Resolve WITHOUT registering: opening a kind nothing declared is a mod
    // bug, reported forgivingly (like an unknown sound key).
    let Some(kind) =
        petramond_world::gui_state::resolve_kind(kind_key).filter(|k| k.is_registered())
    else {
        log::warn!("[mod {mod_id}] GuiOpen: unknown or non-mod gui kind '{kind_key}'");
        return false;
    };
    if ctx.session_index(player).is_none() {
        return false;
    }
    let anchor = at.map(crate::modding::convert::menu_anchor);
    if anchor.is_some_and(|anchor| !anchor.present(ctx.world)) {
        return false;
    }
    ctx.queue.push_action(DeferredAction::OpenGui {
        player,
        kind,
        anchor,
    });
    true
}

/// Queue closing `player`'s open mod GUI. `false` = no such session.
fn close_gui(ctx: &mut SimCtx<'_>, player: PlayerId) -> bool {
    if ctx.session_index(player).is_none() {
        return false;
    }
    ctx.queue.push_action(DeferredAction::CloseGui { player });
    true
}

/// `player`'s GUI state value under `key`. `None` = unset or no such session.
fn state_get(ctx: &mut SimCtx<'_>, player: PlayerId, key: &str) -> Option<mod_api::GuiValue> {
    ctx.with_gui_state(player, |map| {
        map.get(key).map(crate::modding::convert::gui_value_out)
    })
    .flatten()
}

/// Mod-GUI calls (session state map plus open/close).
/// State keys are mod-local: the map belongs to one GUI session (cleared
/// on open/close), so unlike the persistent KV no prefix is enforced.
pub(super) fn handle_gui_call(mod_id: &str, call: GuiCall) -> HostRet {
    match call {
        GuiCall::GuiStateSet { key, value } => sim_mutate(|ctx| {
            let id = actor_for(ctx, "GuiStateSet", "GuiStateSetFor")?;
            let value = crate::modding::convert::gui_value(value);
            ctx.with_gui_state(id, |map| {
                petramond_world::gui_state::gui_state_set(map, key, value)
            });
            Ok(())
        }),
        // The per-SESSION write: a machine's gauges reach the session that is
        // looking at it, however many players stand at machines.
        GuiCall::GuiStateSetFor {
            player_id,
            key,
            value,
        } => sim_query(move |ctx| {
            let value = crate::modding::convert::gui_value(value);
            let id = PlayerId(player_id.0);
            HostRet::Bool(
                ctx.with_gui_state(id, |map| {
                    petramond_world::gui_state::gui_state_set(map, key, value)
                })
                .is_some(),
            )
        }),
        GuiCall::GuiViewers => sim_query(|ctx| {
            HostRet::GuiViewers(
                ctx.gui_viewers()
                    .into_iter()
                    .filter_map(|(id, open)| {
                        // Mod kinds only: an engine chest is nobody's machine
                        // and its key is not this vocabulary.
                        let kind = open
                            .kind
                            .is_registered()
                            .then(|| petramond_world::gui_state::kind_key(open.kind))??;
                        Some(mod_api::GuiViewerData {
                            player_id: mod_api::PlayerId(id.0),
                            kind: kind.to_owned(),
                            anchor: open.anchor.map(crate::modding::convert::container_address),
                        })
                    })
                    .collect(),
            )
        }),
        GuiCall::GuiStateGet { key } => sim_query(|ctx| {
            match actor_for(ctx, "GuiStateGet", "GuiStateGetFor") {
                Ok(id) => HostRet::GuiValue(state_get(ctx, id, &key)),
                Err(e) => e,
            }
        }),
        GuiCall::GuiStateGetFor { player_id, key } => {
            sim_query(|ctx| HostRet::GuiValue(state_get(ctx, PlayerId(player_id.0), &key)))
        }
        GuiCall::GuiOpen { kind_key, at } => sim_query(|ctx| {
            match actor_for(ctx, "GuiOpen", "GuiOpenFor") {
                Ok(id) => HostRet::Bool(open_gui(ctx, mod_id, id, &kind_key, at)),
                Err(e) => e,
            }
        }),
        GuiCall::GuiOpenFor {
            player_id,
            kind_key,
            at,
        } => sim_query(|ctx| {
            HostRet::Bool(open_gui(ctx, mod_id, PlayerId(player_id.0), &kind_key, at))
        }),
        GuiCall::GuiClose => sim_mutate(|ctx| {
            let id = actor_for(ctx, "GuiClose", "GuiCloseFor")?;
            close_gui(ctx, id);
            Ok(())
        }),
        GuiCall::GuiCloseFor { player_id } => {
            sim_query(|ctx| HostRet::Bool(close_gui(ctx, PlayerId(player_id.0))))
        }
    }
}

#[cfg(test)]
mod tests {
    use mod_api::{calls, GuiValue, HostCall, HostRet};

    use crate::events::tick::TickEvents;
    use crate::events::{OpenGui, PostQueue, RosterRefs, SessionPlayerRef, SimCtx};
    use crate::modding::host::{handle_host_call, ModStoreData};
    use crate::modding::scope;
    use crate::player::Player;
    use crate::player::PlayerId;
    use crate::world::ServerWorld;
    use petramond_math::math::IVec3;
    use petramond_math::world_pos::WorldPos;

    /// TWO sessions with panels open at TWO machines, which is the whole
    /// point: a mod runs once for the server, and a tick system acts for
    /// nobody, so an implicit `GuiStateSet` has no map to write — it refuses
    /// — while the per-session write reaches each viewer's own panel.
    #[test]
    fn gauges_reach_the_session_that_is_looking_and_implicit_writes_refuse() {
        let mut world = ServerWorld::new(1, 1);
        let mut host = Player::new(WorldPos::new(0.0, 80.0, 0.0));
        let mut guest = Player::new(WorldPos::new(8.0, 80.0, 8.0));
        let mut host_gui = petramond_world::gui_state::empty_gui_state();
        let mut guest_gui = petramond_world::gui_state::empty_gui_state();
        let mut feed = TickEvents::default();
        let mut queue = PostQueue::default();
        let kind =
            petramond_world::gui_state::intern_kind("doctest:machine").expect("a mod kind interns");
        let (host_at, guest_at) = (IVec3::new(1, 2, 3), IVec3::new(9, 2, 3));

        let mut players = RosterRefs::new(vec![
            SessionPlayerRef {
                id: PlayerId(0),
                player: &mut host,
                gui_state: &mut host_gui,
                gui: Some(OpenGui {
                    kind,
                    anchor: Some(host_at.into()),
                }),
            },
            SessionPlayerRef {
                id: PlayerId(1),
                player: &mut guest,
                gui_state: &mut guest_gui,
                gui: Some(OpenGui {
                    kind,
                    anchor: Some(guest_at.into()),
                }),
            },
        ]);
        let mut ctx = SimCtx {
            world: &mut world,
            actor: None,
            players: &mut players,
            feed: &mut feed,
            queue: &mut queue,
        };
        scope::enter(&mut ctx, || {
            let mut data = ModStoreData::new("doctest", 1);

            // Who is looking, and at what: the machine's anchor, so matching
            // a viewer to a placed machine is an equality test.
            let HostRet::GuiViewers(viewers) = handle_host_call(&mut data, HostCall::from(calls::GuiViewers))
            else {
                panic!("GuiViewers answers its own reply kind");
            };
            assert_eq!(
                viewers
                    .iter()
                    .map(|v| (v.player_id.0, v.anchor))
                    .collect::<Vec<_>>(),
                vec![
                    (0, Some(mod_api::ContainerAddress::Block([1, 2, 3]))),
                    (1, Some(mod_api::ContainerAddress::Block([9, 2, 3]))),
                ],
                "both sessions, in session order, with the cell each opened"
            );
            assert!(viewers.iter().all(|v| v.kind == "doctest:machine"));

            // No actor, no implicit map: the host session is nobody special.
            assert!(matches!(
                handle_host_call(
                    &mut data,
                    HostCall::from(calls::GuiStateSet {
                        key: "doctest:level".into(),
                        value: GuiValue::F32(1.0),
                    })
                ),
                HostRet::Err(_)
            ));

            // One reading per viewer, under the SAME flat key — which is
            // exactly what a flat key space costs nothing when the map is per
            // session, because a session has one GUI open.
            for (v, level) in viewers.iter().zip([0.25f32, 0.75]) {
                assert_eq!(
                    handle_host_call(
                        &mut data,
                        HostCall::from(calls::GuiStateSetFor {
                            player_id: v.player_id,
                            key: "doctest:level".into(),
                            value: GuiValue::F32(level),
                        })
                    ),
                    HostRet::Bool(true)
                );
            }
            assert_eq!(
                handle_host_call(
                    &mut data,
                    HostCall::from(calls::GuiStateGetFor {
                        player_id: mod_api::PlayerId(1),
                        key: "doctest:level".into(),
                    })
                ),
                HostRet::GuiValue(Some(GuiValue::F32(0.75)))
            );
            // An unknown session is a refusal, not a write into someone.
            assert_eq!(
                handle_host_call(
                    &mut data,
                    HostCall::from(calls::GuiStateSetFor {
                        player_id: mod_api::PlayerId(99),
                        key: "doctest:level".into(),
                        value: GuiValue::F32(1.0),
                    })
                ),
                HostRet::Bool(false)
            );
        });

        let read = |map: &std::sync::Arc<petramond_world::gui_state::GuiStateMap>| match map
            .get("doctest:level")
        {
            Some(petramond_world::gui_state::GuiValue::F32(v)) => *v,
            other => panic!("expected the published gauge, got {other:?}"),
        };
        assert_eq!(read(&host_gui), 0.25);
        assert_eq!(
            read(&guest_gui),
            0.75,
            "the second player's panel shows THEIR machine, not the host's"
        );
    }
}
