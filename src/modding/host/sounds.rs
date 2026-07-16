//! Sound and burst calls: one-shots, handle-addressed spatial sounds, and
//! particle bursts — all riding `TickEvents`; the sim never touches audio.

use mod_api::{HostCall, HostRet};

use crate::mathh::Vec3;

use super::guards::{finite3, sim_call, sim_query};

/// Phase 3b: sound (one-shots plus the handle-based spatial commands; the sim
/// never touches audio — everything rides `TickEvents` to the app layer).
pub(super) fn handle_sound_call(mod_id: &str, call: HostCall) -> HostRet {
    match call {
        // Not a sound, but the same shape: a fire-and-forget world-anchored
        // presentation one-shot riding the NON-lossy tick queue.
        HostCall::EmitterBurst {
            key,
            pos,
            intensity,
        } => match finite3(pos, "EmitterBurst.pos") {
            Err(e) => e,
            Ok(pos) => sim_query(|ctx| {
                if !intensity.is_finite() {
                    return HostRet::Error("EmitterBurst: non-finite intensity".into());
                }
                let Some(bundle) = crate::particle_emitters::by_key(&key) else {
                    log::warn!("[mod {mod_id}] EmitterBurst: unknown emitter '{key}'");
                    return HostRet::Bool(false);
                };
                if bundle.burst.is_none() {
                    log::warn!("[mod {mod_id}] EmitterBurst: '{key}' is not a burst bundle");
                    return HostRet::Bool(false);
                }
                ctx.feed
                    .world
                    .emitter_bursts
                    .push((bundle.id, pos, intensity));
                HostRet::Bool(true)
            }),
        },
        HostCall::EmitSound { key, pos } => sim_query(|ctx| {
            let Some(sound) = crate::audio::sound_by_name(&key) else {
                log::warn!("[mod {mod_id}] EmitSound: unknown sound '{key}'");
                return HostRet::Bool(false);
            };
            // The sim never touches audio: the sound rides the NON-lossy tick
            // queue on `TickEvents` and the app layer plays it next frame.
            ctx.feed.world.sounds.push(crate::game::ModSound {
                sound,
                pos: pos.map(Vec3::from),
            });
            HostRet::Bool(true)
        }),
        HostCall::SoundPlayAt {
            key,
            pos,
            volume,
            pitch,
        } => sim_query(|ctx| {
            let Some(sound) = crate::audio::sound_by_name(&key) else {
                log::warn!("[mod {mod_id}] SoundPlayAt: unknown sound '{key}'");
                return HostRet::U64(0);
            };
            if !spatial_sound_params_ok(pos, volume, pitch) {
                log::warn!("[mod {mod_id}] SoundPlayAt: rejected non-finite or negative parameter");
                return HostRet::U64(0);
            }
            let handle = ctx.feed.alloc_spatial_sound_handle();
            ctx.feed
                .world
                .spatial_sounds
                .push(crate::game::ModSpatialSoundCommand::PlayAt {
                    handle,
                    sound,
                    pos: pos.into(),
                    volume,
                    pitch,
                });
            HostRet::U64(handle)
        }),
        HostCall::SoundPlayOnMob {
            mob_id,
            key,
            volume,
            pitch,
        } => sim_query(|ctx| {
            let Some(sound) = crate::audio::sound_by_name(&key) else {
                log::warn!("[mod {mod_id}] SoundPlayOnMob: unknown sound '{key}'");
                return HostRet::U64(0);
            };
            if !spatial_sound_scalar_params_ok(volume, pitch) {
                log::warn!(
                    "[mod {mod_id}] SoundPlayOnMob: rejected non-finite or negative parameter"
                );
                return HostRet::U64(0);
            }
            let Some(last_pos) = ctx
                .world
                .mobs()
                .instances()
                .iter()
                .find(|m| m.id() == mob_id && !m.is_dead())
                .map(|m| m.pos)
            else {
                log::warn!("[mod {mod_id}] SoundPlayOnMob: no live mob with stable id {mob_id}");
                return HostRet::U64(0);
            };
            let handle = ctx.feed.alloc_spatial_sound_handle();
            ctx.feed
                .world
                .spatial_sounds
                .push(crate::game::ModSpatialSoundCommand::PlayOnMob {
                    handle,
                    sound,
                    mob_id,
                    volume,
                    pitch,
                    last_pos,
                });
            HostRet::U64(handle)
        }),
        HostCall::SoundStop { handle } => sim_call(|ctx| {
            if handle != 0 {
                ctx.feed
                    .world
                    .spatial_sounds
                    .push(crate::game::ModSpatialSoundCommand::Stop { handle });
            }
        }),
        other => HostRet::Error(format!(
            "non-sound call {other:?} mis-routed to handle_sound_call (host bug)"
        )),
    }
}

