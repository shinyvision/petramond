use std::collections::HashMap;

use petramond_math::math::IVec3;

use crate::events::tick::TickEvents;
use crate::mob::Mobs;
use crate::world::ServerWorld;

#[derive(Default)]
pub struct ContainerViewers {
    chests: HashMap<IVec3, u8>,
    holds: HashMap<IVec3, Vec<u64>>,
}

impl ContainerViewers {
    pub fn add_viewer(&mut self, pos: IVec3, events: &mut TickEvents) {
        let count = self.chests.entry(pos).or_insert(0);
        *count = count.saturating_add(1);
        if *count == 1 {
            events.world.chest_changed.push((pos, true));
        }
    }

    pub fn drop_viewer(&mut self, pos: IVec3, events: &mut TickEvents) {
        if let Some(count) = self.chests.get_mut(&pos) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                self.chests.remove(&pos);
                events.world.chest_changed.push((pos, false));
            }
        }
    }

    pub fn viewers(&self, pos: IVec3) -> u8 {
        self.chests.get(&pos).copied().unwrap_or(0)
    }

    pub fn open_chests(&self) -> Vec<IVec3> {
        let mut open: Vec<IVec3> = self.chests.keys().copied().collect();
        open.sort_unstable_by_key(|p| (p.x, p.y, p.z));
        open
    }

    pub fn set_mob_hold(
        &mut self,
        world: &ServerWorld,
        mob_id: u64,
        pos: IVec3,
        open: bool,
        events: &mut TickEvents,
    ) {
        let held = self
            .holds
            .get(&pos)
            .is_some_and(|ids| ids.contains(&mob_id));
        if open && !held && world.mobs().contains(mob_id) {
            self.holds.entry(pos).or_default().push(mob_id);
            let chest = world
                .block_if_stream_final(pos.x, pos.y, pos.z)
                .is_some_and(|b| {
                    b.interaction()
                        == petramond_world::block::BlockInteraction::OpenGui(
                            petramond_world::gui_state::GuiKind::Chest,
                        )
                });
            if chest {
                self.add_viewer(pos, events);
            }
        } else if !open && held {
            self.release(pos, mob_id);
            self.drop_viewer(pos, events);
        }
    }

    pub fn release_absent_holders(&mut self, mobs: &Mobs, events: &mut TickEvents) {
        if self.holds.is_empty() {
            return;
        }
        let gone: Vec<(IVec3, u64)> = self
            .holds
            .iter()
            .flat_map(|(pos, ids)| ids.iter().map(move |id| (*pos, *id)))
            .filter(|(_, id)| !mobs.contains(*id))
            .collect();
        for (pos, id) in gone {
            self.release(pos, id);
            self.drop_viewer(pos, events);
        }
    }

    fn release(&mut self, pos: IVec3, mob_id: u64) {
        if let Some(ids) = self.holds.get_mut(&pos) {
            ids.retain(|id| *id != mob_id);
            if ids.is_empty() {
                self.holds.remove(&pos);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_first_and_last_viewer_move_the_lid() {
        let mut viewers = ContainerViewers::default();
        let mut events = TickEvents::default();
        let pos = IVec3::new(1, 2, 3);
        viewers.add_viewer(pos, &mut events);
        viewers.add_viewer(pos, &mut events);
        assert_eq!(viewers.viewers(pos), 2);
        viewers.drop_viewer(pos, &mut events);
        assert_eq!(viewers.open_chests(), vec![pos]);
        viewers.drop_viewer(pos, &mut events);
        viewers.drop_viewer(pos, &mut events);
        assert_eq!(viewers.viewers(pos), 0);
        assert_eq!(events.world.chest_changed, vec![(pos, true), (pos, false)]);
    }
}
