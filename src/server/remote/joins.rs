use crate::net::identity::PlayerKey;
use crate::net::protocol::JoinData;
use crate::player::PlayerId;
use crate::server::admissions::self_restore_from;
use crate::server::game::{wire_world_events, ServerGame};

impl ServerGame {
    pub fn local_join_data(&self) -> Option<Box<JoinData>> {
        if !self.sessions.has_local_session() {
            return None;
        }
        let local = self.sessions.first()?;
        Some(Box::new(JoinData {
            player_id: local.id,
            player_name: local.name.clone(),
            seed: self.world.data().seed,
            clock: crate::server::daynight::current_clock(&self.world),
            tables: crate::net::remap::local_name_tables(),
            self_restore: self_restore_from(&local.player),
            crafting_recipes: self.catalog.recipes().crafting().to_data(),
            players: self.sessions[1..]
                .iter()
                .map(|s| (s.id, s.name.clone()))
                .collect(),
            client_policy: self.client_policy,
        }))
    }

    /// The leave path, in order: close the open menu (cursor/craft returns,
    /// chest-viewer release), flush the drop queue into the world (overflow
    /// from those returns must not vanish), prepare a safe detached snapshot,
    /// detach riding state, persist that snapshot, remove the session, and bank
    /// the close's world events for the next tick batch. Returns the leaver's
    /// name (`None` = no such session).
    ///
    /// `swap_remove` keeps a LISTEN server's local session at index 0 (only
    /// `s >= 1` is ever removed there; a headless server may remove any
    /// session, down to an empty list) and every survivor's `PlayerId` rides
    /// with its element; nothing stores session INDICES across loop
    /// iterations — the hub re-resolves ids at every drain, and the pump
    /// resolves its tagged inbox against the post-leave list.
    pub fn remove_remote_session(&mut self, id: PlayerId) -> Option<String> {
        let s = self.sessions.index_of(id)?;
        if s == 0 && self.sessions.has_local_session() {
            debug_assert!(false, "the local session never leaves");
            return None;
        }
        let mut events = self.mods.open_feed();
        self.close_open_menu_for(s, &mut events);
        self.tick_drops(s, &mut events);
        self.mods.settle_feed(&events);
        self.broadcast
            .bank_wire_events(wire_world_events(&mut events.world));
        let obstacles = self.world.mobs().solid_obstacles();
        let snapshot = self.player_snapshot_for_save(s, &obstacles);
        self.detach_departing_session(s);
        if let Some(save) = self.world.save() {
            if let Some(snapshot) = snapshot {
                save.save_player(&self.sessions[s].key, &snapshot);
            } else {
                log::debug!(
                    "deferring final player save for '{}': no stream-final detached riding position",
                    self.sessions[s].name
                );
            }
        }
        Some(self.sessions.leave(s).name)
    }
}

pub(in crate::server) fn account_key(user_id: i64) -> PlayerKey {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"petramond/account-identity/v1\0");
    hasher.update(&user_id.to_le_bytes());
    PlayerKey(*hasher.finalize().as_bytes())
}
