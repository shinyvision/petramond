//! Core calls, legal on any instance: logging, the tick clock, RNG
//! streams, the `mod_init` registration window, and shader parameters.

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
        HostCall::CurrentTick => match scope::with_active(|ctx| ctx.world.current_tick()) {
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
        other => HostRet::Error(format!(
            "non-core call {other:?} mis-routed to handle_core_call (host bug)"
        )),
    }
}

#[cfg(test)]
mod tests {
    use mod_api::{HostCall, HostRet};

    use crate::events::{PostQueue, SimCtx};
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
