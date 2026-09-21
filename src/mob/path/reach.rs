//! Reachability over the pathfinder's moves: whether a goal is walked to, and
//! every foothold of a box that is. The same moves as [`super::find_path_nav`]
//! minus everything only a ROUTE needs.

use std::cmp::Reverse;
use std::collections::BinaryHeap;

use petramond_math::math::IVec3;
use rustc_hash::{FxHashMap, FxHashSet};

use super::{
    body_clear, fits_a_table, is_navigation_foothold_with, neighbors, BoxLeads, CellCache,
    CellMemo, Fact, PathParams, CLIMB_CELLS, COST_DIAG, COST_DROP_PER_BLOCK, COST_FLAT, COST_JUMP,
};

/// Whether `goal` is reachable from `start` within `params.max_nodes`
/// expansions (`None`: they ran out before the search decided), and how many
/// expansions that verdict cost.
///
/// The same search as [`super::find_path_nav`] — same order, same admissible
/// octile heuristic, same neighbour rules — minus everything only a ROUTE
/// needs: no predecessor map, no closest-cell fallback, no reconstruction. A
/// probe that FAILS pays the whole budget, and the predecessor map alone is
/// four hash inserts per expansion, so a reachability question answers for a
/// fraction of what asking for the path costs. The verdict is identical by
/// construction: dropping `came_from` cannot change which cells the open set
/// pops.
///
/// The expansion count is what a caller throttling probes across one tick
/// charges against its budget (see `mob::nav::ReachBudget`).
pub fn reachable_nav(
    start: IVec3,
    goal: IVec3,
    params: PathParams,
    solid: &impl Fn(IVec3) -> bool,
    support: &impl Fn(IVec3) -> bool,
    fluid: impl Fn(IVec3) -> bool,
    step_allowed: impl Fn(IVec3, IVec3) -> bool,
) -> (Option<bool>, usize) {
    let passable_col = |c: IVec3| body_clear(c, params, solid);
    let memo = CellMemo::<2048>::default();
    let foothold = |c: IVec3| {
        memo.get(c, |c| {
            is_navigation_foothold_with(c, params, solid, support, &fluid)
        })
    };
    reachable_by(
        start,
        goal,
        params.max_nodes,
        foothold(start),
        |a, steps| {
            neighbors(
                a,
                &params,
                &foothold,
                &passable_col,
                solid,
                &step_allowed,
                steps,
            );
        },
    )
}

/// The search of [`reachable_nav`], the moves out of a foothold left to
/// `moves` (it fills its buffer with `(cell, cost)`).
fn reachable_by(
    start: IVec3,
    goal: IVec3,
    max_nodes: usize,
    start_is_foothold: bool,
    mut moves: impl FnMut(IVec3, &mut Vec<(IVec3, u32)>),
) -> (Option<bool>, usize) {
    if !start_is_foothold {
        return (Some(false), 0);
    }
    if start == goal {
        return (Some(true), 0);
    }

    let h = |c: IVec3| -> u32 {
        let dx = (c.x - goal.x).unsigned_abs();
        let dz = (c.z - goal.z).unsigned_abs();
        let (lo, hi) = if dx < dz { (dx, dz) } else { (dz, dx) };
        COST_DIAG * lo + COST_FLAT * (hi - lo)
    };

    let mut g_score: FxHashMap<IVec3, u32> = FxHashMap::default();
    let mut open: BinaryHeap<Reverse<(u32, u32, [i32; 3])>> = BinaryHeap::new();
    g_score.insert(start, 0);
    open.push(Reverse((h(start), 0, start.to_array())));

    let mut expanded = 0usize;
    let mut steps: Vec<(IVec3, u32)> = Vec::with_capacity(8);
    while let Some(Reverse((_, g_at_pop, pos_arr))) = open.pop() {
        let current = IVec3::from_array(pos_arr);
        if g_at_pop > *g_score.get(&current).unwrap_or(&u32::MAX) {
            continue;
        }
        if current == goal {
            return (Some(true), expanded);
        }
        expanded += 1;
        if expanded >= max_nodes {
            return (None, expanded);
        }
        moves(current, &mut steps);
        for &(next, step_cost) in &steps {
            let tentative = g_at_pop.saturating_add(step_cost);
            if tentative < *g_score.get(&next).unwrap_or(&u32::MAX) {
                g_score.insert(next, tentative);
                open.push(Reverse((tentative + h(next), tentative, next.to_array())));
            }
        }
    }
    (Some(false), expanded)
}

/// What a search asks of the world it walks: the pathfinder's four
/// predicates, as one value instead of four closures borrowing one another.
pub trait NavWorld {
    fn solid(&self, c: IVec3) -> bool;
    fn support(&self, c: IVec3) -> bool;
    fn fluid(&self, c: IVec3) -> bool;
    fn step_allowed(&self, from: IVec3, to: IVec3) -> bool;
}

