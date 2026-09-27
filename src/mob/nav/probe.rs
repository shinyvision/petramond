use mod_api::Route;
use petramond_math::math::IVec3;

use super::budget::{ReachBudget, REACH_PROBE_NODES};
use super::kept::{self, FactLane, KeptBox};
use super::step_gate::{navigation_step_gate, navigation_step_gate_on};
use super::{fluid_footing, hazards, nav_fluid_fn, nav_loaded_fn, nav_solid_fn, nav_support_fn};
use crate::mob::path::{self, CellCache, Fact, NavWorld, PathParams, Planned};
use crate::mob::{def, Instance, Mob};
use crate::world::ServerWorld;

/// Whether a body (`params`, physical `height`) standing at foothold `start`
/// can genuinely path to `dest` within [`REACH_PROBE_NODES`]. Entity
/// soft-costs are deliberately absent: bodies never make a spot unreachable,
/// and the navigator prices them when it routes for real. This is the
/// DESTINATION-honesty gate — the pathfinder deliberately walks best-effort
/// partial routes toward unreachable goals (chases must crowd their target),
/// which parks a mob against the obstacle when the goal was a picked CELL, so
/// every cell-picking policy must ask this first.
///
/// `None` when `budget` is present and spent: the answer is UNKNOWN this tick
/// and the caller must defer, not guess (see [`REACH_PROBE_TICK_BUDGET`]).
pub(in crate::mob) fn destination_reachable(
    world: &ServerWorld,
    start: IVec3,
    dest: IVec3,
    mut params: PathParams,
    height: f32,
    budget: Option<&ReachBudget>,
) -> Option<bool> {
    if let Some(b) = budget {
        if !b.covers(REACH_PROBE_NODES) {
            return None;
        }
    }
    params.max_nodes = REACH_PROBE_NODES;
    let cursor = world.cursor();
    let solid = nav_solid_fn(&cursor);
    let support = nav_support_fn(&cursor, params.half_width);
    let fluid = nav_fluid_fn(&cursor);
    let step_allowed = navigation_step_gate(&cursor, params, height);
    let (out, nodes) =
        path::reachable_nav(start, dest, params, &solid, &support, &fluid, step_allowed);
    if let Some(b) = budget {
        b.spend(nodes);
    }
    Some(out.unwrap_or(false))
}

pub fn mob_can_reach(world: &ServerWorld, mob: &Instance, dest: IVec3) -> bool {
    let d = super::def(mob.kind);
    let params = d.path_params();
    let cursor = world.cursor();
    let solid = nav_solid_fn(&cursor);
    let support = nav_support_fn(&cursor, d.size.half_width);
    let fluid = nav_fluid_fn(&cursor);
    let start = path::navigation_cell_with(
        mob.pos,
        d.size.half_width,
        d.size.head_cells(),
        fluid_footing(&cursor, mob.pos).is_some(),
        &solid,
        &support,
        &fluid,
    )
    .unwrap_or_else(|| mob.pos.block());
    destination_reachable(
        world,
        start,
        dest,
        params,
        d.size.height,
        Some(world.reach_budget()),
    )
    .unwrap_or(false)
}

pub const ROUTE_PROBE_TICK_BUDGET: usize = 32_000;

pub const ROUTE_PROBE_MAX_NODES: usize = 4000;

struct PlannedWorld<'a, So, Su, Fl, St> {
    planned: &'a [IVec3],
    lanes: [Fact<'a>; 3],
    solid: So,
    support: Su,
    fluid: Fl,
    step: St,
}

impl<So, Su, Fl, St> NavWorld for PlannedWorld<'_, So, Su, Fl, St>
where
    So: Fn(IVec3) -> bool,
    Su: Fn(IVec3) -> bool,
    Fl: Fn(IVec3) -> bool,
    St: Fn(IVec3, IVec3) -> bool,
{
    fn solid(&self, c: IVec3) -> bool {
        self.planned.contains(&c) || self.lanes[0].get(c, &self.solid)
    }

    fn support(&self, c: IVec3) -> bool {
        self.planned.contains(&c) || self.lanes[1].get(c, &self.support)
    }

    fn fluid(&self, c: IVec3) -> bool {
        !self.planned.contains(&c) && self.lanes[2].get(c, &self.fluid)
    }

    fn step_allowed(&self, from: IVec3, to: IVec3) -> bool {
        !self.planned.contains(&to) && (self.step)(from, to)
    }
}

trait GraphSearch {
    type Found;
    fn run<W: NavWorld>(self, graph: &path::BoxGraph<'_, W>) -> Self::Found;
}

fn search_over<S: GraphSearch>(
    world: &ServerWorld,
    kind: Mob,
    params: PathParams,
    planned: &[IVec3],
    kept: &KeptBox,
    search: S,
) -> S::Found {
    let d = def(kind);
    let cursor = world.cursor();
    let planned_world = PlannedWorld {
        planned,
        lanes: [FactLane::Solid, FactLane::Support, FactLane::Fluid].map(|l| kept.lane(l)),
        solid: nav_solid_fn(&cursor),
        support: nav_support_fn(&cursor, d.size.half_width),
        fluid: nav_fluid_fn(&cursor),
        step: navigation_step_gate_on(
            &cursor,
            params,
            d.size.height,
            [
                FactLane::Partial,
                FactLane::Hazard,
                FactLane::HazardFoothold,
                FactLane::Plain,
            ]
            .map(|l| kept.lane(l)),
        ),
    };
    search.run(&path::BoxGraph {
        params,
        world: &planned_world,
        foothold_memo: kept.lane(FactLane::Foothold),
        passable_memo: kept.lane(FactLane::Passable),
        leads: &kept.leads,
        planned: Planned {
            cells: planned,
            reads: kept.reads,
        },
    })
}

struct ReachTo {
    from: IVec3,
    to: IVec3,
}

impl GraphSearch for ReachTo {
    type Found = (Option<bool>, usize);
    fn run<W: NavWorld>(self, graph: &path::BoxGraph<'_, W>) -> Self::Found {
        path::reachable_on(self.from, self.to, graph)
    }
}

struct FloodOver<'a> {
    ask: &'a FloodAsk<'a>,
    comes: &'a path::BoxLeads,
}

