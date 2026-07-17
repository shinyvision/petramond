use crate::game::tick::TickEvents;
#[cfg(test)]
use crate::server::player::ConnectedPlayer;
use crate::server::player::PlayerId;

use super::ServerGame;

impl ServerGame {
    /// Test-only: connect a second (remote-shaped) session and return its index.
    #[cfg(test)]
    pub(crate) fn add_session_for_test(&mut self, player: crate::player::Player) -> usize {
        let id = crate::server::player::PlayerId(self.sessions.len() as u8);
        let radius = self.world.render_dist;
        // A fresh session must receive the CURRENT env params even when the
        // map is static (a frozen clock freezes day/night AND weather params;
        // without this reseed a late joiner would render a default sky until
        // anything changed).
        self.last_shipped_env = None;
        self.sessions.push(ConnectedPlayer::new(
            id,
            format!("Player{}", id.0),
            player,
            radius,
        ));
        self.sessions.len() - 1
    }

    /// Persist everything: flush modified chunks to the save thread, then write
    /// `level.dat` (seed + world tick + mod world KV), one `players/<name>.dat`
    /// per connected session, and the save's mod-set record (`mods.json`). A
    /// mounted session is encoded from a safely dismounted clone because the
    /// attachment itself is transient; the live autosave state stays mounted,
    /// and a player write defers if no detached position is provably safe. A
    /// no-op without an attached save.
    pub(crate) fn save_all(&mut self) {
        self.world.flush_modified_chunks();
        if self.world.save().is_none() {
            return;
        }

        let obstacles = self.world.mobs().solid_obstacles();
        let players: Vec<_> = self
            .sessions
            .iter()
            .enumerate()
            .filter_map(|(s, session)| {
                let Some(mut snapshot) = self.player_snapshot_for_save(s, &obstacles) else {
                    log::debug!(
                        "deferring player save for '{}': no stream-final detached riding position",
                        session.name
                    );
                    return None;
                };
                let complete = session
                    .menu
                    .unpersisted_items()
                    .into_iter()
                    .flatten()
                    .all(|stack| snapshot.inventory.add(stack).is_none());
                if !complete {
                    log::debug!(
                        "deferring player save for '{}': transient menu items do not fit",
                        session.name
                    );
                    return None;
                }
                Some((session.name.clone(), crate::save::player::encode(&snapshot)))
            })
            .collect();

        if let Some(save) = self.world.save() {
            save.save_level(crate::save::level::encode(
                self.world.seed,
                self.world.current_tick(),
                self.world.mod_kv(),
                self.world.populated_columns(),
            ));
            for (name, bytes) in players {
                save.save_player(&name, bytes);
            }
            save.save_mods_json(crate::modding::modset::encode_active(
                self.world.disabled_mods(),
            ));
        }
    }

    /// Final persistence boundary. Menu state is intentionally transient, so
    /// recover every cursor/crafting/workbench stack (and materialize safe
    /// overflow drops) before encoding players and world entities. This runs
    /// independently of fixed ticks and therefore also works while paused.
    pub(crate) fn close_sessions_and_save(&mut self) {
        let mut events = TickEvents::with_next_spatial_sound_handle(self.next_mod_sound_handle);
        for s in 0..self.sessions.len() {
            self.close_open_menu_for(s, &mut events);
            self.tick_drops(s, &mut events);
        }
        self.next_mod_sound_handle = events.next_spatial_sound_handle();
        self.save_all();
    }

    pub(crate) fn maybe_autosave(&mut self, dt: f32) {
        const AUTOSAVE_SECS: f32 = 30.0;
        if self.world.save().is_none() {
            return;
        }
        self.autosave_t += dt;
        if self.autosave_t >= AUTOSAVE_SECS {
            self.autosave_t = 0.0;
            self.save_all();
        }
    }

    /// The local session's id (always index 0 on a listen server); `None` on
    /// a headless server, whose sessions are all remote.
    pub(crate) fn local_session_id(&self) -> Option<PlayerId> {
        self.has_local_session.then(|| self.sessions[0].id)
    }
}
