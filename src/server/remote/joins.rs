use crate::game::tick::TickEvents;
use crate::net::protocol::{ItemSlotWire, JoinData, SelfRestore};
use crate::server::game::{wire_world_events, ServerGame};
use crate::server::player::{ConnectedPlayer, PlayerId};

impl ServerGame {
    /// Admit `requested` as a new remote session and return the `JoinAccept`
    /// payload plus the session's FINAL name. A join is never refused for its
    /// name: if `requested` is taken (case-insensitive, vs connected
    /// sessions), the lowest free numeric suffix is appended ("Rachel" →
    /// "Rachel2" → "Rachel3") and the suffixed name IS the session's name —
    /// it keys `players/<name>.dat`, so a deduped guest saves AND restores as
    /// the suffixed name (a returning "Rachel" only sees "Rachel2"'s state
    /// while the original "Rachel" is still connected to claim the base
    /// name). Restore from `players/<name>.dat` when the world has a save,
    /// else a fresh surface spawn — exactly the local session's restore path.
    pub(crate) fn admit_remote_player(
        &mut self,
        requested: &str,
        view_distance: i32,
        cached_sections: &[crate::net::protocol::SectionCacheClaim],
    ) -> (Box<JoinData>, String) {
        let name = self.dedupe_player_name(requested);
        let id = self.next_free_player_id();
        let player = self
            .world
            .save()
            .and_then(|save| save.load_player(&name))
            .and_then(|bytes| crate::save::player::decode(&bytes))
            .map(|data| data.restore())
            .unwrap_or_else(|| crate::game::session::spawn_player(self.world.seed));
        let data = Box::new(JoinData {
            player_id: id,
            seed: self.world.seed,
            clock: crate::server::daynight::current_clock(&self.world),
            tables: crate::net::remap::local_name_tables(),
            self_restore: self_restore_from(&player),
            crafting_recipes: self.recipes.crafting().to_data(),
            players: self
                .sessions
                .iter()
                .map(|s| (s.id, s.name.clone()))
                .collect(),
        });
        let mut session = ConnectedPlayer::new(id, name.clone(), player, view_distance);
        session.terrain.seed_client_cache(cached_sections);
        // Reseed the env params for the newcomer (see the local-join twin in
        // game.rs): a static param map would otherwise never reach them.
        self.last_shipped_env = None;
        self.sessions.push(session);
        (data, name)
    }

    /// The requested name, or — when a connected session already uses it
    /// (case-insensitive) — the requested name with the lowest free numeric
    /// suffix appended (`{name}2`, `{name}3`, …).
    fn dedupe_player_name(&self, requested: &str) -> String {
        let taken = |candidate: &str| {
            self.sessions
                .iter()
                .any(|s| s.name.eq_ignore_ascii_case(candidate))
        };
        if !taken(requested) {
            return requested.to_string();
        }
        (2u32..)
            .map(|n| format!("{requested}{n}"))
            .find(|candidate| !taken(candidate))
            .expect("fewer than u32::MAX sessions")
    }

    /// The smallest `PlayerId` no connected session uses (freed ids recycle).
    fn next_free_player_id(&self) -> PlayerId {
        (0u16..=u8::MAX as u16)
            .map(|i| PlayerId(i as u8))
            .find(|id| !self.sessions.iter().any(|s| s.id == *id))
            .expect("fewer than 256 sessions")
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
    pub(crate) fn remove_remote_session(&mut self, id: PlayerId) -> Option<String> {
        let s = self.sessions.iter().position(|x| x.id == id)?;
        if s == 0 && self.has_local_session {
            debug_assert!(false, "the local session never leaves");
            return None;
        }
        let mut events = TickEvents::with_next_spatial_sound_handle(self.next_mod_sound_handle);
        self.close_open_menu_for(s, &mut events);
        self.tick_drops(s, &mut events);
        self.next_mod_sound_handle = events.next_spatial_sound_handle();
        self.pending_wire_events
            .extend(wire_world_events(&mut events.world));
        let obstacles = self.world.mobs().solid_obstacles();
        let snapshot = self.player_snapshot_for_save(s, &obstacles);
        self.detach_departing_session(s);
        if let Some(save) = self.world.save() {
            if let Some(snapshot) = snapshot {
                save.save_player(
                    &self.sessions[s].name,
                    crate::save::player::encode(&snapshot),
                );
            } else {
                log::debug!(
                    "deferring final player save for '{}': no stream-final detached riding position",
                    self.sessions[s].name
                );
            }
        }
        Some(self.sessions.swap_remove(s).name)
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
        mode: match player.mode() {
            crate::player::PlayerMode::Survival => 0,
            crate::player::PlayerMode::Spectator => 1,
        },
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
            .map(|slot| {
                slot.map(|st| ItemSlotWire {
                    item_id: st.item.0,
                    count: st.count,
                })
            })
            .collect(),
        active_slot: player.inventory.active_slot(),
        craft_craftable_only: player.craft_craftable_only,
    }
}