impl GraphSearch for FloodOver<'_> {
    type Found = (Option<Vec<(IVec3, u32)>>, usize);
    fn run<W: NavWorld>(self, graph: &path::BoxGraph<'_, W>) -> Self::Found {
        path::walk_region(
            self.ask.from,
            self.ask.span,
            self.ask.toward,
            graph,
            self.comes,
        )
    }
}

/// Can a `kind` body walk from `from` to `to`? Cells in `blocked` count as solid (planned, not
/// built). Hitting `max_nodes` gives `Undecided`, never `Closed`, since a long detour isn't a
/// wall. `None` means the route budget can't cover `max_nodes` more expansions this tick; ask
/// again later. Engine seam for the `PathProbe` HostCall.
pub fn route_probe(
    world: &ServerWorld,
    kind: Mob,
    from: IVec3,
    to: IVec3,
    blocked: &[IVec3],
    max_nodes: usize,
) -> Option<Route> {
    let max_nodes = max_nodes.min(ROUTE_PROBE_MAX_NODES);
    let budget = world.route_probe_budget();
    if !budget.covers(max_nodes) {
        return None;
    }
    let mut params = def(kind).path_params();
    params.max_nodes = max_nodes;
    let (reached, nodes) = world.kept_boxes().over(
        world,
        kind,
        params,
        (from.min(to), from.max(to)),
        kept::probe_span(from, to),
        |kept| search_over(world, kind, params, blocked, kept, ReachTo { from, to }),
    );
    budget.spend(nodes);
    Some(match reached {
        Some(true) => Route::Open,
        Some(false) => Route::Closed,
        None => Route::Undecided,
    })
}

pub struct FloodAsk<'a> {
    pub from: IVec3,
    pub span: (IVec3, IVec3),
    pub toward: bool,
    pub blocked: &'a [IVec3],
    pub max_nodes: usize,
}

pub fn walk_region(world: &ServerWorld, kind: Mob, ask: FloodAsk<'_>) -> mod_api::Flood {
    let max_nodes = ask.max_nodes.min(ROUTE_PROBE_TICK_BUDGET);
    let budget = world.route_probe_budget();
    if !budget.covers(max_nodes) {
        return mod_api::Flood::Deferred;
    }
    let mut params = def(kind).path_params();
    params.max_nodes = max_nodes;
    let (cells, nodes) = world
        .kept_boxes()
        .over(world, kind, params, ask.span, ask.span, |kept| {
            let comes = &kept.comes;
            search_over(
                world,
                kind,
                params,
                ask.blocked,
                kept,
                FloodOver { ask: &ask, comes },
            )
        });
    budget.spend(nodes);
    match cells {
        Some(cells) => mod_api::Flood::Reached(
            cells
                .iter()
                .map(|(c, moves)| (c.to_array(), *moves))
                .collect(),
        ),
        None => mod_api::Flood::Exceeded,
    }
}

#[cfg(any(test, feature = "test-support"))]
pub fn route_path(world: &ServerWorld, kind: Mob, from: IVec3, to: IVec3) -> Vec<IVec3> {
    let d = def(kind);
    let params = d.path_params();
    let cursor = world.cursor();
    let solid = nav_solid_fn(&cursor);
    let support = nav_support_fn(&cursor, d.size.half_width);
    let fluid = nav_fluid_fn(&cursor);
    let step_allowed = navigation_step_gate(&cursor, params, d.size.height);
    path::find_path_nav(
        from,
        to,
        params,
        &solid,
        &support,
        &fluid,
        step_allowed,
        |_| 0,
    )
}

pub fn footholds(world: &ServerWorld, kind: Mob, cells: &[IVec3]) -> Vec<bool> {
    let d = def(kind);
    let params = d.path_params();
    let cursor = world.cursor();
    let solid = nav_solid_fn(&cursor);
    let support = nav_support_fn(&cursor, d.size.half_width);
    let fluid = nav_fluid_fn(&cursor);
    cells
        .iter()
        .map(|&cell| {
            path::is_navigation_foothold_with(cell, params, &solid, &support, &fluid)
                && !hazards::foothold_in_hazard(&cursor, cell, params)
        })
        .collect()
}

pub fn site_open(world: &ServerWorld, kind: Mob, cell: IVec3) -> bool {
    let d = def(kind);
    let params = d.path_params();
    let cursor = world.cursor();
    let solid = nav_solid_fn(&cursor);
    let support = nav_support_fn(&cursor, d.size.half_width);
    let fluid = nav_fluid_fn(&cursor);
    if !path::is_navigation_foothold_with(cell, params, &solid, &support, &fluid)
        || hazards::foothold_in_hazard(&cursor, cell, params)
    {
        return false;
    }
    let step_allowed = navigation_step_gate(&cursor, params, d.size.height);
    let loaded = nav_loaded_fn(&cursor);
    crate::mob::confined::confined_region(
        cell,
        params,
        &solid,
        &support,
        &fluid,
        &step_allowed,
        &loaded,
    )
    .is_none()
}