fn spatial_sound_params_ok(pos: [f32; 3], volume: f32, pitch: f32) -> bool {
    pos.iter().all(|c| c.is_finite()) && spatial_sound_scalar_params_ok(volume, pitch)
}

fn spatial_sound_scalar_params_ok(volume: f32, pitch: f32) -> bool {
    volume.is_finite() && volume >= 0.0 && pitch.is_finite() && pitch > 0.0
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

    /// `EmitSound` feeds the NON-lossy tick queue (never audio directly) and
    /// an unknown key reports failure without disabling anything.
    #[test]
    fn emit_sound_rides_the_tick_feed() {
        let mut data = ModStoreData::new("alpha", 1);
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
                    &mut data,
                    HostCall::EmitSound {
                        key: "petramond:item_pickup".into(),
                        pos: Some([1.0, 64.0, 1.0]),
                    },
                ),
                HostRet::Bool(true)
            );
            assert_eq!(
                handle_host_call(
                    &mut data,
                    HostCall::EmitSound {
                        key: "no_such:sound".into(),
                        pos: None,
                    },
                ),
                HostRet::Bool(false)
            );
        });
        assert_eq!(feed.world.sounds.len(), 1, "one resolved sound queued");
        assert_eq!(feed.world.sounds[0].pos, Some(Vec3::new(1.0, 64.0, 1.0)));
    }

    #[test]
    fn spatial_sound_calls_queue_resolved_commands_with_deterministic_handles() {
        fn run_once() -> (u64, u64, Vec<crate::game::ModSpatialSoundCommand>) {
            let mut data = ModStoreData::new("alpha", 1);
            let mut world = World::new(1, 1);
            assert!(world
                .mobs_mut()
                .spawn(crate::mob::Mob::Owl, Vec3::new(2.0, 80.0, 3.0), 0.0));
            let mob_id = world.mobs().instances()[0].id();
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
            let mut handles = (0, 0);
            scope::enter(&mut ctx, || {
                handles.0 = match handle_host_call(
                    &mut data,
                    HostCall::SoundPlayAt {
                        key: "petramond:item_pickup".into(),
                        pos: [1.0, 81.0, 1.0],
                        volume: 0.5,
                        pitch: 1.25,
                    },
                ) {
                    HostRet::U64(handle) => handle,
                    other => panic!("SoundPlayAt returned {other:?}"),
                };
                handles.1 = match handle_host_call(
                    &mut data,
                    HostCall::SoundPlayOnMob {
                        mob_id,
                        key: "petramond:item_pickup".into(),
                        volume: 0.75,
                        pitch: 0.9,
                    },
                ) {
                    HostRet::U64(handle) => handle,
                    other => panic!("SoundPlayOnMob returned {other:?}"),
                };
                assert_eq!(
                    handle_host_call(&mut data, HostCall::SoundStop { handle: handles.0 }),
                    HostRet::Unit
                );
                assert_eq!(
                    handle_host_call(
                        &mut data,
                        HostCall::SoundPlayAt {
                            key: "no_such:sound".into(),
                            pos: [0.0, 0.0, 0.0],
                            volume: 1.0,
                            pitch: 1.0,
                        },
                    ),
                    HostRet::U64(0),
                    "unknown sounds do not allocate handles"
                );
            });
            (handles.0, handles.1, feed.world.spatial_sounds)
        }

        let first = run_once();
        let second = run_once();
        assert_ne!(first.0, 0);
        assert_ne!(first.0, first.1, "two starts get distinct handles");
        assert_eq!(
            first, second,
            "same session inputs produce the same handles"
        );

        let sound =
            crate::audio::sound_by_name("petramond:item_pickup").expect("engine sound exists");
        assert_eq!(first.2.len(), 3);
        assert_eq!(
            first.2[0],
            crate::game::ModSpatialSoundCommand::PlayAt {
                handle: first.0,
                sound,
                pos: Vec3::new(1.0, 81.0, 1.0),
                volume: 0.5,
                pitch: 1.25,
            }
        );
        match first.2[1] {
            crate::game::ModSpatialSoundCommand::PlayOnMob {
                handle,
                sound: queued_sound,
                mob_id,
                volume,
                pitch,
                last_pos,
            } => {
                assert_eq!(handle, first.1);
                assert_eq!(queued_sound, sound);
                assert_ne!(mob_id, 0);
                assert_eq!(volume, 0.75);
                assert_eq!(pitch, 0.9);
                assert_eq!(last_pos, Vec3::new(2.0, 80.0, 3.0));
            }
            other => panic!("expected mob-pinned sound command, got {other:?}"),
        }
        assert_eq!(
            first.2[2],
            crate::game::ModSpatialSoundCommand::Stop { handle: first.0 }
        );
    }
}
