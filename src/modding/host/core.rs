use mod_api::{CoreCall, ErrorCode, HostRet};

use crate::modding::scope;

use super::guards::{
    event_key_guard, key_owned_by_namespace, public_write_key_guard, sim_call, sim_query,
};
use super::{ModStoreData, Registration};

pub(super) fn handle_core_call(data: &mut ModStoreData, call: CoreCall) -> HostRet {
    match call {
        CoreCall::Log { msg } => {
            log::info!("[mod {}] {msg}", data.mod_id);
            HostRet::Unit
        }
        CoreCall::RuntimeSide => HostRet::RuntimeSide(data.side),
        CoreCall::CurrentTick => match scope::with_active_ref(|ctx| ctx.world.current_tick())
            .or_else(crate::modding::ai::detached_tick)
        {
            Some(tick) => HostRet::U64(tick),
            None => HostRet::error(
                ErrorCode::NoContext,
                "no simulation context is active".into(),
            ),
        },
        CoreCall::RngU64 { stream_key } => HostRet::U64(data.rng_next(&stream_key)),
        CoreCall::RegisterTickSystem {
            stage,
            attach,
            priority,
            system_id,
        } => data.register(Registration::TickSystem {
            stage,
            attach,
            priority,
            system_id,
        }),
        CoreCall::RegisterEventHandler {
            event,
            priority,
            handler_id,
            filter,
        } => {
            if let Err(why) = filter.check(event) {
                return data.refuse_registration(
                    ErrorCode::InvalidArgument,
                    format!("event handler {handler_id}: {why}"),
                );
            }
            data.register(Registration::EventHandler {
                event,
                priority,
                handler_id,
                filter,
            })
        }
        CoreCall::RegisterHostileSpawner {
            callback_id,
            priority,
        } => data.register(Registration::HostileSpawner {
            priority,
            callback_id,
        }),
        CoreCall::RegisterBlockBehavior { key, callback_id } => {
            if !key_owned_by_namespace(&data.mod_id, &key) {
                return HostRet::error(
                    ErrorCode::Forbidden,
                    format!(
                        "block behavior key '{key}' must be namespaced '{}:name'",
                        data.mod_id
                    ),
                );
            }
            data.register(Registration::BlockBehavior { key, callback_id })
        }
        CoreCall::RegisterAiNode { key, callback_id } => {
            if !key_owned_by_namespace(&data.mod_id, &key) {
                return HostRet::error(
                    ErrorCode::Forbidden,
                    format!(
                        "AI node key '{key}' must be namespaced '{}:name'",
                        data.mod_id
                    ),
                );
            }
            data.register(Registration::AiNode { key, callback_id })
        }
        CoreCall::ShaderSetParam { key, value } => match public_write_key_guard(&data.mod_id, &key)
        {
            Some(e) => e,
            None => sim_call(|ctx| ctx.world.set_shader_param(key, value)),
        },
        CoreCall::EmitEvent { key, data: bytes } => {
            if let Some(e) = event_key_guard("EmitEvent", &data.mod_id, &key, bytes.len()) {
                return e;
            }
            sim_call(|ctx| {
                ctx.queue
                    .emit(crate::events::PostEvent::ModEvent { key, data: bytes })
            })
        }
        CoreCall::EmitEventTo {
            player,
            key,
            data: bytes,
        } => {
            if let Some(e) = event_key_guard("EmitEventTo", &data.mod_id, &key, bytes.len()) {
                return e;
            }
            let mod_id = data.mod_id.clone();
            sim_query(move |ctx| {
                let player = crate::player::PlayerId(player.0);
                if ctx.session_index(player).is_none() {
                    log::warn!(
                        "[mod {mod_id}] EmitEventTo '{key}': player {} is not connected",
                        player.0
                    );
                    return HostRet::Bool(false);
                }
                ctx.feed.client_events.push(crate::events::ClientEvent {
                    player,
                    key,
                    data: bytes,
                });
                HostRet::Bool(true)
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use mod_api::{calls, HostCall, HostRet};

    use crate::events::{PostQueue, RosterRefs, SimCtx};

    #[test]
    fn current_tick_reads_the_detached_ai_dispatch_stash() {
        use crate::modding::host::{handle_host_call, ModStoreData};
        let mut data = ModStoreData::new("alpha", 1);
        assert!(
            matches!(
                handle_host_call(&mut data, HostCall::from(calls::CurrentTick)),
                HostRet::Err(_)
            ),
            "outside every scope there is no tick to report"
        );
        let ret = crate::modding::ai::with_detached_tick(7, || {
            handle_host_call(&mut data, HostCall::from(calls::CurrentTick))
        });
        assert_eq!(ret, HostRet::U64(7));
        assert!(
            matches!(
                handle_host_call(&mut data, HostCall::from(calls::CurrentTick)),
                HostRet::Err(_)
            ),
            "the stash is scoped to the dispatch"
        );
    }
    use crate::events::tick::TickEvents;
    use crate::modding::host::{handle_host_call, ModStoreData};
    use crate::modding::scope;
    use crate::world::ServerWorld;

    #[test]
    fn shader_param_writes_are_namespaced_and_tick_scoped() {
        let mut alpha = ModStoreData::new("alpha", 1);
        let mut beta = ModStoreData::new("beta", 1);
        let mut world = ServerWorld::new(1, 1);
        let mut nobody = RosterRefs::empty();
        let mut feed = TickEvents::default();
        let mut queue = PostQueue::default();
        let mut ctx = SimCtx {
            world: &mut world,
            actor: None,
            players: &mut nobody,
            feed: &mut feed,
            queue: &mut queue,
        };

        scope::enter(&mut ctx, || {
            assert_eq!(
                handle_host_call(
                    &mut alpha,
                    HostCall::from(calls::ShaderSetParam {
                        key: "alpha:sky".into(),
                        value: [0.25, 0.5, 0.75, 1.0],
                    }),
                ),
                HostRet::Unit
            );
            assert!(matches!(
                handle_host_call(
                    &mut beta,
                    HostCall::from(calls::ShaderSetParam {
                        key: "alpha:sky".into(),
                        value: [1.0; 4],
                    }),
                ),
                HostRet::Err(_)
            ));
            assert_eq!(
                handle_host_call(
                    &mut beta,
                    HostCall::from(calls::ShaderSetParam {
                        key: "petramond:light".into(),
                        value: [0.8, 0.0, 0.0, 0.0],
                    }),
                ),
                HostRet::Unit
            );
        });

        assert_eq!(
            world.data().environment().shader_params().get("alpha:sky"),
            Some(&[0.25, 0.5, 0.75, 1.0])
        );
        assert_eq!(
            world
                .data()
                .environment()
                .shader_params()
                .get("petramond:light"),
            Some(&[0.8, 0.0, 0.0, 0.0])
        );
        assert!(matches!(
            handle_host_call(
                &mut alpha,
                HostCall::from(calls::ShaderSetParam {
                    key: "alpha:outside".into(),
                    value: [0.0; 4],
                }),
            ),
            HostRet::Err(_)
        ));
    }
}
