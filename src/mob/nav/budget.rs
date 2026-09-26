//! The per-tick expansion budgets navigation searches spend from: the
//! [`ReachBudget`] reachability probes share, and the [`PathBudget`] the
//! navigators' route searches share.

use std::cell::{Cell, RefCell};

use rustc_hash::FxHashMap;

use petramond_math::math::IVec3;

use super::super::path::NavSearch;

/// Node budget for one goal-reachability probe (wander picks, the mod ABI's
/// `MobCanReach`). Destinations sit within local seek radii, so an honest
/// route needs far fewer expansions than the navigator's full budget — and it
/// is the UNREACHABLE probe that pays the entire budget before failing, so a
/// small cap keeps the worst tick cheap. A spot the budget can't prove
/// reachable counts as unreachable.
pub const REACH_PROBE_NODES: usize = 600;

/// Expansions every reachability probe in ONE game tick may spend BETWEEN
/// THEM ([`ReachBudget`]).
///
/// A probe that succeeds costs a few dozen expansions (wander destinations sit
/// inside a local radius); a probe that FAILS pays the entire
/// [`REACH_PROBE_NODES`] budget, and several mobs can fail in the same tick —
/// which is what produced this simulation's worst ticks by a wide margin, an
/// order of magnitude above its median. The budget lets ordinary traffic
/// through untouched (twenty-odd successful probes fit) while capping how much
/// FAILURE one tick can absorb at roughly one full probe.
///
/// A probe refused for budget is DEFERRED, never answered: the asking policy
/// simply does not pick a destination this tick and rolls again on the next
/// one, 50 ms later. Verdicts are never altered — a probe that runs runs with
/// the full [`REACH_PROBE_NODES`] cap and answers exactly what it always did.
pub const REACH_PROBE_TICK_BUDGET: usize = 1200;

/// Fixed per-probe charge against [`REACH_PROBE_TICK_BUDGET`], in the same
/// units as expansions: what a probe costs before it expands anything.
const PROBE_SETUP_CHARGE: usize = 40;

/// The tick-scoped expansion budget shared by every reachability probe (see
/// `REACH_PROBE_TICK_BUDGET`). Interior mutability so the AI context can
/// carry it by shared reference alongside the world; `None` in a context means
/// "unbudgeted" (unit fixtures, the mod ABI's explicit `MobCanReach`).
pub struct ReachBudget {
    remaining: std::cell::Cell<usize>,
    capacity: usize,
}

/// A fresh budget starts FULL, so a world that has never ticked (unit
/// fixtures, a mod probing at init) answers probes instead of deferring them.
impl Default for ReachBudget {
    fn default() -> Self {
        Self::with_capacity(REACH_PROBE_TICK_BUDGET)
    }
}

impl ReachBudget {
    /// A full budget refilling to `capacity` expansions per tick.
    pub fn with_capacity(capacity: usize) -> Self {
        ReachBudget {
            remaining: std::cell::Cell::new(capacity),
            capacity,
        }
    }

    /// Refill for a new tick — the mob manager calls this once per tick.
    pub fn refill(&self) {
        self.remaining.set(self.capacity);
    }

    /// What is left of this tick's expansions.
    pub fn remaining(&self) -> usize {
        self.remaining.get()
    }

    /// Charge a probe that ran: its expansions and its fixed setup.
    pub(super) fn spend(&self, expansions: usize) {
        self.remaining.set(
            self.remaining
                .get()
                .saturating_sub(expansions + PROBE_SETUP_CHARGE),
        );
    }

    /// A probe may only START when the budget can cover its WORST case
    /// (`nodes` expansions), so a tick can absorb at most one full-cost
    /// failure however many probes ask.
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

/// Fixed per-slice charge against [`PATH_TICK_BUDGET`]: what running a slice
/// costs before it expands anything (probe setup, the foothold memo).
const SLICE_SETUP_CHARGE: usize = 40;

/// Searches kept for reuse; more than this many finishing in one tick just
/// lets the extras drop.
const SEARCH_POOL: usize = 8;

/// The tick-scoped route-search budget (see [`PATH_TICK_BUDGET`]) and the
/// buffers searches reuse. Interior mutability so every mob's tick can carry
/// it by shared reference alongside the world.
///
/// Fairness: requests that had to WAIT on an earlier tick (their search is
/// suspended mid-way) are served before new ones. When any are waiting, half
/// of the tick's budget is held back for them — fresh requests may only spend
/// the other half, while a waiting search may spend from both. So however
/// many mobs ask each tick, a suspended search always progresses and
/// finishes within a bounded number of ticks.
pub struct PathBudget {
    /// Expansions left this tick that any request may spend.
    general: Cell<usize>,
    /// Expansions left this tick reserved for suspended searches.
    waiting: Cell<usize>,
    capacity: usize,
    /// Finished searches whose buffers the next request reuses.
    searches: RefCell<Vec<NavSearch>>,
    /// The entity cell-cost map every search rebuilds, kept allocated.
    costs: RefCell<FxHashMap<IVec3, u32>>,
}

impl Default for PathBudget {
    fn default() -> Self {
        Self::with_capacity(PATH_TICK_BUDGET)
    }
}

impl PathBudget {
    /// A full budget refilling to `capacity` expansions per tick.
    pub fn with_capacity(capacity: usize) -> Self {
        PathBudget {
            general: Cell::new(capacity),
            waiting: Cell::new(0),
            capacity,
            searches: RefCell::new(Vec::new()),
            costs: RefCell::new(FxHashMap::default()),
        }
    }

    /// Refill for a new tick in which `waiters` suspended searches will ask
    /// to continue — the mob manager calls this once per tick.
    pub fn refill(&self, waiters: usize) {
        let reserved = if waiters > 0 { self.capacity / 2 } else { 0 };
        self.waiting.set(reserved);
        self.general.set(self.capacity - reserved);
    }

    /// Expansions a request may spend right now: a suspended search's
    /// continuation reaches into the reserved pool, a fresh request does not.
    pub fn grant(&self, continuation: bool) -> usize {
        let general = self.general.get();
        if continuation {
            general + self.waiting.get()
        } else {
            general
        }
    }

    /// Charge a slice that ran `expansions` (plus its fixed setup). A
    /// continuation drains the reserved pool first.
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

    /// A search to run, reusing a pooled one's buffers when available.
    pub fn take_search(&self) -> Box<NavSearch> {
        Box::new(self.searches.borrow_mut().pop().unwrap_or_default())
    }

    /// Return a search that is done with, for the next request to reuse.
    pub fn recycle(&self, search: NavSearch) {
        let mut pool = self.searches.borrow_mut();
        if pool.len() < SEARCH_POOL {
            pool.push(search);
        }
    }

    /// The shared entity cell-cost map, for one search slice at a time.
    pub(super) fn costs(&self) -> std::cell::RefMut<'_, FxHashMap<IVec3, u32>> {
        self.costs.borrow_mut()
    }
}
