use crate::events::tick::TickEvents;
use crate::world::ServerWorld;

use super::bus::{PostQueue, SimCtx};
use super::roster::PlayerRoster;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Stage {
    Mining,
    Placement,
    Attack,
    Drops,
    Menu,
    PlayerDamage,
    WorldScheduled,
    NaturalBreaks,
    Pickup,
    Mobs,
    ItemPhysics,
    Spawning,
}

impl Stage {
    pub const COUNT: usize = 12;
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Attach {
    Before(Stage),
    After(Stage),
}

impl Attach {
    #[inline]
    fn slot(self) -> usize {
        match self {
            Attach::Before(s) => s as usize * 2,
            Attach::After(s) => s as usize * 2 + 1,
        }
    }
}

type SystemFn = Box<dyn FnMut(&mut SimCtx) + Send>;

struct SystemEntry {
    priority: i32,
    f: SystemFn,
}

#[derive(Default)]
pub struct TickSystems {
    slots: [Vec<SystemEntry>; Stage::COUNT * 2],
}

impl TickSystems {
    pub fn attach(
        &mut self,
        at: Attach,
        priority: i32,
        f: impl FnMut(&mut SimCtx) + Send + 'static,
    ) {
        let list = &mut self.slots[at.slot()];
        let i = list.partition_point(|s| s.priority <= priority);
        list.insert(
            i,
            SystemEntry {
                priority,
                f: Box::new(f),
            },
        );
    }

    #[inline]
    pub fn is_empty_at(&self, at: Attach) -> bool {
        self.slots[at.slot()].is_empty()
    }

    pub fn run(
        &mut self,
        at: Attach,
        world: &mut ServerWorld,
        players: &mut dyn PlayerRoster,
        feed: &mut TickEvents,
        queue: &mut PostQueue,
    ) {
        for s in self.slots[at.slot()].iter_mut() {
            let mut ctx = SimCtx {
                world: &mut *world,
                actor: None,
                players: &mut *players,
                feed: &mut *feed,
                queue: &mut *queue,
            };
            (s.f)(&mut ctx);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;

    #[test]
    fn systems_in_one_slot_run_in_priority_then_registration_order() {
        let mut systems = TickSystems::default();
        let order = Arc::new(Mutex::new(Vec::new()));
        for (label, priority) in [("a", 5), ("b", -1), ("c", 5), ("d", 0)] {
            let order = order.clone();
            systems.attach(Attach::Before(Stage::Mining), priority, move |_| {
                order.lock().unwrap().push(label);
            });
        }
        assert!(!systems.is_empty_at(Attach::Before(Stage::Mining)));
        assert!(systems.is_empty_at(Attach::After(Stage::Mining)));

        let mut world = ServerWorld::new(1, 1);
        let mut feed = TickEvents::default();
        let mut queue = PostQueue::default();
        systems.run(
            Attach::Before(Stage::Mining),
            &mut world,
            &mut super::super::roster::RosterRefs::empty(),
            &mut feed,
            &mut queue,
        );
        assert_eq!(*order.lock().unwrap(), vec!["b", "d", "a", "c"]);
    }
}
