use std::cell::{Cell, RefCell};

use rustc_hash::FxHashMap;

use petramond_math::math::IVec3;

use super::super::path::NavSearch;

pub const REACH_PROBE_NODES: usize = 600;

pub const REACH_PROBE_TICK_BUDGET: usize = 1200;

const PROBE_SETUP_CHARGE: usize = 40;

pub struct ReachBudget {
    remaining: std::cell::Cell<usize>,
    capacity: usize,
}

impl Default for ReachBudget {
    fn default() -> Self {
        Self::with_capacity(REACH_PROBE_TICK_BUDGET)
    }
}

impl ReachBudget {
    pub fn with_capacity(capacity: usize) -> Self {
        ReachBudget {
            remaining: std::cell::Cell::new(capacity),
            capacity,
        }
    }

    pub fn refill(&self) {
        self.remaining.set(self.capacity);
    }

    pub fn remaining(&self) -> usize {
        self.remaining.get()
    }

    pub(super) fn spend(&self, expansions: usize) {
        self.remaining.set(
            self.remaining
                .get()
                .saturating_sub(expansions + PROBE_SETUP_CHARGE),
        );
    }

    pub(super) fn covers(&self, nodes: usize) -> bool {
        self.remaining.get() >= nodes
    }
}

/// Expansions every navigator route search in ONE game tick may spend between
/// them ([`PathBudget`]) — four worst-case searches at the default node cap.
///
/// Before the budget, a tick in which many chasers changed goal ran every
/// one of their full searches back to back; the cost of the worst tick grew
/// with the hostile cap. Ordinary traffic (routes of a few hundred
/// expansions) fits untouched; past the budget, searches continue on the
/// next tick from where they stopped while their mobs keep walking their old
/// routes.
pub const PATH_TICK_BUDGET: usize = 16_000;

const SLICE_SETUP_CHARGE: usize = 40;

const SEARCH_POOL: usize = 8;

/// Route-search budget for the tick (see [`PATH_TICK_BUDGET`]), plus reused search buffers.
/// Interior mutability so each mob's tick can carry it by shared ref alongside the world.
///
/// Searches suspended on an earlier tick get served first. While any are waiting, fresh requests
/// only get half the budget and a waiting search can spend from both halves, so however many mobs
/// ask, a suspended search finishes within a bounded number of ticks.
pub struct PathBudget {
    general: Cell<usize>,
    waiting: Cell<usize>,
    capacity: usize,
    searches: RefCell<Vec<NavSearch>>,
    costs: RefCell<FxHashMap<IVec3, u32>>,
}

impl Default for PathBudget {
    fn default() -> Self {
        Self::with_capacity(PATH_TICK_BUDGET)
    }
}

impl PathBudget {
    pub fn with_capacity(capacity: usize) -> Self {
        PathBudget {
            general: Cell::new(capacity),
            waiting: Cell::new(0),
            capacity,
            searches: RefCell::new(Vec::new()),
            costs: RefCell::new(FxHashMap::default()),
        }
    }

    pub fn refill(&self, waiters: usize) {
        let reserved = if waiters > 0 { self.capacity / 2 } else { 0 };
        self.waiting.set(reserved);
        self.general.set(self.capacity - reserved);
    }

    pub fn grant(&self, continuation: bool) -> usize {
        let general = self.general.get();
        if continuation {
            general + self.waiting.get()
        } else {
            general
        }
    }

    pub fn spend(&self, expansions: usize, continuation: bool) {
        let mut due = expansions + SLICE_SETUP_CHARGE;
        if continuation {
            let reserved = self.waiting.get();
            let taken = reserved.min(due);
            self.waiting.set(reserved - taken);
            due -= taken;
        }
        self.general.set(self.general.get().saturating_sub(due));
    }

    pub fn take_search(&self) -> Box<NavSearch> {
        Box::new(self.searches.borrow_mut().pop().unwrap_or_default())
    }

    pub fn recycle(&self, search: NavSearch) {
        let mut pool = self.searches.borrow_mut();
        if pool.len() < SEARCH_POOL {
            pool.push(search);
        }
    }

    pub(super) fn costs(&self) -> std::cell::RefMut<'_, FxHashMap<IVec3, u32>> {
        self.costs.borrow_mut()
    }
}
