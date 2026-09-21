//! Reachability questions asked of the world for a species rather than by a
//! live mob's navigator: is a destination walked to, does one foothold walk
//! to another with planned cells built, which footholds of a box are walked
//! to, which cells are footholds at all.

use mod_api::Route;
use petramond_math::math::IVec3;

use super::budget::{ReachBudget, REACH_PROBE_NODES};
use super::kept::{self, FactLane, KeptBox};
use super::step_gate::{navigation_step_gate, navigation_step_gate_on};
use super::{fluid_footing, hazards, nav_fluid_fn, nav_loaded_fn, nav_solid_fn, nav_support_fn};
use crate::mob::path::{self, CellCache, Fact, NavWorld, PathParams, Planned};
use crate::mob::{def, Instance, Mob};
use crate::world::World;

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
    world: &World,
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
        // Charged with its SETUP too: a probe that answers in twenty
        // expansions still built a cursor, four closures and two memo tables,
        // so expansions alone under-price a burst of cheap probes.
        b.spend(nodes);
    }
    Some(out.unwrap_or(false))
}

/// `destination_reachable` for a live mob instance: probes from the mob's
/// current navigation cell with its real body. `false` when the mob is not on
/// a foothold (airborne — nothing is provable, callers retry later). The
/// `MobCanReach` HostCall's engine seam.
pub fn mob_can_reach(world: &World, mob: &Instance, dest: IVec3) -> bool {
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
    // The mod ABI shares the tick's probe budget (a husbandry sweep can ask
    // for dozens of probes in one tick); a refusal reads as "not reachable",
    // which is what the asking policies already do with a spot they cannot
    // prove — they re-roll on their next heartbeat.
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

/// Expansions every [`route_probe`] in one game tick may spend between
/// them. A route probe is asked positionally and at its asker's own cadence
/// (would this foothold still walk home if that wall stood), over distances
/// a wander probe never covers, so it spends from its own budget rather
/// than starving the brains'.
pub const ROUTE_PROBE_TICK_BUDGET: usize = 32_000;

/// The most expansions one route probe may ask for: the navigator's own
/// per-route search budget.
pub const ROUTE_PROBE_MAX_NODES: usize = 4000;

/// The world as a search walks it with `planned` cells read as built, its
/// per-cell reads memoized on a kept box's lanes.
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

    // The sweep itself reads the world as it stands: a planned cell is
    // refused as a destination, not swept against.
    fn step_allowed(&self, from: IVec3, to: IVec3) -> bool {
        !self.planned.contains(&to) && (self.step)(from, to)
    }
}

/// A search to run over a [`path::BoxGraph`], whatever world it is built on.
trait GraphSearch {
    type Found;
    fn run<W: NavWorld>(self, graph: &path::BoxGraph<'_, W>) -> Self::Found;
}

/// Run `search` over species `kind`'s moves through `kept`'s box, `planned`
/// cells read as built.
fn search_over<S: GraphSearch>(
    world: &World,
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

/// Whether a body of species `kind` standing at foothold `from` can walk to
/// foothold `to`, treating every cell of `blocked` as a solid block (planned,
/// not built yet). A search that spends `max_nodes` first is `Undecided`,
/// never `Closed`: a long detour is not a wall. `None` when this tick's route
/// budget cannot cover `max_nodes` more expansions: unknown, ask again next
/// tick. The `PathProbe` HostCall's engine seam.
pub fn route_probe(
    world: &World,
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

/// A flood over a box: from where, which way, and with what read as built.
pub struct FloodAsk<'a> {
    pub from: IVec3,
    /// The inclusive box the flood stays in.
    pub span: (IVec3, IVec3),
    /// Footholds that walk TO `from`, rather than from it.
    pub toward: bool,
    pub blocked: &'a [IVec3],
    pub max_nodes: usize,
}

/// Every foothold inside the ask's box that a body of species `kind` walks to
/// from `from` without leaving the box (`toward`: that walks to `from`),
/// every `blocked` cell treated as a solid block. One flood answers what a
/// probe per cell would, and a detour inside the box is never cut short the
/// way a capped search is: `Deferred` when this tick's route budget cannot
/// cover `max_nodes`, `Exceeded` when the box holds more footholds.
pub fn walk_region(world: &World, kind: Mob, ask: FloodAsk<'_>) -> mod_api::Flood {
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

/// The cells a body of species `kind` would walk from `from` to `to`, as the
/// navigator plans them with no bodies in the way (a best-effort partial
/// route when the goal is out of reach). A diagnostic seam.
#[cfg(any(test, feature = "test-support"))]
pub fn route_path(world: &World, kind: Mob, from: IVec3, to: IVec3) -> Vec<IVec3> {
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

/// Which of `cells` a body of species `kind` could stand in: a navigation
/// foothold with room for the body, not in a hazard.
pub fn footholds(world: &World, kind: Mob, cells: &[IVec3]) -> Vec<bool> {
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

/// Whether `cell` is somewhere a body of species `kind` could stand and still
/// ROAM: a navigation foothold whose reachable ground is open world rather
/// than a closed-off region (see [`crate::mob::confined`]). The `SiteOpen`
/// HostCall's engine seam.
///
/// This is the question a spawner asks about a site it picked itself, and both
/// halves matter: the foothold half is why a site over a hole or inside rock
/// answers `false` instead of dropping a body into it, and the confinement
/// half is why a pen someone built stays a pen — the engine's own confinement
/// probe, asked positionally so the answer arrives BEFORE a mob exists.
pub fn site_open(world: &World, kind: Mob, cell: IVec3) -> bool {
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
