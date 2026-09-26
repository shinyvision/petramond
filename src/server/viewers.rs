//! Who holds each container open — players' chest screens and mobs'
//! `ContainerHold`s — and therefore what every client's chest lids show.
//!
//! One funnel owns the counts: a viewer added or dropped here reports its
//! 0↔1 transition onto the tick's world events (`chest_changed`) in the same
//! call, so no caller can move a count without the lid following it.

use std::collections::HashMap;

use petramond_math::math::IVec3;

use crate::events::tick::TickEvents;
use crate::mob::Mobs;
use crate::world::World;

/// Viewer counts per chest and the mob holds counted among them.
#[derive(Default)]
pub struct ContainerViewers {
    /// How many viewers each chest has; entries are removed at zero.
    chests: HashMap<IVec3, u8>,
    /// The live mobs holding each container open, each counted once among
    /// its chest's viewers.
    holds: HashMap<IVec3, Vec<u64>>,
}

impl ContainerViewers {
    /// One more viewer of the chest at `pos`; the first lifts its lid.
    pub fn add_viewer(&mut self, pos: IVec3, events: &mut TickEvents) {
        let count = self.chests.entry(pos).or_insert(0);
        *count = count.saturating_add(1);
        if *count == 1 {
            events.world.chest_changed.push((pos, true));
        }
    }

    /// One viewer fewer of the chest at `pos`; the last lets its lid fall.
    pub fn drop_viewer(&mut self, pos: IVec3, events: &mut TickEvents) {
        if let Some(count) = self.chests.get_mut(&pos) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                self.chests.remove(&pos);
                events.world.chest_changed.push((pos, false));
            }
        }
    }

    /// How many viewers the chest at `pos` has.
    pub fn viewers(&self, pos: IVec3) -> u8 {
        self.chests.get(&pos).copied().unwrap_or(0)
    }

    /// Every chest with at least one viewer, sorted — the replicated
    /// open-chest set (deterministic across runs).
    pub fn open_chests(&self) -> Vec<IVec3> {
        let mut open: Vec<IVec3> = self.chests.keys().copied().collect();
        open.sort_unstable_by_key(|p| (p.x, p.y, p.z));
        open
    }

    /// A live mob holding the container at `pos` open, or letting it go
    /// (`ContainerHold`). Only a chest shows it: the mob counts once among
    /// its viewers however often it asks, as a player's open screen does.
    pub fn set_mob_hold(
        &mut self,
        world: &World,
        mob_id: u64,
        pos: IVec3,
        open: bool,
        events: &mut TickEvents,
    ) {
        let held = self
            .holds
            .get(&pos)
            .is_some_and(|ids| ids.contains(&mob_id));
        if open && !held && world.mobs().index_of_id(mob_id).is_some() {
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

    /// Let go every container held by a mob no longer in the world.
    pub fn release_absent_holders(&mut self, mobs: &Mobs, events: &mut TickEvents) {
        if self.holds.is_empty() {
            return;
        }
        let gone: Vec<(IVec3, u64)> = self
            .holds
            .iter()
            .flat_map(|(pos, ids)| ids.iter().map(move |id| (*pos, *id)))
            .filter(|(_, id)| mobs.index_of_id(*id).is_none())
            .collect();
        for (pos, id) in gone {
            self.release(pos, id);
            self.drop_viewer(pos, events);
        }
    }

    /// Forget mob `mob_id`'s hold on the container at `pos`.
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
