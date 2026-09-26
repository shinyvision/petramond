//! Named tick stages + the attachment seam between them.
//!
//! The engine's fixed-tick steps (`Game::game_tick_step`) stay hardwired calls in
//! their exact order; [`Stage`] just names them so systems can attach
//! `Before(stage)` / `After(stage)`. The scheduler owns only the seams — it never
//! reorders or replaces engine steps.

use crate::events::tick::TickEvents;
use crate::world::ServerWorld;

use super::bus::{PostQueue, SimCtx};
use super::roster::PlayerRoster;

/// The engine steps of one fixed game tick, in execution order. `WorldScheduled`
/// is `World::game_tick`, whose internal order (scheduled → block updates →
/// furnaces → random ticks) is its own contract and stays sealed.
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

/// Where a system attaches relative to an engine stage. At a boundary between
/// stage N and N+1, `After(N)` systems run before `Before(N+1)` systems.
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

/// The systems attached between the engine's fixed-tick stages. Per slot they run
/// in `(priority ascending, registration order)` — kept by sorted insertion, same
/// determinism contract as the event bus.
#[derive(Default)]
pub struct TickSystems {
    slots: [Vec<SystemEntry>; Stage::COUNT * 2],
}

impl TickSystems {
    /// Attach a system at `at`; runs every fixed tick.
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

    /// Whether nothing is attached at `at` (the per-stage fast path).
    #[inline]
    pub fn is_empty_at(&self, at: Attach) -> bool {
        self.slots[at.slot()].is_empty()
    }

    /// Run the systems attached at `at`, in order. A stage seam belongs to no
    /// player: every system runs ACTOR-LESS and reaches the connected
    /// sessions by id through `players`.
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
        // Two entries share priority 5: registration order must hold between them.
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
