use std::collections::BTreeMap;

use super::protocol::{SpatialSoundMsg, WorldEventMsg};

pub type LiveSpatialLoops = BTreeMap<u64, SpatialSoundMsg>;

pub fn is_looped(sound_id: u8) -> bool {
    petramond_world::sound_registry::defs()
        .get(sound_id as usize)
        .is_some_and(|def| def.looped)
}

pub fn loop_restarts(live: &LiveSpatialLoops) -> impl Iterator<Item = WorldEventMsg> + '_ {
    live.values().map(|cmd| WorldEventMsg::SpatialSound(*cmd))
}

pub fn fold_spatial_loops(
    live: &mut LiveSpatialLoops,
    world_events: &mut Vec<WorldEventMsg>,
    looped: impl Fn(u8) -> bool,
    alive: impl Fn(u64) -> bool,
) {
    note_loop_commands(live, world_events, looped);
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

pub fn note_loop_commands(
    live: &mut LiveSpatialLoops,
    world_events: &[WorldEventMsg],
    looped: impl Fn(u8) -> bool,
) {
    for ev in world_events {
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use petramond_math::world_pos::WorldPos;

    const LOOP_ROW: u8 = 200;
    const ONE_SHOT_ROW: u8 = 201;

    fn play_on_mob(handle: u64, sound_id: u8, mob_id: u64) -> WorldEventMsg {
        WorldEventMsg::SpatialSound(SpatialSoundMsg::PlayOnMob {
            handle,
            sound_id,
            mob_id,
            volume: 0.5,
            pitch: 1.0,
            last_pos: WorldPos::ZERO,
        })
    }

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
}
