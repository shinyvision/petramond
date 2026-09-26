//! The resumable form of the production navigation search.
//!
//! [`NavSearch`] is [`super::find_path_nav`]'s A* with its whole frontier kept
//! in the struct, so a search can stop after any number of expansions and pick
//! up later exactly where it left off — the navigator's per-tick path budget
//! (see `mob::nav::PathBudget`) runs searches in slices across ticks instead
//! of letting one tick absorb every mob's worst case. A search run to
//! completion in one slice expands the same nodes in the same order as an
//! uninterrupted one, so slicing never changes a route, only when it arrives.
//!
//! The buffers (open heap, scores, parents) are reused across searches: a
//! finished search is [`begin`](NavSearch::begin)-reset rather than
//! reallocated, which is what lets the navigator pool them.

use rustc_hash::FxHashMap;
use std::cmp::Reverse;
use std::collections::BinaryHeap;

use petramond_math::math::IVec3;

use super::{
    body_clear, is_navigation_foothold_with, neighbors, reconstruct, CellMemo, PathParams,
    COST_DIAG, COST_FLAT,
};

/// The world probes one search slice reads (see [`super::find_path_nav`] for
/// what each one means). Rebuilt by the caller for every slice: a search
/// resumed on a later tick reads the world as it is then.
pub struct SearchProbes<'p, S: ?Sized, U: ?Sized, F: ?Sized, G: ?Sized, C: ?Sized> {
    pub solid: &'p S,
    pub support: &'p U,
    pub fluid: &'p F,
    pub step_allowed: &'p G,
    pub cell_cost: &'p C,
}

/// What a search slice came to.
#[derive(Debug, PartialEq)]
pub enum SearchPoll {
    /// The search finished: the route toward the goal (possibly partial, or
    /// empty when the start is no foothold), exactly as [`super::find_path_nav`]
    /// returns it.
    Done(Vec<IVec3>),
    /// The slice's expansion grant ran out first; call
    /// [`run`](NavSearch::run) again to continue.
    Pending,
}

/// Where a search is in its lifecycle.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Phase {
    /// `begin` was called; the start has not been checked yet.
    Fresh,
    /// The frontier is live.
    Searching,
}

/// A resumable A* over footholds from `start` toward `goal` (see the module
/// docs).
pub struct NavSearch {
    start: IVec3,
    goal: IVec3,
    phase: Phase,
    g_score: FxHashMap<IVec3, u32>,
    came_from: FxHashMap<IVec3, IVec3>,
    open: BinaryHeap<Reverse<(u32, u32, [i32; 3])>>,
    /// Closest cell to the goal seen so far, by heuristic — the partial-route
    /// fallback.
    best: IVec3,
    best_h: u32,
    /// Expansions across every slice, against `PathParams::max_nodes`.
    expanded: usize,
    steps: Vec<(IVec3, u32)>,
}

impl Default for NavSearch {
    fn default() -> Self {
        NavSearch {
            start: IVec3::ZERO,
            goal: IVec3::ZERO,
            phase: Phase::Fresh,
            g_score: FxHashMap::default(),
            came_from: FxHashMap::default(),
            open: BinaryHeap::new(),
            best: IVec3::ZERO,
            best_h: 0,
            expanded: 0,
            steps: Vec::with_capacity(8),
        }
    }
}

impl NavSearch {
    /// Reset for a new search from `start` to `goal`, keeping the buffers.
    pub fn begin(&mut self, start: IVec3, goal: IVec3) {
        self.start = start;
        self.goal = goal;
        self.phase = Phase::Fresh;
        self.g_score.clear();
        self.came_from.clear();
        self.open.clear();
        self.best = start;
        self.best_h = 0;
        self.expanded = 0;
    }

    /// The cell this search routes from.
    pub fn start(&self) -> IVec3 {
        self.start
    }

    /// Octile distance to the goal: the cheapest diagonal-then-straight route
    /// over flat ground, ignoring height (vertical moves cost ≥ `COST_FLAT`,
    /// so this stays admissible).
    fn h(&self, c: IVec3) -> u32 {
        let dx = (c.x - self.goal.x).unsigned_abs();
        let dz = (c.z - self.goal.z).unsigned_abs();
        let (lo, hi) = if dx < dz { (dx, dz) } else { (dz, dx) };
        COST_DIAG * lo + COST_FLAT * (hi - lo)
    }

