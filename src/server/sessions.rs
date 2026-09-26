//! The session registry: every connected player's simulation session, in
//! roster order, plus whether index 0 is this process's local player.
//!
//! The registry is the one owner of the session list. Joins and leaves go
//! through [`SessionRegistry::join`] / [`SessionRegistry::leave`] so the
//! listen-server invariant (the local session stays at index 0 for the whole
//! run) is kept in one place; everything else reads and mutates sessions in
//! place through the slice it derefs to. It is also the [`PlayerRoster`]
//! every mod dispatch receives: a handler reaches a player only by id
//! through it.

use std::ops::{Deref, DerefMut};
use std::sync::Arc;

use petramond_world::gui_state::GuiStateMap;

use crate::events::{OpenGui, PlayerRoster};
use crate::player::{Player, PlayerId};
use crate::server::player::ConnectedPlayer;

/// The connected sessions. On a LISTEN server (the in-game host) the LOCAL
/// session is index 0 and always exists; on a HEADLESS server every session
/// is remote and the list may be EMPTY — fixed ticks are skipped while it is
/// (the world freezes between players).
pub struct SessionRegistry {
    sessions: Vec<ConnectedPlayer>,
    /// Whether index 0 is THIS process's local player (listen server).
    has_local: bool,
    /// Most sessions admitted at once, the local one included (see
    /// [`set_capacity`](Self::set_capacity)).
    capacity: usize,
    /// Sessions whose work panicked this pump, awaiting eviction (see
    /// `server::game::isolation`).
    faulted: Vec<PlayerId>,
}

/// How many sessions distinct `PlayerId`s can name: the ceiling any
/// configured player cap is clamped to.
pub const MAX_SESSIONS: usize = u8::MAX as usize + 1;

impl SessionRegistry {
    /// A listen server's registry around its local session, or a headless
    /// server's empty one.
    pub fn new(local: Option<ConnectedPlayer>) -> Self {
        Self {
            has_local: local.is_some(),
            sessions: local.into_iter().collect(),
            capacity: MAX_SESSIONS,
            faulted: Vec::new(),
        }
    }

    /// Mark session `id` faulted: skipped by every isolated stage until the
    /// pump evicts it.
    pub fn mark_faulted(&mut self, id: PlayerId) {
        if !self.faulted.contains(&id) {
            self.faulted.push(id);
        }
    }

    /// Whether the session at `index` is awaiting eviction.
    pub fn is_faulted(&self, index: usize) -> bool {
        self.faulted.contains(&self.sessions[index].id)
    }

    /// Every faulted session id, clearing the set.
    pub fn take_faulted(&mut self) -> Vec<PlayerId> {
        std::mem::take(&mut self.faulted)
    }

    /// Cap how many sessions may be connected at once (a headless server's
    /// `max_players`), clamped to `1..=`[`MAX_SESSIONS`]. Lowering it below
    /// the current count never kicks anyone; it only refuses further joins.
    pub fn set_capacity(&mut self, max_players: usize) {
        self.capacity = max_players.clamp(1, MAX_SESSIONS);
    }

    /// Most sessions admitted at once, the local one included.
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Whether index 0 is this process's local player. False on a headless
    /// server: no local pipe recipient, every session windowed by the
    /// streaming ack loop, and the leave path may empty the list.
    #[inline]
    pub fn has_local_session(&self) -> bool {
        self.has_local
    }

    /// The local session's id (always index 0 on a listen server); `None` on
    /// a headless server, whose sessions are all remote.
    pub fn local_id(&self) -> Option<PlayerId> {
        self.has_local.then(|| self.sessions[0].id)
    }

    /// The roster index of session `id`.
    pub fn index_of(&self, id: PlayerId) -> Option<usize> {
        self.sessions.iter().position(|sess| sess.id == id)
    }

    /// Session `id`, if connected.
    pub fn by_id(&self, id: PlayerId) -> Option<&ConnectedPlayer> {
        self.sessions.iter().find(|sess| sess.id == id)
    }

    /// Append a joining session; returns its roster index.
    pub fn join(&mut self, session: ConnectedPlayer) -> usize {
        self.sessions.push(session);
        self.sessions.len() - 1
    }

    /// Remove the session at `index` and hand it back. `swap_remove` keeps a
    /// listen server's local session at index 0 (it never leaves — only
    /// `index >= 1` is ever removed there) and every survivor's `PlayerId`
    /// rides with its element; nothing stores session INDICES across a leave.
    pub fn leave(&mut self, index: usize) -> ConnectedPlayer {
        debug_assert!(
            !(index == 0 && self.has_local),
            "the local session never leaves"
        );
        self.sessions.swap_remove(index)
    }

    /// Drop every session (a test fixture emptying a server to the headless
    /// shape).
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

    /// Joins append, leaves swap-remove without ever moving the local
    /// session, freed ids recycle, and the roster a mod dispatch receives
    /// resolves the same ids to the same slots.
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

    /// The configured cap is clamped to what `PlayerId`s can name.
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
