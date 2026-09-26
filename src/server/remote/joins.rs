use crate::net::identity::PlayerKey;
use crate::net::protocol::{ItemSlotWire, JoinData, JoinRejectReason, SelfRestore};
use crate::player::PlayerId;
use crate::server::game::{wire_world_events, ServerGame};
use crate::server::player::ConnectedPlayer;

impl ServerGame {
    /// Whether an authenticated `key` may join right now: refused while that
    /// identity is already connected, or when every `PlayerId` is taken.
    /// Checked BEFORE the connection's I/O threads are spawned, so a refusal
    /// costs nothing; [`admit_remote_player`](Self::admit_remote_player)
    /// re-checks it.
    pub fn check_admission(&self, key: &PlayerKey) -> Result<(), JoinRejectReason> {
        if self.sessions.iter().any(|s| s.key == *key) {
            return Err(JoinRejectReason::AlreadyConnected);
        }
        if self.sessions.next_free_id().is_none() {
            return Err(JoinRejectReason::ServerFull);
        }
        Ok(())
    }

    /// Admit the authenticated identity `key` as a new remote session and
    /// return the `JoinAccept` payload plus the session's FINAL name.
    /// `requested` is a display name already validated at the edge
    /// (`net::identity::validate_player_name`). A join is never refused for
    /// its name: when another identity owns it (or a connected session uses
    /// it) the lowest free numeric suffix is appended ("Rachel" → "Rachel2")
    /// — see `server::accounts`. The player restores from `key`'s own save
    /// file (never from the name's), else a fresh surface spawn — exactly the
    /// local session's restore path.
    pub fn admit_remote_player(
        &mut self,
        key: PlayerKey,
        requested: &str,
        view_distance: i32,
        cached_sections: &[crate::net::protocol::SectionCacheClaim],
    ) -> Result<(Box<JoinData>, String), JoinRejectReason> {
        self.check_admission(&key)?;
        let id = self
            .sessions
            .next_free_id()
            .expect("check_admission found a free id");
        let sessions = &self.sessions;
        let claim = self
            .accounts
            .claim(self.world.save(), key, requested, |candidate| {
                sessions
                    .iter()
                    .any(|s| s.key != key && s.name.eq_ignore_ascii_case(candidate))
            });
        if claim.first_seen && self.operators.claim_legacy(&claim.name, key) {
            crate::server::permissions::store(&mut self.world, &self.operators);
        }
        let name = claim.name;
        let mut player = claim
            .restored
            .unwrap_or_else(|| crate::server::session_build::spawn_player(self.world.seed));
        // Reconcile the restored record against this world's catalog before
        // the handshake ships it (see `server::progression::catch_up`).
        crate::server::progression::catch_up(&mut player, self.catalog.unlocks());
        let data = Box::new(JoinData {
            player_id: id,
            seed: self.world.seed,
            clock: crate::server::daynight::current_clock(&self.world),
            tables: crate::net::remap::local_name_tables(),
            self_restore: self_restore_from(&player),
            crafting_recipes: self.catalog.recipes().crafting().to_data(),
            players: self
                .sessions
                .iter()
                .map(|s| (s.id, s.name.clone()))
                .collect(),
        });
        let mut session = ConnectedPlayer::new(id, key, name.clone(), player, view_distance);
        // The handshake already carried the full unlocked list.
        session.replication.sent_unlock_count = session.player.progression.unlocked().len();
        session.transport.terrain.seed_client_cache(cached_sections);
        // Reseed the env params for the newcomer: a static param map would
        // otherwise never reach them.
        self.broadcast.reseed_env();
        self.broadcast.replay_spatial_loops(&mut session);
        self.sessions.join(session);
        Ok((data, name))
    }

    /// The LOCAL session's join payload — the in-process twin of the
    /// `JoinAccept` a remote client receives, so the listen client boots
    /// through the same `JoinData` path instead of reading server memory.
    /// `None` on a headless server (no local session).
    pub fn local_join_data(&self) -> Option<Box<JoinData>> {
        if !self.sessions.has_local_session() {
            return None;
        }
        let local = self.sessions.first()?;
        Some(Box::new(JoinData {
            player_id: local.id,
            seed: self.world.seed,
            clock: crate::server::daynight::current_clock(&self.world),
            tables: crate::net::remap::local_name_tables(),
            self_restore: self_restore_from(&local.player),
            crafting_recipes: self.catalog.recipes().crafting().to_data(),
            players: self.sessions[1..]
                .iter()
                .map(|s| (s.id, s.name.clone()))
                .collect(),
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
        self.broadcast.bank_wire_events(wire_world_events(&mut events.world));
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

/// The joining player's `SelfRestore`, mirrored off the restored session
/// player (wire ids are raw server ids; effects travel by name).
fn self_restore_from(player: &crate::player::Player) -> SelfRestore {
    SelfRestore {
        transform: crate::net::protocol::Transform {
            pos: player.pos,
            vel: player.vel,
            yaw: player.yaw,
            pitch: player.pitch,
        },
        mode: player.mode().to_u8(),
        health: player.health(),
        bed_spawn: player.bed_spawn.map(|b| (b.bed, b.spot)),
        effects: player
            .effects()
            .iter()
            .map(|e| (e.effect.def().name.to_string(), e.remaining))
            .collect(),
        inventory: player
            .inventory
            .raw_slots()
            .iter()
            .copied()
            .chain(std::iter::once(player.inventory.cursor().copied()))
            .chain(std::iter::once(player.inventory.off_hand().copied()))
            .map(|slot| slot.map(ItemSlotWire::from_stack))
            .collect(),
        active_slot: player.inventory.active_slot(),
        craft_craftable_only: player.craft_craftable_only,
        unlocked_recipes: player.progression.unlocked().to_vec(),
    }
}
