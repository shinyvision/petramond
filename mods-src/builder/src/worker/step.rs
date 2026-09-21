//! The golem's work a step at a time: what it is working on, the step it
//! is in the middle of, and what a walk leads to.

use super::Pillar;
use crate::design::Design;

/// What the golem is doing a step at a time.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Task {
    /// Build or clear a design unit, whichever the world asks.
    Unit(usize),
    /// Take down scaffolding nothing open rests on any more.
    Scaffold([i32; 3]),
    /// Put a scaffold under a unit that has nothing to be placed against.
    Support { unit: usize, cell: [i32; 3] },
    /// Take a built unit back down: work it seals in cannot be seen otherwise.
    Reopen(usize),
    /// Cut away natural overgrowth outside the design (a tree's canopy) that
    /// crowds or hides work no stance reaches.
    Trim([i32; 3]),
    /// Dig through a block walling the golem in: its way out.
    Breakout([i32; 3]),
}

impl Task {
    /// Whether the task goes first whatever its layer: overgrowth in the way
    /// is why work waits, and so is scaffolding marked to come down.
    pub(super) fn urgent(&self, urgent: &[[i32; 3]]) -> bool {
        matches!(self, Task::Trim(_)) || matches!(self, Task::Scaffold(c) if urgent.contains(c))
    }

    /// Whether the task lays a block that goes in late (leaves).
    pub(super) fn late(&self, design: &Design) -> bool {
        matches!(self, Task::Unit(i) if design.late(design.units[*i]))
    }

    /// The design unit the task works toward, if it has one.
    pub(super) fn unit(&self) -> usize {
        match *self {
            Task::Unit(i) | Task::Support { unit: i, .. } => i,
            Task::Scaffold(_) | Task::Reopen(_) | Task::Trim(_) | Task::Breakout(_) => usize::MAX,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Then {
    Task(Task),
    Fetch([i32; 3]),
    Deposit([i32; 3]),
    Climb(Pillar),
    Home,
    /// Walked back toward home to plan again from reachable ground.
    Regroup,
    /// Walked along a pillar's course to work on the task from there.
    Course(Task),
    /// At a door it opened, to shut it on the way home.
    Shut([i32; 3]),
    /// A step of the way out from somewhere no route leads home from.
    Escape,
    /// At a shut door standing between the golem and its work, to open it.
    Open([i32; 3]),
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Step {
    #[default]
    Plan,
    /// Walking the leg to `to` of the way to `goal`, then doing `then`.
    Walk {
        to: [i32; 3],
        goal: [i32; 3],
        then: Then,
        best: i32,
        progress: u64,
    },
    Centre {
        task: Task,
        since: u64,
    },
    /// Turned to a placement, the block in hand, a moment before it goes in.
    Aim {
        task: Task,
        until: u64,
    },
    /// At a chest with its lid held open: fetching or putting back once it is
    /// up, then letting go.
    Rummage {
        container: [i32; 3],
        deposit: bool,
        since: u64,
        moved: bool,
    },
    Dig {
        task: Task,
        cell: [i32; 3],
        since: u64,
    },
    /// Turning to a door in reach and swinging it: `open` to work beyond it,
    /// or shut again on the way home.
    Use {
        door: [i32; 3],
        open: bool,
        since: u64,
    },
    /// A queued place or break waiting for its outcome.
    Await {
        task: Task,
        since: u64,
    },
    Climb {
        pillar: Pillar,
        level: Option<i32>,
        placed: bool,
        since: u64,
    },
    Descend {
        since: u64,
    },
    /// Laying a scaffold walkway out from a pillar top (`Crew::bridge`).
    Bridge {
        since: u64,
        /// When the support now being laid was asked for (0: none yet).
        asked: u64,
    },
    /// Walking the walkway back, taking it down.
    Unbridge {
        since: u64,
    },
    /// Hopping down off a ledge onto ground below it survives the fall to.
    Hop {
        to: [i32; 3],
        since: u64,
    },
    /// Up a level on a scaffold laid into the cell left, getting out.
    Rise {
        from: [i32; 3],
        since: u64,
    },
    /// Sinking into the ground where it is stuck and rising again at home.
    Relocate {
        t: u32,
        /// Sinking, then the three legs under the ground, then rising.
        leg: u8,
    },
    Emerge {
        t: u32,
    },
    Burrow {
        t: u32,
    },
}
