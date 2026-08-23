//! Mod GUI calls: the session state map plus queued open/close requests.

use mod_api::{HostCall, HostRet};

use crate::events::DeferredAction;

use super::guards::{sim_call, sim_query};

/// Mod-GUI calls (session state map plus open/close).
/// State keys are mod-local: the map belongs to one GUI session (cleared
/// on open/close), so unlike the persistent KV no prefix is enforced.
pub(super) fn handle_gui_call(mod_id: &str, call: HostCall) -> HostRet {
    match call {
        HostCall::GuiStateSet { key, value } => sim_call(|ctx| {
            petramond_world::gui_state::gui_state_set(
                ctx.gui_state,
                key,
                crate::modding::convert::gui_value(value),
            )
        }),
        // The per-SESSION write. Everything above it writes whichever session
        // the running dispatch belongs to, which for a tick system is the host
        // — so a machine's gauges reached one player, chosen by join order.
        HostCall::GuiStateSetFor {
            player_id,
            key,
            value,
        } => sim_query(move |ctx| {
            let value = crate::modding::convert::gui_value(value);
            let id = crate::player::PlayerId(player_id.0);
            HostRet::Bool(
                ctx.with_gui_state(id, |map| {
                    petramond_world::gui_state::gui_state_set(map, key, value)
                })
                .is_some(),
            )
        }),
        HostCall::GuiViewers => sim_query(|ctx| {
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
                            anchor: open.anchor.map(|p| [p.x, p.y, p.z]),
                        })
                    })
                    .collect(),
            )
        }),
        HostCall::GuiStateGet { key } => sim_query(|ctx| {
            HostRet::GuiValue(
                ctx.gui_state
                    .get(&key)
                    .map(crate::modding::convert::gui_value_out),
            )
        }),
        HostCall::GuiOpen { kind_key } => {
            // Resolve WITHOUT registering: opening a kind nothing declared is
            // a mod bug, reported forgivingly (like an unknown sound key).
            let Some(kind) =
                petramond_world::gui_state::resolve_kind(&kind_key).filter(|k| k.is_registered())
            else {
                log::warn!("[mod {mod_id}] GuiOpen: unknown or non-mod gui kind '{kind_key}'");
                return HostRet::Bool(false);
            };
            sim_query(|ctx| {
                ctx.queue.push_action(DeferredAction::OpenGui { kind });
                HostRet::Bool(true)
            })
        }
        HostCall::GuiClose => sim_call(|ctx| ctx.queue.push_action(DeferredAction::CloseGui)),
        other => HostRet::Error(format!(
            "non-GUI call {other:?} mis-routed to handle_gui_call (host bug)"
        )),
    }
}

#[cfg(test)]
mod tests {
    use mod_api::{GuiValue, HostCall, HostRet};

    use crate::events::tick::TickEvents;
    use crate::events::{OpenGui, PostQueue, SessionPlayerRef, SimCtx};
    use crate::modding::host::{handle_host_call, ModStoreData};
    use crate::modding::scope;
    use crate::player::Player;
    use crate::player::PlayerId;
    use crate::world::World;
    use petramond_math::math::{IVec3, Vec3};

    /// TWO sessions with panels open at TWO machines, which is the whole
    /// point: a mod runs once for the server, and a tick system acts as the
    /// HOST session, so the implicit `GuiStateSet` writes one map — session
    /// 0's — however many players are standing at machines. Every reading the
    /// other player's panel showed was then either the first player's machine
    /// or nothing at all.
    #[test]
    fn gauges_reach_the_session_that_is_looking_not_the_host_session() {
        let mut world = World::new(1, 1);
        let mut host = Player::new(Vec3::new(0.0, 80.0, 0.0));
        let mut guest = Player::new(Vec3::new(8.0, 80.0, 8.0));
        let mut host_gui = petramond_world::gui_state::empty_gui_state();
        let mut guest_gui = petramond_world::gui_state::empty_gui_state();
        let mut feed = TickEvents::default();
        let mut queue = PostQueue::default();
        let kind =
            petramond_world::gui_state::intern_kind("doctest:machine").expect("a mod kind interns");
        let (host_at, guest_at) = (IVec3::new(1, 2, 3), IVec3::new(9, 2, 3));

        let others = vec![SessionPlayerRef {
            id: PlayerId(1),
            player: &mut guest,
            gui_state: &mut guest_gui,
            gui: Some(OpenGui {
                kind,
                anchor: Some(guest_at),
            }),
        }];
        let acting_gui = Some(OpenGui {
            kind,
            anchor: Some(host_at),
        });
        crate::events::with_sessions_scope(PlayerId(0), acting_gui, others, || {
            let mut ctx = SimCtx {
                world: &mut world,
                player: &mut host,
                gui_state: &mut host_gui,
                feed: &mut feed,
                queue: &mut queue,
            };
            scope::enter(&mut ctx, || {
                let mut data = ModStoreData::new("doctest", 1);

                // Who is looking, and at what: the machine's anchor, so
                // matching a viewer to a placed machine is an equality test.
                let HostRet::GuiViewers(viewers) =
                    handle_host_call(&mut data, HostCall::GuiViewers)
                else {
                    panic!("GuiViewers answers its own reply kind");
                };
                assert_eq!(
                    viewers
                        .iter()
                        .map(|v| (v.player_id.0, v.anchor))
                        .collect::<Vec<_>>(),
                    vec![(0, Some([1, 2, 3])), (1, Some([9, 2, 3]))],
                    "both sessions, in session order, with the cell each opened"
                );
                assert!(viewers.iter().all(|v| v.kind == "doctest:machine"));

                // One reading per viewer, under the SAME flat key — which is
                // exactly what a flat key space costs nothing when the map is
                // per session, because a session has one GUI open.
                for (v, level) in viewers.iter().zip([0.25f32, 0.75]) {
                    assert_eq!(
                        handle_host_call(
                            &mut data,
                            HostCall::GuiStateSetFor {
                                player_id: v.player_id,
                                key: "doctest:level".into(),
                                value: GuiValue::F32(level),
                            }
                        ),
                        HostRet::Bool(true)
                    );
                }
                // An unknown session is a refusal, not a write into someone.
                assert_eq!(
                    handle_host_call(
                        &mut data,
                        HostCall::GuiStateSetFor {
                            player_id: mod_api::PlayerId(99),
                            key: "doctest:level".into(),
                            value: GuiValue::F32(1.0),
                        }
                    ),
                    HostRet::Bool(false)
                );
            });
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
