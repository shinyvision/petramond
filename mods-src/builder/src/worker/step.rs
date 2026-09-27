use super::Pillar;
use crate::design::Design;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Task {
    Unit(usize),
    Scaffold([i32; 3]),
    Support { unit: usize, cell: [i32; 3] },
    Reopen(usize),
    Trim([i32; 3]),
    Breakout([i32; 3]),
}

impl Task {
    pub(super) fn urgent(&self, urgent: &[[i32; 3]]) -> bool {
        matches!(self, Task::Trim(_)) || matches!(self, Task::Scaffold(c) if urgent.contains(c))
    }

    pub(super) fn late(&self, design: &Design) -> bool {
        matches!(self, Task::Unit(i) if design.late(design.units[*i]))
    }

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
    Regroup,
    Course(Task),
    Shut([i32; 3]),
    Escape,
    Open([i32; 3]),
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Step {
    #[default]
    Plan,
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
    Aim {
        task: Task,
        until: u64,
    },
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
    Use {
        door: [i32; 3],
        open: bool,
        since: u64,
    },
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
    Bridge {
        since: u64,
        asked: u64,
    },
    Unbridge {
        since: u64,
    },
    Hop {
        to: [i32; 3],
        since: u64,
    },
    Rise {
        from: [i32; 3],
        since: u64,
    },
    Relocate {
        t: u32,
        leg: u8,
    },
    Emerge {
        t: u32,
    },
    Burrow {
        t: u32,
    },
}
