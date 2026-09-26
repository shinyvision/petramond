//! The connected sessions a simulation dispatch can reach.
//!
//! Every [`SimCtx`](super::SimCtx) carries the roster EXPLICITLY — a
//! `&mut dyn PlayerRoster` borrowed from whoever owns the sessions (the
//! server's session registry, or a test fixture's [`RosterRefs`]). A handler
//! reaches a player only by id through it, so there is no implicit "current
//! player" to lend and no second path to any player: the roster is the one
//! borrow.

use std::sync::Arc;

use petramond_world::gui_state::GuiStateMap;

use crate::player::{Player, PlayerId};

/// One session's open GUI, as the roster publishes it.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct OpenGui {
    pub kind: petramond_world::gui_state::GuiKind,
    pub anchor: Option<crate::menu::MenuAnchor>,
}

/// The sessions one dispatch may address, by roster index `0..len()`. The
/// index is the slot the session's per-player tick events live in
/// ([`TickEvents::player`](super::tick::TickEvents::player)); the stable
/// address a mod speaks is the [`PlayerId`] ([`index_of`](Self::index_of)).
pub trait PlayerRoster {
    /// How many sessions are connected.
    fn len(&self) -> usize;

    /// Whether no session is connected (a headless server between players,
    /// mod init, an actor-less fixture).
    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The stable id of the session at `index`. Panics out of range.
    fn id_at(&self, index: usize) -> PlayerId;

    /// The authoritative player of the session at `index`.
    fn player_at(&mut self, index: usize) -> &mut Player;

    /// The mod-GUI state map of the session at `index`.
    fn gui_state_at(&mut self, index: usize) -> &mut Arc<GuiStateMap>;

    /// What the session at `index` has open (`None` = nothing, or a non-GUI
    /// screen).
    fn open_gui_at(&self, index: usize) -> Option<OpenGui>;

    /// The roster index of session `id`, or `None` when it is not connected.
    fn index_of(&self, id: PlayerId) -> Option<usize> {
        (0..self.len()).find(|&i| self.id_at(i) == id)
    }
}

/// One session lent into a [`RosterRefs`]: its stable id, its authoritative
/// player, its own mod-GUI state map and what it currently has open.
pub struct SessionPlayerRef<'a> {
    pub id: PlayerId,
    pub player: &'a mut Player,
    pub gui_state: &'a mut Arc<GuiStateMap>,
    pub gui: Option<OpenGui>,
}

/// A roster over borrowed sessions, in roster-index order — what a dispatch
/// outside the server (mod init, unit fixtures) passes. [`RosterRefs::empty`]
/// is the roster of a context with nobody in it.
#[derive(Default)]
pub struct RosterRefs<'a> {
    sessions: Vec<SessionPlayerRef<'a>>,
}

impl<'a> RosterRefs<'a> {
    pub fn new(sessions: Vec<SessionPlayerRef<'a>>) -> Self {
        Self { sessions }
    }

    /// No sessions at all.
    pub fn empty() -> Self {
        Self::default()
    }
}

impl PlayerRoster for RosterRefs<'_> {
    fn len(&self) -> usize {
        self.sessions.len()
    }

    fn id_at(&self, index: usize) -> PlayerId {
        self.sessions[index].id
    }

    fn player_at(&mut self, index: usize) -> &mut Player {
        &mut *self.sessions[index].player
    }

    fn gui_state_at(&mut self, index: usize) -> &mut Arc<GuiStateMap> {
        &mut *self.sessions[index].gui_state
    }

    fn open_gui_at(&self, index: usize) -> Option<OpenGui> {
        self.sessions[index].gui
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use petramond_math::world_pos::WorldPos;

    #[test]
    fn a_borrowed_roster_resolves_ids_to_the_lent_sessions() {
        let mut a = Player::new(WorldPos::new(0.0, 80.0, 0.0));
        let mut b = Player::new(WorldPos::new(4.0, 80.0, 0.0));
        let mut a_gui = petramond_world::gui_state::empty_gui_state();
        let mut b_gui = petramond_world::gui_state::empty_gui_state();
        {
            let mut roster = RosterRefs::new(vec![
                SessionPlayerRef {
                    id: PlayerId(3),
                    player: &mut a,
                    gui_state: &mut a_gui,
                    gui: None,
                },
                SessionPlayerRef {
                    id: PlayerId(7),
                    player: &mut b,
                    gui_state: &mut b_gui,
                    gui: None,
                },
            ]);
            assert_eq!(roster.len(), 2);
            assert_eq!(roster.index_of(PlayerId(7)), Some(1));
            assert_eq!(roster.index_of(PlayerId(0)), None);
            roster.player_at(1).set_health(4);
        }
        assert_eq!(b.health(), 4, "the write landed on the lent player");
        assert!(RosterRefs::empty().is_empty());
    }
}