/// How far from a cell the pathfinder reads to answer for it, as (toward
/// −x/−y/−z, toward +x/+y/+z): its standing room, its moves, and the moves
/// into it. Whatever is kept about a cell is stale once a cell inside that
/// reach changes — and a changed cell `x` is read by the cells from
/// `x − reach.1` to `x + reach.0`, the other way round.
#[derive(Clone, Copy)]
pub struct Reads {
    pub standing: (IVec3, IVec3),
    pub leads: (IVec3, IVec3),
    pub comes: (IVec3, IVec3),
}

impl Reads {
    pub fn of(params: PathParams) -> Self {
        // The footprint, and one more for a body wider than its cell.
        let side = 1 + params.half_width.max(0.0).ceil() as i32;
        let head = params.head_cells();
        // A foothold reads its floor and its head room; a plain column one
        // more each way.
        let standing = (IVec3::new(side, 2, side), IVec3::new(side, head + 1, side));
        // Its moves read a neighbour's standing room: down a drop's column to
        // the floor under its landing, up past a climb's head room.
        let leads = (
            IVec3::new(side + 1, params.max_drop + 2, side + 1),
            IVec3::new(side + 1, head + 2, side + 1),
        );
        // The moves into it are those of every foothold a step, a climb or a
        // drop away.
        let comes = (
            leads.0 + IVec3::ONE,
            leads.1 + IVec3::new(1, params.max_drop, 1),
        );
        Reads {
            standing,
            leads,
            comes,
        }
    }

    /// Every cell read by whatever a search over `min..=max` asks.
    pub fn around(&self, min: IVec3, max: IVec3) -> (IVec3, IVec3) {
        (min - self.leads.0, max + self.leads.1)
    }

    /// The cells that read `changed` for one of the three, as a box.
    pub fn readers(changed: IVec3, reach: (IVec3, IVec3)) -> (IVec3, IVec3) {
        (changed - reach.1, changed + reach.0)
    }
}

/// Cells a search treats as built though the world does not hold them yet.
/// What is kept about the cells that read one is this search's alone: never
/// read from nor written to what outlives it.
#[derive(Clone, Copy)]
pub struct Planned<'a> {
    pub cells: &'a [IVec3],
    pub reads: Reads,
}

impl Planned<'_> {
    fn touches(&self, c: IVec3, reach: (IVec3, IVec3)) -> bool {
        self.cells.iter().any(|planned| {
            let (min, max) = Reads::readers(*planned, reach);
            c.cmpge(min).all() && c.cmple(max).all()
        })
    }
}

/// The moves of one search's world, with its standing-room memos and the
/// kept move lists of the box it searches.
pub struct BoxGraph<'a, W> {
    pub params: PathParams,
    pub world: &'a W,
    pub foothold_memo: Fact<'a>,
    pub passable_memo: Fact<'a>,
    pub leads: &'a BoxLeads,
    pub planned: Planned<'a>,
}

impl<W: NavWorld> BoxGraph<'_, W> {
    fn passable_col(&self, c: IVec3) -> bool {
        let clear = |c| body_clear(c, self.params, &|c| self.world.solid(c));
        if self.planned.touches(c, self.planned.reads.standing) {
            return clear(c);
        }
        self.passable_memo.get(c, clear)
    }

    pub fn foothold(&self, c: IVec3) -> bool {
        let stands = |c| {
            is_navigation_foothold_with(
                c,
                self.params,
                &|c| self.world.solid(c),
                &|c| self.world.support(c),
                &|c| self.world.fluid(c),
            )
        };
        if self.planned.touches(c, self.planned.reads.standing) {
            return stands(c);
        }
        self.foothold_memo.get(c, stands)
    }

    /// Where `a` leads in one move, in the pathfinder's own order (nowhere,
    /// for a cell that is no foothold).
    pub fn leads_of(&self, a: IVec3, out: &mut Vec<IVec3>) {
        let volatile = self.planned.touches(a, self.planned.reads.leads);
        self.leads.of(a, volatile, out, |out| {
            if !self.foothold(a) {
                return;
            }
            let mut steps = Vec::with_capacity(8);
            neighbors(
                a,
                &self.params,
                &|c| self.foothold(c),
                &|c| self.passable_col(c),
                &|c| self.world.solid(c),
                &|from, to| self.world.step_allowed(from, to),
                &mut steps,
            );
            out.extend(steps.iter().map(|(to, _)| *to));
        });
    }
}

/// What [`neighbors`] prices the move `a → to` at, from its shape alone.
fn move_cost(a: IVec3, to: IVec3) -> u32 {
    match to.y - a.y {
        rise if rise > 0 => COST_JUMP,
        0 if to.x != a.x && to.z != a.z => COST_DIAG,
        0 => COST_FLAT,
        drop => COST_FLAT + drop.unsigned_abs() * COST_DROP_PER_BLOCK,
    }
}

