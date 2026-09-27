use rustc_hash::FxHashMap;
use serde::Deserialize;

use crate::world::ServerWorld;
use petramond_math::math::IVec3;

use super::super::path::{NavSearch, PathParams, SearchPoll, SearchProbes};
use super::super::spatial::MobSnapshot;
use super::super::{def, EntityRef, PlayerAnchor};
use super::budget::PathBudget;
use super::{
    hazards, nav_fluid_fn, nav_solid_fn, nav_support_fn, navigation_step_gate, Navigator,
    MAX_REPATH_BACKOFF_TICKS, REPATH_TICKS,
};

pub(super) const GOAL_DRIFT_REPATH_TICKS: u32 = 4;

#[derive(Copy, Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct NavTuning {
    pub max_nodes: usize,
    pub repath_ticks: u32,
}

impl Default for NavTuning {
    fn default() -> Self {
        NavTuning {
            max_nodes: PathParams::default().max_nodes,
            repath_ticks: REPATH_TICKS,
        }
    }
}

impl NavTuning {
    pub fn validate(self) -> Result<(), String> {
        if !(64..=20_000).contains(&self.max_nodes) {
            return Err(format!(
                "nav.max_nodes must be in [64, 20000], got {}",
                self.max_nodes
            ));
        }
        if !(1..=MAX_REPATH_BACKOFF_TICKS).contains(&self.repath_ticks) {
            return Err(format!(
                "nav.repath_ticks must be in [1, {MAX_REPATH_BACKOFF_TICKS}], got {}",
                self.repath_ticks
            ));
        }
        Ok(())
    }
}

pub(super) struct PendingSearch {
    search: Box<NavSearch>,
    goal_changed: bool,
}

pub(in crate::mob) struct NavInputs<'a> {
    pub self_id: u64,
    pub target: Option<EntityRef>,
    pub mobs: &'a MobSnapshot,
    pub players: &'a [PlayerAnchor],
    pub budget: Option<&'a PathBudget>,
}

impl NavInputs<'static> {
    #[cfg(test)]
    pub fn none() -> Self {
        NavInputs {
            self_id: 0,
            target: None,
            mobs: MobSnapshot::empty(),
            players: &[],
            budget: None,
        }
    }
}

impl Navigator {
    pub fn with_tuning(mut self, tuning: NavTuning) -> Self {
        self.params.max_nodes = tuning.max_nodes;
        self.repath_ticks = tuning.repath_ticks;
        self.repath_interval = tuning.repath_ticks;
        self
    }

    pub fn search_waiting(&self) -> bool {
        self.pending.is_some()
    }

    pub fn update_goal_when_supported(
        &mut self,
        goal: Option<IVec3>,
        start: IVec3,
        world: &ServerWorld,
        can_repath: bool,
        inputs: &NavInputs,
    ) {
        let Some(g) = goal else {
            if self.goal.is_some() {
                self.clear();
            }
            return;
        };
        if !can_repath {
            return;
        }
        if self.goal == Some(g) {
            if self.pending.is_some() {
                self.continue_search(start, world, inputs);
                return;
            }
            self.since_path = self.since_path.saturating_add(1);
            if self.since_path >= self.repath_interval {
                self.plan(start, g, world, inputs, false);
            }
            return;
        }
        if self.retarget_along_route(g) {
            return;
        }
        if self.drift_holds(g) {
            self.since_path = self.since_path.saturating_add(1);
            return;
        }
        self.drop_pending(inputs.budget);
        self.repath_interval = self.repath_ticks;
        self.goal = Some(g);
        self.stuck = 0;
        self.goal_best = f32::INFINITY;
        self.goal_stall = 0;
        self.plan(start, g, world, inputs, true);
    }

    fn retarget_along_route(&mut self, g: IVec3) -> bool {
        if self.pending.is_some() {
            return false;
        }
        let Some(at) = self.path[self.index.min(self.path.len())..]
            .iter()
            .position(|&c| c == g)
        else {
            return false;
        };
        self.path.truncate(self.index + at + 1);
        self.goal = Some(g);
        self.path_reaches_goal = true;
        self.repath_interval = self.repath_ticks;
        self.stuck = 0;
        self.goal_best = f32::INFINITY;
        self.goal_stall = 0;
        true
    }

    fn drift_holds(&self, g: IVec3) -> bool {
        let Some(held) = self.goal else {
            return false;
        };
        let d = g - held;
        self.pending.is_none()
            && self.path_reaches_goal
            && self.index < self.path.len()
            && d.x.abs() <= 1
            && d.y.abs() <= 1
            && d.z.abs() <= 1
            && self.since_path < GOAL_DRIFT_REPATH_TICKS
    }

