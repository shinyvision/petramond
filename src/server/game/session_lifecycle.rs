use crate::player::PlayerId;
#[cfg(any(test, feature = "test-support"))]
use crate::server::player::ConnectedPlayer;

use super::ServerGame;

impl ServerGame {
    /// Register the engine's own policies on the seams — day/night on the
    /// tick stages, recipe unlocks on the bus — before any mod registers, so
    /// mods sort behind the engine at equal priority.
    pub(in crate::server) fn install_core_systems(&mut self) {
        crate::server::daynight::install_core(&mut self.world, self.mods.systems_mut());
        crate::server::progression::install_core(
            self.mods.bus_mut(),
            self.catalog.unlocks().clone(),
        );
    }

    /// Reconcile every session's restored progression against this world's
    /// catalog (a pack installed since the player last played); the
    /// handshake then carries the whole list, so nothing is owed.
    pub(in crate::server) fn catch_up_sessions(&mut self) {
        for sess in &mut self.sessions {
            crate::server::progression::catch_up(&mut sess.player, self.catalog.unlocks());
            sess.replication.sent_unlock_count = sess.player.progression.unlocked().len();
        }
    }

    /// Test-only: connect a second (remote-shaped) session and return its index.
    #[cfg(any(test, feature = "test-support"))]
    pub fn add_session_for_test(&mut self, player: crate::player::Player) -> usize {
        let id = crate::player::PlayerId(self.sessions.len() as u8);
        let radius = self.world.render_dist;
        // A fresh session must receive the CURRENT env params even when the
        // map is static (a frozen clock freezes day/night AND weather params;
        // without this reseed a late joiner would render a default sky until
        // anything changed).
        self.broadcast.reseed_env();
        let s = self.sessions.join(ConnectedPlayer::new(
            id,
            crate::net::identity::PlayerKey([id.0; 32]),
            format!("Player{}", id.0),
            player,
            radius,
        ));
        self.unlock_all_recipes_for_test(s);
        self.broadcast
            .replay_spatial_loops(&mut self.sessions[s]);
        s
    }

    /// Test-only: install a recipe catalog and open all of it for every
    /// session — the fixture twin of session start, where the catalog and the
    /// player's unlocked record arrive together. Tests about crafting
    /// mechanics are not tests about discovery.
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
        // A real join carries the whole unlocked list in the handshake, so
        // the pump has nothing to catch this session up on — mirror that, or
        // fixtures see a `RecipesUnlocked` message no real session would get.
        self.sessions[s].replication.sent_unlock_count = self.sessions[s].player.progression.unlocked().len();
    }

    /// Persist everything: flush modified chunks to the save thread, then write
    /// `level.dat` (seed + world tick + mod world KV), one `players/<name>.dat`
    /// per connected session, and the save's mod-set record (`mods.json`). A
    /// mounted session is encoded from a safely dismounted clone because the
    /// attachment itself is transient; the live autosave state stays mounted,
    /// and a player write defers if no detached position is provably safe. A
    /// no-op without an attached save.
    pub fn save_all(&mut self) {
        let Some(save) = self.world.save() else {
            return;
        };
        // Sections, the level (mod world KV) and every player land together:
        // as of this save, items moved between a chest, a mob and a player
        // are on disk on both sides or neither.
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
                    .sim.menu
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
                self.world.seed,
                self.world.current_tick(),
                self.world.world_kv(),
                self.world.populated_columns(),
            ));
            for (key, snapshot) in &players {
                save.save_player(key, snapshot);
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
    pub fn close_sessions_and_save(&mut self) {
        let mut events = self.mods.open_feed();
        for s in 0..self.sessions.len() {
            self.close_open_menu_for(s, &mut events);
            self.tick_drops(s, &mut events);
        }
        self.mods.settle_feed(&events);
        self.save_all();
    }

    /// Autosave on the frame clock's cadence (a no-op without a save).
    pub fn maybe_autosave(&mut self, dt: f32) {
        if self.world.save().is_some() && self.clock.autosave_due(dt) {
            self.save_all();
        }
    }

    /// The local session's id (always index 0 on a listen server); `None` on
    /// a headless server, whose sessions are all remote.
    pub fn local_session_id(&self) -> Option<PlayerId> {
        self.sessions.local_id()
    }
}
