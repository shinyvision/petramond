//! The server's memory of every spatial LOOP still playing: a looping row
//! (`sounds.json` `loop: true`) started by `SoundPlayAt`/`SoundPlayOnMob`
//! plays until its `SoundStop`, so unlike a one-shot it has STATE a session
//! joining mid-play would otherwise never hear. This table replays that
//! state to a newcomer and ends a mob-pinned loop with its mob, so a mod
//! owns "start, retune, stop" and nothing else.

use std::collections::BTreeMap;

use crate::net::protocol::{SpatialSoundMsg, WorldEventMsg};

use super::ServerGame;

/// Live loops keyed by session handle, as the command that (re)starts them.
pub type LiveSpatialLoops = BTreeMap<u64, SpatialSoundMsg>;

/// Fold one tick window's spatial commands into `live`: a play of a looping
/// row (`looped(sound_id)`) is remembered as its own restart command, a
/// `Set` retunes the remembered play, a `Stop` forgets it. Then every
/// remembered mob-pinned loop whose mob `alive(mob_id)` denies is dropped
/// and a `Stop` for it is APPENDED to the window, so observers hear it end
/// where the mob died and a mod that forgot (or was disabled) leaves no
/// orphan humming forever.
pub fn fold_spatial_loops(
    live: &mut LiveSpatialLoops,
    world_events: &mut Vec<WorldEventMsg>,
    looped: impl Fn(u8) -> bool,
    alive: impl Fn(u64) -> bool,
) {
    for ev in world_events.iter() {
        let WorldEventMsg::SpatialSound(cmd) = ev else {
            continue;
        };
        match *cmd {
            SpatialSoundMsg::PlayAt {
                handle, sound_id, ..
            }
            | SpatialSoundMsg::PlayOnMob {
                handle, sound_id, ..
            } => {
                if looped(sound_id) {
                    live.insert(handle, *cmd);
                }
            }
            SpatialSoundMsg::Set {
                handle,
                volume,
                pitch,
            } => {
                if let Some(
                    SpatialSoundMsg::PlayAt {
                        volume: v,
                        pitch: p,
                        ..
                    }
                    | SpatialSoundMsg::PlayOnMob {
                        volume: v,
                        pitch: p,
                        ..
                    },
                ) = live.get_mut(&handle)
                {
                    *v = volume;
                    *p = pitch;
                }
            }
            SpatialSoundMsg::Stop { handle } => {
                live.remove(&handle);
            }
        }
    }
    let orphaned: Vec<u64> = live
        .iter()
        .filter_map(|(&handle, remembered)| match *remembered {
            SpatialSoundMsg::PlayOnMob { mob_id, .. } if !alive(mob_id) => Some(handle),
            _ => None,
        })
        .collect();
    for handle in orphaned {
        live.remove(&handle);
        world_events.push(WorldEventMsg::SpatialSound(SpatialSoundMsg::Stop {
            handle,
        }));
    }
}

impl ServerGame {
    /// [`fold_spatial_loops`] over the registry's rows and the world's live
    /// mobs, for the window about to ship.
    pub(super) fn track_spatial_loops(&mut self, world_events: &mut Vec<WorldEventMsg>) {
        let mobs = self.world.mobs();
        fold_spatial_loops(
            &mut self.live_spatial_loops,
            world_events,
            |sound_id| {
                petramond_world::sound_registry::Sound(sound_id)
                    .def()
                    .looped
            },
            |mob_id| {
                mobs.instances()
                    .iter()
                    .any(|m| m.id() == mob_id && !m.is_dead())
            },
        );
    }