    fn drop_pending(&mut self, budget: Option<&PathBudget>) {
        if let (Some(pending), Some(budget)) = (self.pending.take(), budget) {
            budget.recycle(*pending.search);
        }
    }

    fn plan(
        &mut self,
        start: IVec3,
        goal: IVec3,
        world: &ServerWorld,
        inputs: &NavInputs,
        goal_changed: bool,
    ) {
        let budget = inputs.budget;
        let mut grant = budget.map_or(usize::MAX, |b| b.grant(false));
        let old_waypoint = (self.index < self.path.len()).then(|| self.path[self.index]);
        let mut search = budget.map_or_else(Box::<NavSearch>::default, PathBudget::take_search);
        let (preserved, spent) = match old_waypoint {
            Some(wp) if grant >= 2 * self.params.max_nodes => {
                self.with_probes(world, inputs, start, |params, probes| {
                    preserve_waypoint_path(&mut search, start, wp, goal, params, probes)
                })
            }
            _ => (None, 0),
        };
        if let Some(b) = budget {
            b.spend(spent, false);
            grant = b.grant(false);
        }
        if let Some(route) = preserved {
            if let Some(b) = budget {
                b.recycle(*search);
            }
            self.install(route, goal_changed);
            return;
        }
        search.begin(start, goal);
        self.run_slice(search, goal_changed, start, world, inputs, grant, false);
    }

    fn continue_search(&mut self, start: IVec3, world: &ServerWorld, inputs: &NavInputs) {
        let Some(PendingSearch {
            search,
            goal_changed,
        }) = self.pending.take()
        else {
            return;
        };
        let grant = inputs.budget.map_or(usize::MAX, |b| b.grant(true));
        self.run_slice(search, goal_changed, start, world, inputs, grant, true);
    }

    #[allow(clippy::too_many_arguments)]
    fn run_slice(
        &mut self,
        mut search: Box<NavSearch>,
        goal_changed: bool,
        start: IVec3,
        world: &ServerWorld,
        inputs: &NavInputs,
        grant: usize,
        continuation: bool,
    ) {
        let (poll, spent) = self.with_probes(world, inputs, search.start(), |params, probes| {
            search.run(params, probes, grant)
        });
        if let Some(b) = inputs.budget {
            b.spend(spent, continuation);
        }
        match poll {
            SearchPoll::Pending => {
                self.pending = Some(PendingSearch {
                    search,
                    goal_changed,
                });
                if goal_changed {
                    self.path_reaches_goal = false;
                }
            }
            SearchPoll::Done(mut route) => {
                if let Some(b) = inputs.budget {
                    b.recycle(*search);
                }
                if let Some(at) = route.iter().position(|&c| c == start) {
                    route.drain(..at);
                }
                self.install(route, goal_changed);
            }
        }
    }