    /// Run until the search finishes or `grant` expansions have been spent in
    /// this slice. Returns the outcome and the expansions this slice spent.
    pub fn run<S, U, F, G, C>(
        &mut self,
        params: PathParams,
        probes: &SearchProbes<'_, S, U, F, G, C>,
        grant: usize,
    ) -> (SearchPoll, usize)
    where
        S: Fn(IVec3) -> bool + ?Sized,
        U: Fn(IVec3) -> bool + ?Sized,
        F: Fn(IVec3) -> bool + ?Sized,
        G: Fn(IVec3, IVec3) -> bool + ?Sized,
        C: Fn(IVec3) -> u32 + ?Sized,
    {
        // Sized views of the (possibly unsized) probes, for the generic
        // cell predicates.
        let solid = |c: IVec3| (probes.solid)(c);
        let support = |c: IVec3| (probes.support)(c);
        let fluid = |c: IVec3| (probes.fluid)(c);
        let step_allowed = |a: IVec3, b: IVec3| (probes.step_allowed)(a, b);
        let passable_col = |c: IVec3| body_clear(c, params, &solid);
        // A cell is a foothold if its floor *supports* it (solid ground, a
        // partial shape's top, or the fluid surface) and the body fits above.
        // Submerged fluid cells are passable, not footholds.
        let memo = CellMemo::<2048>::default();
        let foothold = |c: IVec3| {
            memo.get(c, |c| {
                is_navigation_foothold_with(c, params, &solid, &support, &fluid)
            })
        };

        if self.phase == Phase::Fresh {
            if !foothold(self.start) {
                return (SearchPoll::Done(Vec::new()), 0);
            }
            if self.start == self.goal {
                return (SearchPoll::Done(vec![self.start]), 0);
            }
            self.g_score.insert(self.start, 0);
            self.open
                .push(Reverse((self.h(self.start), 0, self.start.to_array())));
            self.best = self.start;
            self.best_h = self.h(self.start);
            self.phase = Phase::Searching;
        }

        let mut spent = 0usize;
        loop {
            if spent >= grant {
                return (SearchPoll::Pending, spent);
            }
            let Some(Reverse((_, g_at_pop, pos_arr))) = self.open.pop() else {
                break;
            };
            let current = IVec3::from_array(pos_arr);
            // Skip stale heap entries (a cheaper path to `current` was found
            // after this entry was queued).
            if g_at_pop > *self.g_score.get(&current).unwrap_or(&u32::MAX) {
                continue;
            }
            if current == self.goal {
                return (
                    SearchPoll::Done(reconstruct(&self.came_from, current)),
                    spent,
                );
            }
            let hc = self.h(current);
            if hc < self.best_h {
                self.best_h = hc;
                self.best = current;
            }

            self.expanded += 1;
            spent += 1;
            if self.expanded >= params.max_nodes {
                break;
            }

            neighbors(
                current,
                &params,
                &foothold,
                &passable_col,
                &solid,
                &step_allowed,
                &mut self.steps,
            );
            let g_current = self.g_score[&current];
            for i in 0..self.steps.len() {
                let (next, step_cost) = self.steps[i];
                let tentative = g_current
                    .saturating_add(step_cost)
                    .saturating_add((probes.cell_cost)(next));
                if tentative < *self.g_score.get(&next).unwrap_or(&u32::MAX) {
                    self.came_from.insert(next, current);
                    self.g_score.insert(next, tentative);
                    self.open.push(Reverse((
                        tentative + self.h(next),
                        tentative,
                        next.to_array(),
                    )));
                }
            }
        }

        // Goal unreachable within the node cap: walk toward the closest cell
        // found.
        (
            SearchPoll::Done(reconstruct(&self.came_from, self.best)),
            spent,
        )
    }

    /// Run a fresh search from `start` to `goal` to completion in one slice.
    /// Returns the route and the expansions it cost.
    pub fn solve<S, U, F, G, C>(
        &mut self,
        start: IVec3,
        goal: IVec3,
        params: PathParams,
        probes: &SearchProbes<'_, S, U, F, G, C>,
    ) -> (Vec<IVec3>, usize)
    where
        S: Fn(IVec3) -> bool + ?Sized,
        U: Fn(IVec3) -> bool + ?Sized,
        F: Fn(IVec3) -> bool + ?Sized,
        G: Fn(IVec3, IVec3) -> bool + ?Sized,
        C: Fn(IVec3) -> u32 + ?Sized,
    {
        self.begin(start, goal);
        match self.run(params, probes, usize::MAX) {
            (SearchPoll::Done(path), spent) => (path, spent),
            (SearchPoll::Pending, _) => unreachable!("an unlimited grant always finishes"),
        }
    }
}

#[cfg(test)]
mod tests;
