use std::ops::{Deref, DerefMut};
use std::sync::Arc;

use petramond_world::gui_state::GuiStateMap;

use crate::events::{OpenGui, PlayerRoster};
use crate::player::{Player, PlayerId};
use crate::server::player::ConnectedPlayer;

pub struct SessionRegistry {
    sessions: Vec<ConnectedPlayer>,
    has_local: bool,
    capacity: usize,
    faulted: Vec<PlayerId>,
}

pub const MAX_SESSIONS: usize = u8::MAX as usize + 1;

impl SessionRegistry {
    pub fn new(local: Option<ConnectedPlayer>) -> Self {
        Self {
            has_local: local.is_some(),
            sessions: local.into_iter().collect(),
            capacity: MAX_SESSIONS,
            faulted: Vec::new(),
        }
    }

    pub fn mark_faulted(&mut self, id: PlayerId) {
        if !self.faulted.contains(&id) {
            self.faulted.push(id);
        }
    }

    pub fn is_faulted(&self, index: usize) -> bool {
        self.faulted.contains(&self.sessions[index].id)
    }

    pub fn take_faulted(&mut self) -> Vec<PlayerId> {
        std::mem::take(&mut self.faulted)
    }

    pub fn set_capacity(&mut self, max_players: usize) {
        self.capacity = max_players.clamp(1, MAX_SESSIONS);
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    #[inline]
    pub fn has_local_session(&self) -> bool {
        self.has_local
    }

    pub fn local_id(&self) -> Option<PlayerId> {
        self.has_local.then(|| self.sessions[0].id)
    }

    pub fn index_of(&self, id: PlayerId) -> Option<usize> {
        self.sessions.iter().position(|sess| sess.id == id)
    }

    pub fn by_id(&self, id: PlayerId) -> Option<&ConnectedPlayer> {
        self.sessions.iter().find(|sess| sess.id == id)
    }

    pub fn join(&mut self, session: ConnectedPlayer) -> usize {
        self.sessions.push(session);
        self.sessions.len() - 1
    }

    pub fn leave(&mut self, index: usize) -> ConnectedPlayer {
        debug_assert!(
            !(index == 0 && self.has_local),
            "the local session never leaves"
        );
        self.sessions.swap_remove(index)
    }

    #[cfg(test)]
    pub fn clear_for_test(&mut self) {
        self.sessions.clear();
        self.has_local = false;
    }
}

impl Deref for SessionRegistry {
    type Target = [ConnectedPlayer];

    #[inline]
    fn deref(&self) -> &[ConnectedPlayer] {
        &self.sessions
    }
}

impl DerefMut for SessionRegistry {
    #[inline]
    fn deref_mut(&mut self) -> &mut [ConnectedPlayer] {
        &mut self.sessions
    }
}

impl<'a> IntoIterator for &'a SessionRegistry {
    type Item = &'a ConnectedPlayer;
    type IntoIter = std::slice::Iter<'a, ConnectedPlayer>;

    fn into_iter(self) -> Self::IntoIter {
        self.sessions.iter()
    }
}

impl<'a> IntoIterator for &'a mut SessionRegistry {
    type Item = &'a mut ConnectedPlayer;
    type IntoIter = std::slice::IterMut<'a, ConnectedPlayer>;

    fn into_iter(self) -> Self::IntoIter {
        self.sessions.iter_mut()
    }
}

impl PlayerRoster for SessionRegistry {
    fn len(&self) -> usize {
        self.sessions.len()
    }

    fn id_at(&self, index: usize) -> PlayerId {
        self.sessions[index].id
    }

    fn player_at(&mut self, index: usize) -> &mut Player {
        &mut self.sessions[index].player
    }

    fn gui_state_at(&mut self, index: usize) -> &mut Arc<GuiStateMap> {
        &mut self.sessions[index].sim.gui_state
    }

    fn open_gui_at(&self, index: usize) -> Option<OpenGui> {
        self.sessions[index].open_gui()
    }

    fn index_of(&self, id: PlayerId) -> Option<usize> {
        SessionRegistry::index_of(self, id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use petramond_math::world_pos::WorldPos;

    fn session(id: u8) -> ConnectedPlayer {
        ConnectedPlayer::new(
            PlayerId(id),
            crate::net::identity::PlayerKey([id; 32]),
            format!("P{id}"),
            Player::new(WorldPos::new(0.0, 80.0, 0.0)),
            8,
        )
    }

    #[test]
    fn joins_and_leaves_keep_the_local_session_first_and_ids_resolvable() {
        let mut registry = SessionRegistry::new(Some(session(0)));
        assert!(registry.has_local_session());
        assert_eq!(registry.local_id(), Some(PlayerId(0)));

        registry.join(session(1));
        registry.join(session(2));
        assert_eq!(registry.leave(1).id, PlayerId(1));
        assert_eq!(registry[0].id, PlayerId(0), "the local session never moves");
        assert_eq!(
            registry.index_of(PlayerId(2)),
            Some(1),
            "the survivor took the slot"
        );
        assert!(
            registry.by_id(PlayerId(1)).is_none(),
            "the leaver's id is free"
        );

        let roster: &mut dyn PlayerRoster = &mut registry;
        assert_eq!(roster.len(), 2);
        assert_eq!(roster.index_of(PlayerId(2)), Some(1));
        assert_eq!(roster.id_at(1), PlayerId(2));
        assert!(SessionRegistry::new(None).local_id().is_none());
    }

    #[test]
    fn the_player_cap_is_clamped_to_the_id_space() {
        let mut registry = SessionRegistry::new(None);
        assert_eq!(registry.capacity(), MAX_SESSIONS);
        registry.set_capacity(0);
        assert_eq!(registry.capacity(), 1);
        registry.set_capacity(usize::MAX);
        assert_eq!(registry.capacity(), MAX_SESSIONS);
    }
}