    /// Queue every live loop for session `s`'s next tick batch — the join
    /// catch-up. A mob-pinned loop's client resolves the emitter from the
    /// mob rows in the same batch, so the stale `last_pos` only matters if
    /// the mob is already gone.
    pub(crate) fn replay_spatial_loops_to(&mut self, s: usize) {
        let replay = self
            .live_spatial_loops
            .values()
            .map(|cmd| WorldEventMsg::SpatialSound(*cmd));
        self.sessions[s].pending_world_events.extend(replay);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::protocol::ServerToClient;
    use petramond_math::math::Vec3;

    const LOOP_ROW: u8 = 200;
    const ONE_SHOT_ROW: u8 = 201;

    fn play_on_mob(handle: u64, sound_id: u8, mob_id: u64) -> WorldEventMsg {
        WorldEventMsg::SpatialSound(SpatialSoundMsg::PlayOnMob {
            handle,
            sound_id,
            mob_id,
            volume: 0.5,
            pitch: 1.0,
            last_pos: Vec3::ZERO,
        })
    }

    /// A loop row is remembered through its retunes until its stop; a
    /// one-shot never is; a remembered loop on a dead mob is stopped for
    /// everyone by the window that finds it gone.
    #[test]
    fn loops_are_remembered_retuned_forgotten_and_ended_with_their_mob() {
        let mut live = LiveSpatialLoops::new();
        let looped = |id: u8| id == LOOP_ROW;
        let mut window = vec![
            play_on_mob(1, LOOP_ROW, 10),
            play_on_mob(2, ONE_SHOT_ROW, 10),
            WorldEventMsg::SpatialSound(SpatialSoundMsg::Set {
                handle: 1,
                volume: 0.9,
                pitch: 1.5,
            }),
        ];
        fold_spatial_loops(&mut live, &mut window, looped, |_| true);
        assert_eq!(window.len(), 3, "nothing appended while the mob lives");
        assert!(!live.contains_key(&2), "a one-shot is never remembered");
        match live.get(&1) {
            Some(SpatialSoundMsg::PlayOnMob { volume, pitch, .. }) => {
                assert_eq!(
                    (*volume, *pitch),
                    (0.9, 1.5),
                    "the restart carries the retune"
                );
            }
            other => panic!("loop not remembered as its play: {other:?}"),
        }

        let mut window = vec![WorldEventMsg::SpatialSound(SpatialSoundMsg::Stop {
            handle: 1,
        })];
        fold_spatial_loops(&mut live, &mut window, looped, |_| true);
        assert!(live.is_empty(), "a stopped loop is forgotten");

        let mut window = vec![play_on_mob(3, LOOP_ROW, 10)];
        fold_spatial_loops(&mut live, &mut window, looped, |_| true);
        let mut window = Vec::new();
        fold_spatial_loops(&mut live, &mut window, looped, |mob| mob != 10);
        assert!(live.is_empty(), "the dead mob's loop is dropped");
        assert_eq!(
            window,
            vec![WorldEventMsg::SpatialSound(SpatialSoundMsg::Stop {
                handle: 3
            })],
            "and its stop is appended for every observer"
        );
    }

    /// A session joining while a loop plays hears it: the replay leads its
    /// first tick batch, and no other session is told twice.
    #[test]
    fn a_joining_session_is_caught_up_on_the_live_loops() {
        let mut server = crate::server::session_build::build_server_inline("", 1, 2);
        let restart = SpatialSoundMsg::PlayAt {
            handle: 7,
            sound_id: 0,
            pos: Vec3::new(1.0, 64.0, 1.0),
            volume: 0.3,
            pitch: 1.0,
        };
        server.live_spatial_loops.insert(7, restart);
        let joiner = crate::server::session_build::spawn_player(server.world.seed);
        let s = server.add_session_for_test(joiner);
        let joiner_id = server.sessions[s].id;

        let out = server.pump(0.06, &mut Vec::new());
        let spatial_events = |msgs: &[ServerToClient]| -> Vec<SpatialSoundMsg> {
            msgs.iter()
                .filter_map(|m| match m {
                    ServerToClient::Tick(update) => Some(update.events.iter()),
                    _ => None,
                })
                .flatten()
                .filter_map(|ev| match ev {
                    WorldEventMsg::SpatialSound(cmd) => Some(*cmd),
                    _ => None,
                })
                .collect()
        };
        let joiner_msgs = out
            .remote
            .iter()
            .find(|(id, _)| *id == joiner_id)
            .map(|(_, msgs)| msgs.as_slice())
            .expect("the joiner got a batch");
        assert_eq!(
            spatial_events(joiner_msgs).first(),
            Some(&restart),
            "the joiner's first batch leads with the live loop"
        );
        assert!(
            spatial_events(&out.msgs).is_empty(),
            "the host session is not told again"
        );
        let again = server.pump(0.06, &mut Vec::new());
        let joiner_again = again
            .remote
            .iter()
            .find(|(id, _)| *id == joiner_id)
            .map(|(_, msgs)| msgs.as_slice())
            .unwrap_or(&[]);
        assert!(
            spatial_events(joiner_again).is_empty(),
            "the catch-up ships once"
        );
    }
}