/// [`reachable_nav`] over a [`BoxGraph`]: the same search in the same order,
/// so the same verdict and the same expansion count, with every foothold's
/// moves read from what the box keeps.
pub fn reachable_on<W: NavWorld>(
    start: IVec3,
    goal: IVec3,
    graph: &BoxGraph<'_, W>,
) -> (Option<bool>, usize) {
    let mut led = Vec::with_capacity(8);
    reachable_by(
        start,
        goal,
        graph.params.max_nodes,
        graph.foothold(start),
        |a, steps| {
            graph.leads_of(a, &mut led);
            steps.clear();
            steps.extend(led.iter().map(|to| (*to, move_cost(a, *to))));
        },
    )
}

/// Every foothold inside the inclusive box `min..=max` that a body walks to
/// from foothold `start` without stepping out of the box, or with `toward`
/// every foothold in it that walks to `start` so; `start` included, each with
/// its moves from (to) `start`, breadth first. `comes` keeps what leads into
/// each cell, like the graph's own lists. `None` once more than
/// `params.max_nodes` footholds are found (the box holds more ground than
/// asked for). The count is the expansions spent, for the caller's budget.
pub fn walk_region<W: NavWorld>(
    start: IVec3,
    (min, max): (IVec3, IVec3),
    toward: bool,
    graph: &BoxGraph<'_, W>,
    comes: &BoxLeads,
) -> (Option<Vec<(IVec3, u32)>>, usize) {
    let params = graph.params;
    let inside = |c: IVec3| c.cmpge(min).all() && c.cmple(max).all();
    if !inside(start) || !graph.foothold(start) {
        return (Some(Vec::new()), 0);
    }
    let mut found = vec![start];
    // Moves from the start, parallel to `found`.
    let mut depth = vec![0u32];
    let mut seen = Seen::over(min, max);
    seen.first(start);
    let mut led: Vec<IVec3> = Vec::with_capacity(8);
    let mut came: Vec<IVec3> = Vec::with_capacity(8);
    let mut next = 0;
    let mut expanded = 0;
    while next < found.len() {
        let current = found[next];
        let moved = depth[next] + 1;
        next += 1;
        expanded += 1;
        if toward {
            // Whatever moves INTO `current`: a climb from a layer below, a
            // flat or diagonal step, or a drop from up to `max_drop` above.
            const AROUND: [(i32, i32); 8] = [
                (1, 0),
                (-1, 0),
                (0, 1),
                (0, -1),
                (1, 1),
                (1, -1),
                (-1, 1),
                (-1, -1),
            ];
            let volatile = graph.planned.touches(current, graph.planned.reads.comes);
            comes.of(current, volatile, &mut came, |out| {
                for (dx, dz) in AROUND {
                    let rises = if dx != 0 && dz != 0 {
                        0..=0
                    } else {
                        -CLIMB_CELLS..=params.max_drop
                    };
                    for dy in rises {
                        let from = current + IVec3::new(dx, dy, dz);
                        graph.leads_of(from, &mut led);
                        if led.contains(&current) {
                            out.push(from);
                        }
                    }
                }
            });
            // Kept lists are the world's, not this flood's box's.
            for &from in &came {
                if inside(from) && seen.first(from) {
                    found.push(from);
                    depth.push(moved);
                }
            }
        } else {
            graph.leads_of(current, &mut led);
            for &to in &led {
                if inside(to) && seen.first(to) {
                    found.push(to);
                    depth.push(moved);
                }
            }
        }
        if found.len() > params.max_nodes {
            return (None, expanded);
        }
    }
    (Some(found.into_iter().zip(depth).collect()), expanded)
}

/// The cells of a box a flood has taken: a flat table where the box is small
/// enough for one.
enum Seen {
    Table {
        min: IVec3,
        dims: IVec3,
        taken: Vec<bool>,
    },
    Set(FxHashSet<IVec3>),
}

impl Seen {
    fn over(min: IVec3, max: IVec3) -> Self {
        if !fits_a_table(min, max) {
            return Seen::Set(FxHashSet::default());
        }
        let dims = max - min + IVec3::ONE;
        Seen::Table {
            min,
            dims,
            taken: vec![false; (dims.x * dims.y * dims.z) as usize],
        }
    }

    /// Whether `c` (a cell of the box) is taken now for the first time.
    fn first(&mut self, c: IVec3) -> bool {
        match self {
            Seen::Table { min, dims, taken } => {
                let d = c - *min;
                !std::mem::replace(
                    &mut taken[((d.y * dims.z + d.z) * dims.x + d.x) as usize],
                    true,
                )
            }
            Seen::Set(set) => set.insert(c),
        }
    }
}
