//! The per-tick expansion budgets reachability probes spend from.

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
