//! The server's memory of every spatial LOOP still playing (see
//! [`crate::net::spatial_loops`]): the table `super::event_scope` starts and
//! stops per recipient by earshot, ending a mob-pinned loop with its mob, so
//! a mod owns "start, retune, stop" and nothing else.

use crate::mob::Mobs;
use crate::net::protocol::WorldEventMsg;
use crate::net::spatial_loops::{fold_spatial_loops, is_looped};

use super::replication::Broadcast;

impl Broadcast {
    /// [`fold_spatial_loops`] over the registry's rows and the world's live
    /// mobs, for the window about to ship.
    pub fn track_spatial_loops(&mut self, mobs: &Mobs, world_events: &mut Vec<WorldEventMsg>) {
        fold_spatial_loops(
            self.spatial_loops_mut(),
            world_events,
            is_looped,
            |mob_id| {
                mobs.instances()
                    .iter()
                    .any(|m| m.id() == mob_id && !m.is_dead())
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use crate::net::protocol::{ServerToClient, SpatialSoundMsg, WorldEventMsg};

    /// A session joining while a loop plays within its earshot hears it: the
    /// restart leads its first tick batch, and ships only once.
    #[test]
    fn a_joining_session_is_caught_up_on_the_loops_in_earshot() {
        let mut server = crate::server::session_build::build_server_inline("", 1, 2);
        let joiner = crate::server::session_build::spawn_player(server.world.data().seed);
        let restart = SpatialSoundMsg::PlayAt {
            handle: 7,
            sound_id: 0,
            pos: joiner.pos,
            volume: 0.3,
            pitch: 1.0,
        };
        let s = server.add_session_for_test(joiner);
        let joiner_id = server.sessions[s].id;
        server.broadcast.spatial_loops_mut().insert(7, restart);

        let out = server.pump(0.06, &mut Vec::new());
        let spatial_events = |msgs: &[ServerToClient]| -> Vec<SpatialSoundMsg> {
            msgs.iter()
                .filter_map(|m| match m {
                    ServerToClient::Tick(update) => update.events(),
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
