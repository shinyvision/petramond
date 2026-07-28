//! Core calls: logging, the tick clock, RNG streams, the `mod_init`
//! registration window, and shader parameters. Log/RNG/RuntimeSide are
//! scope-free and legal on any instance in any dispatch. The tick clock is
//! legal in EVERY simulation-instance dispatch — it reads the active sim
//! scope, falling back to the detached AI-dispatch tick stash
//! (`modding::ai::detached_tick`) — but errors on instances that have no
//! tick at all (worldgen workers, whose replies must be pure functions of
//! their inputs; client instances gate it off in `client_capability`).

use mod_api::{HostCall, HostRet};

use crate::modding::scope;

use super::guards::{key_owned_by_namespace, public_write_key_guard, sim_call};
use super::{ModStoreData, Registration};

/// Store-side core calls: logging, the tick counter, RNG streams, the
/// `mod_init` registration window, and shader params.
pub(super) fn handle_core_call(data: &mut ModStoreData, call: HostCall) -> HostRet {
    match call {
        HostCall::Log { msg } => {
            log::info!("[mod {}] {msg}", data.mod_id);
            HostRet::Unit
        }
        HostCall::RuntimeSide => HostRet::RuntimeSide(data.side),
        HostCall::CurrentTick => match scope::with_active(|ctx| ctx.world.current_tick())
            .or_else(crate::modding::ai::detached_tick)
        {
            Some(tick) => HostRet::U64(tick),
            None => HostRet::Error("no simulation context is active".into()),
        },
        HostCall::RngU64 { stream_key } => HostRet::U64(data.rng_next(&stream_key)),
        HostCall::RegisterTickSystem {
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
        HostCall::RegisterEventHandler {
            event,
            priority,
            handler_id,
        } => data.register(Registration::EventHandler {
            event,
            priority,
            handler_id,
        }),
        HostCall::RegisterHostileSpawner {
            callback_id,
            priority,
        } => data.register(Registration::HostileSpawner {
            priority,
            callback_id,
        }),
        HostCall::RegisterBlockBehavior { key, callback_id } => {
            // A behavior key routes hooks back to its owner, so it must carry
            // THIS mod's namespace (same ownership rule as catalog keys).
            if !key_owned_by_namespace(&data.mod_id, &key) {
                return HostRet::Error(format!(
                    "block behavior key '{key}' must be namespaced '{}:name'",
                    data.mod_id
                ));
            }
            data.register(Registration::BlockBehavior { key, callback_id })
        }
        HostCall::RegisterAiNode { key, callback_id } => {
            if !key_owned_by_namespace(&data.mod_id, &key) {
                return HostRet::Error(format!(
                    "AI node key '{key}' must be namespaced '{}:name'",
                    data.mod_id
                ));
            }
            data.register(Registration::AiNode { key, callback_id })
        }
        HostCall::ShaderSetParam { key, value } => match public_write_key_guard(&data.mod_id, &key)
        {
            Some(e) => e,
            None => sim_call(|ctx| ctx.world.set_shader_param(key, value)),
        },
        // A mod's own event: QUEUED, never dispatched inline. Re-entering the
        // bus from inside a guest dispatch is what the architecture forbids
        // (it would run other mods' handlers inside this mod's host call), so
        // this rides the post queue like every other observational event and
        // dispatches at the next drain point in the same tick.
        HostCall::EmitEvent { key, data: bytes } => {
            // Emitting under another mod's namespace would let a mod forge
            // events its owner is trusted for; the key is the only filter a
            // handler has. Same rule, same guard, as a KV write.
            if let Some(e) = event_key_guard(&data.mod_id, &key, bytes.len()) {
                return e;
            }
            sim_call(|ctx| {
                ctx.queue
                    .emit(crate::events::PostEvent::ModEvent { key, data: bytes })
            })
        }
        other => HostRet::Error(format!(
            "non-core call {other:?} mis-routed to handle_core_call (host bug)"
        )),
    }
}

/// A mod event's key must be the emitter's OWN namespace (an engine
/// `petramond:` event is the engine's to fire), and its payload is bounded
/// like a KV value — the queue holds these until the next drain.
fn event_key_guard(mod_id: &str, key: &str, len: usize) -> Option<HostRet> {
    if !key_owned_by_namespace(mod_id, key) {
        return Some(HostRet::Error(format!(
            "EmitEvent key '{key}' is not in mod '{mod_id}'s namespace"
        )));
    }
    (len > super::guards::EVENT_MAX_DATA_BYTES).then(|| {
        HostRet::Error(format!(
            "EmitEvent payload is {len} bytes; the limit is {}",
            super::guards::EVENT_MAX_DATA_BYTES
        ))
    })
}

#[cfg(test)]
mod tests {
    use mod_api::{HostCall, HostRet};

    use crate::events::{PostQueue, SimCtx};

    /// The tick clock is legal in the detached AI-dispatch scope: with no sim
    /// scope active it reads the dispatcher's published tick instead of
    /// erroring — the contract that lets scripted AI nodes call
    /// `current_tick` like any other dispatch (mods must never be forced to
    /// count time in dispatches).
    #[test]
    fn current_tick_reads_the_detached_ai_dispatch_stash() {
        use crate::modding::host::{handle_host_call, ModStoreData};
        let mut data = ModStoreData::new("alpha", 1);
        assert!(
            matches!(
                handle_host_call(&mut data, HostCall::CurrentTick),
                HostRet::Error(_)
            ),
            "outside every scope there is no tick to report"
        );
        let ret = crate::modding::ai::with_detached_tick(7, || {
            handle_host_call(&mut data, HostCall::CurrentTick)
        });
        assert_eq!(ret, HostRet::U64(7));
        assert!(
            matches!(
                handle_host_call(&mut data, HostCall::CurrentTick),
                HostRet::Error(_)
            ),
            "the stash is scoped to the dispatch"
        );
    }
    use crate::game::TickEvents;
    use crate::mathh::Vec3;
    use crate::modding::host::{handle_host_call, ModStoreData};
    use crate::modding::scope;
    use crate::player::Player;
    use crate::world::World;

    /// Shader params are the visual environment surface mods use for sky
    /// shaders and other pack-owned effects: own namespace or engine `petramond:*`,
    /// tick-scoped, and stored in the world's neutral environment snapshot.
    #[test]
    fn shader_param_writes_are_namespaced_and_tick_scoped() {
        let mut alpha = ModStoreData::new("alpha", 1);
        let mut beta = ModStoreData::new("beta", 1);
        let mut world = World::new(1, 1);
        let mut player = Player::new(Vec3::new(0.0, 80.0, 0.0));
        let mut feed = TickEvents::default();
        let mut queue = PostQueue::default();
        let mut gui = crate::gui::empty_gui_state();
        let mut ctx = SimCtx {
            world: &mut world,
            player: &mut player,
            gui_state: &mut gui,
            feed: &mut feed,
            queue: &mut queue,
        };

        scope::enter(&mut ctx, || {
            assert_eq!(
                handle_host_call(
                    &mut alpha,
                    HostCall::ShaderSetParam {
                        key: "alpha:sky".into(),
                        value: [0.25, 0.5, 0.75, 1.0],
                    },
                ),
                HostRet::Unit
            );
            assert!(matches!(
                handle_host_call(
                    &mut beta,
                    HostCall::ShaderSetParam {
                        key: "alpha:sky".into(),
                        value: [1.0; 4],
                    },
                ),
                HostRet::Error(_)
            ));
            assert_eq!(
                handle_host_call(
                    &mut beta,
                    HostCall::ShaderSetParam {
                        key: "petramond:light".into(),
                        value: [0.8, 0.0, 0.0, 0.0],
                    },
                ),
                HostRet::Unit
            );
        });

        assert_eq!(
            world.environment().shader_params().get("alpha:sky"),
            Some(&[0.25, 0.5, 0.75, 1.0])
        );
        assert_eq!(
            world.environment().shader_params().get("petramond:light"),
            Some(&[0.8, 0.0, 0.0, 0.0])
        );
        assert!(matches!(
            handle_host_call(
                &mut alpha,
                HostCall::ShaderSetParam {
                    key: "alpha:outside".into(),
                    value: [0.0; 4],
                },
            ),
            HostRet::Error(_)
        ));
    }
}
