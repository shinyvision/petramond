use crate::player::PlayerId;
#[cfg(any(test, feature = "test-support"))]
use crate::server::player::ConnectedPlayer;

use super::ServerGame;

impl ServerGame {
    pub(in crate::server) fn install_core_systems(&mut self) {
        crate::server::daynight::install_core(&mut self.world, self.mods.systems_mut());
        crate::server::progression::install_core(
            self.mods.bus_mut(),
            self.catalog.unlocks().clone(),
        );
    }

    pub(in crate::server) fn catch_up_sessions(&mut self) {
        for sess in &mut self.sessions {
            crate::server::progression::catch_up(&mut sess.player, self.catalog.unlocks());
            sess.replication.sent_unlock_count = sess.player.progression.unlocked().len();
        }
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn add_session_for_test(&mut self, player: crate::player::Player) -> usize {
        let id = crate::player::PlayerId(self.sessions.len() as u8);
        let radius = self.world.data().render_dist;
        self.broadcast.reseed_env();
        let s = self.sessions.join(ConnectedPlayer::new(
            id,
            crate::net::identity::PlayerKey([id.0; 32]),
            format!("Player{}", id.0),
            player,
            radius,
        ));
        self.unlock_all_recipes_for_test(s);
        s
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn install_recipes_for_test(&mut self, recipes: petramond_world::crafting::Recipes) {
        self.catalog = crate::server::progression::RecipeCatalog::new(recipes);
        for s in 0..self.sessions.len() {
            self.unlock_all_recipes_for_test(s);
        }
    }

    #[cfg(any(test, feature = "test-support"))]
    fn unlock_all_recipes_for_test(&mut self, s: usize) {
        let keys: Vec<String> = self
            .catalog
            .recipes()
            .crafting()
            .iter()
            .map(|r| r.key().to_owned())
            .collect();
        for key in keys {
            self.sessions[s].player.progression.unlock(&key);
        }
        self.sessions[s].replication.sent_unlock_count =
            self.sessions[s].player.progression.unlocked().len();
    }

    /// Saves everything. Modified chunks go to the save thread, then we write `level.dat`, a
    /// `players/<name>.dat` for each connected session and `mods.json`. Riders are saved from a
    /// dismounted clone, since being mounted is transient, while the live session stays mounted. If
    /// there's no provably safe spot to put them down, we put off that player's write.
    pub fn save_all(&mut self) {
        let Some(save) = self.world.save() else {
            return;
        };
        let batch = save.begin_batch();
        self.save_all_batched();
        drop(batch);
    }

    fn save_all_batched(&mut self) {
        self.world.flush_modified_chunks();

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
                    .sim
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
                Some((session.key, snapshot))
            })
            .collect();

        if let Some(save) = self.world.save() {
            save.save_level(crate::save::level::encode(
                self.world.data().seed,
                self.world.current_tick(),
                self.world.data().world_kv(),
                self.world.populated_columns(),
            ));
            for (key, snapshot) in &players {
                save.save_player(key, snapshot);
            }
            save.save_mods_json(crate::modding::modset::encode_active(
                self.world.data().disabled_mods(),
            ));
        }
    }

    pub fn close_sessions_and_save(&mut self) {
        let mut events = self.mods.open_feed();
        for s in 0..self.sessions.len() {
            self.close_open_menu_for(s, &mut events);
            self.tick_drops(s, &mut events);
        }
        self.mods.settle_feed(&events);
        self.save_all();
    }

    pub fn maybe_autosave(&mut self, dt: f32) {
        if self.world.save().is_some() && self.clock.autosave_due(dt) {
            self.save_all();
        }
    }

    pub fn local_session_id(&self) -> Option<PlayerId> {
        self.sessions.local_id()
    }
}
