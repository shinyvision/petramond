use std::sync::Arc;

use petramond_world::gui_state::GuiStateMap;

use crate::player::{Player, PlayerId};

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct OpenGui {
    pub kind: petramond_world::gui_state::GuiKind,
    pub anchor: Option<crate::menu::MenuAnchor>,
}

pub trait PlayerRoster {
    fn len(&self) -> usize;

    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn id_at(&self, index: usize) -> PlayerId;

    fn player_at(&mut self, index: usize) -> &mut Player;

    fn gui_state_at(&mut self, index: usize) -> &mut Arc<GuiStateMap>;

    fn open_gui_at(&self, index: usize) -> Option<OpenGui>;

    fn index_of(&self, id: PlayerId) -> Option<usize> {
        (0..self.len()).find(|&i| self.id_at(i) == id)
    }
}

pub struct SessionPlayerRef<'a> {
    pub id: PlayerId,
    pub player: &'a mut Player,
    pub gui_state: &'a mut Arc<GuiStateMap>,
    pub gui: Option<OpenGui>,
}

#[derive(Default)]
pub struct RosterRefs<'a> {
    sessions: Vec<SessionPlayerRef<'a>>,
}

impl<'a> RosterRefs<'a> {
    pub fn new(sessions: Vec<SessionPlayerRef<'a>>) -> Self {
        Self { sessions }
    }

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