    fn with_probes<R>(
        &self,
        world: &ServerWorld,
        inputs: &NavInputs,
        origin: IVec3,
        f: impl FnOnce(PathParams, &Probes<'_>) -> R,
    ) -> R {
        let cursor = world.cursor();
        let solid = nav_solid_fn(&cursor);
        let support = nav_support_fn(&cursor, self.half_width);
        let fluid = nav_fluid_fn(&cursor);
        let step_allowed = navigation_step_gate(&cursor, self.params, self.height);
        let escape_cost = hazards::escape_cost(&cursor, self.params, origin);
        let mut owned = FxHashMap::default();
        let mut shared = inputs.budget.map(PathBudget::costs);
        let costs = shared.as_deref_mut().unwrap_or(&mut owned);
        entity_cell_costs(inputs, origin, costs);
        let costs = &*costs;
        let cell_cost = |c: IVec3| costs.get(&c).copied().unwrap_or(0) + escape_cost(c);
        f(
            self.params,
            &SearchProbes {
                solid: &solid,
                support: &support,
                fluid: &fluid,
                step_allowed: &step_allowed,
                cell_cost: &cell_cost,
            },
        )
    }

    fn install(&mut self, route: Vec<IVec3>, goal_changed: bool) {
        self.path = route;
        let goal = self.goal;
        self.path_reaches_goal = self.path.last().is_some_and(|&last| Some(last) == goal);
        self.index = if self.path.len() > 1 {
            1
        } else {
            self.path.len()
        };
        self.since_path = 0;
        let refused_again = self.refusals.replan();
        if (self.path_reaches_goal || goal_changed) && !refused_again {
            self.repath_interval = self.repath_ticks;
        } else {
            self.repath_interval = next_repath_backoff(self.repath_interval, self.repath_ticks);
        }
        #[cfg(test)]
        {
            self.recomputes += 1;
        }
    }
}

type Probes<'p> = SearchProbes<
    'p,
    dyn Fn(IVec3) -> bool + 'p,
    dyn Fn(IVec3) -> bool + 'p,
    dyn Fn(IVec3) -> bool + 'p,
    dyn Fn(IVec3, IVec3) -> bool + 'p,
    dyn Fn(IVec3) -> u32 + 'p,
>;

fn preserve_waypoint_path(
    search: &mut NavSearch,
    start: IVec3,
    waypoint: IVec3,
    goal: IVec3,
    params: PathParams,
    probes: &Probes<'_>,
) -> (Option<Vec<IVec3>>, usize) {
    if waypoint == start {
        return (None, 0);
    }
    let (step, mut spent) = search.solve(start, waypoint, params, probes);
    if step.last() != Some(&waypoint) || step.len() > 2 {
        return (None, spent);
    }
    if waypoint == goal {
        return (Some(step), spent);
    }
    let (suffix, suffix_spent) = search.solve(waypoint, goal, params, probes);
    spent += suffix_spent;
    if suffix.first() != Some(&waypoint) || suffix.len() <= 1 {
        return (None, spent);
    }
    // Preserving is only worth it while the waypoint stays ON THE WAY. A goal
    // that moved to the mob's own cell or behind it makes the suffix double
    // back through `start` (start → wp → start → …) or hairpin off the
    // preserved step; steering then projects the body as already PAST the
    // waypoint along that return leg and walks it back to where it stands —
    // one tick out, one tick back, every tick, for as long as the goal keeps
    // flipping (the herd-at-the-lure shaking). A route that revisits the
    // start, or turns more than 90° at the preserved step, is never a
    // one-cell detour: fall through to the direct search instead.
    if suffix.contains(&start) {
        return (None, spent);
    }
    let (ax, az) = (waypoint.x - start.x, waypoint.z - start.z);
    let (bx, bz) = (suffix[1].x - waypoint.x, suffix[1].z - waypoint.z);
    if ax * bx + az * bz < 0 {
        return (None, spent);
    }
    let mut stitched = step;
    stitched.extend_from_slice(&suffix[1..]);
    (Some(stitched), spent)
}

fn next_repath_backoff(current: u32, base: u32) -> u32 {
    current
        .max(base)
        .saturating_mul(2)
        .min(MAX_REPATH_BACKOFF_TICKS)
}

const ENTITY_CELL_COST: u32 = 200;
const ENTITY_AVOID_RANGE: f32 = 32.0;

fn entity_cell_costs(avoid: &NavInputs, start: IVec3, costs: &mut FxHashMap<IVec3, u32>) {
    costs.clear();
    let (ox, oz) = (f64::from(start.x) + 0.5, f64::from(start.z) + 0.5);
    let mut mark = |min: [f64; 3], max: [f64; 3]| {
        let ddx = ((min[0] + max[0]) * 0.5 - ox) as f32;
        let ddz = ((min[2] + max[2]) * 0.5 - oz) as f32;
        if ddx * ddx + ddz * ddz > ENTITY_AVOID_RANGE * ENTITY_AVOID_RANGE {
            return;
        }
        for x in (min[0].floor() as i32)..=(max[0].floor() as i32) {
            for y in (min[1].floor() as i32)..=(max[1].floor() as i32) {
                for z in (min[2].floor() as i32)..=(max[2].floor() as i32) {
                    let slot = costs.entry(IVec3::new(x, y, z)).or_insert(0);
                    *slot = slot.saturating_add(ENTITY_CELL_COST);
                }
            }
        }
    };
    let origin = petramond_math::world_pos::WorldPos::new(ox, f64::from(start.y), oz);
    for (_, m) in avoid.mobs.near(origin, ENTITY_AVOID_RANGE) {
        if m.id == avoid.self_id || avoid.target == Some(EntityRef::Mob(m.id)) {
            continue;
        }
        let s = def(m.kind).size;
        let half = f64::from(s.half_length.unwrap_or(s.half_width).max(s.half_width));
        mark(
            [m.pos.x - half, m.pos.y, m.pos.z - half],
            [
                m.pos.x + half,
                m.pos.y + f64::from(s.height),
                m.pos.z + half,
            ],
        );
    }
    for p in avoid.players {
        if avoid.target == Some(EntityRef::Player(p.id)) {
            continue;
        }
        let Some(body) = p.body else {
            continue;
        };
        let (mn, mx) = body.aabb();
        mark(mn, mx);
    }
}

#[cfg(test)]
mod tests;
