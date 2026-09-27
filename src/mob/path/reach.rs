use std::cmp::Reverse;
use std::collections::BinaryHeap;

use petramond_math::math::IVec3;
use rustc_hash::{FxHashMap, FxHashSet};

use super::{
    body_clear, fits_a_table, is_navigation_foothold_with, neighbors, BoxLeads, CellCache,
    CellMemo, Fact, PathParams, CLIMB_CELLS, COST_DIAG, COST_DROP_PER_BLOCK, COST_FLAT, COST_JUMP,
};

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

pub trait NavWorld {
    fn solid(&self, c: IVec3) -> bool;
    fn support(&self, c: IVec3) -> bool;
    fn fluid(&self, c: IVec3) -> bool;
    fn step_allowed(&self, from: IVec3, to: IVec3) -> bool;
}

#[derive(Clone, Copy)]
pub struct Reads {
    pub standing: (IVec3, IVec3),
    pub leads: (IVec3, IVec3),
    pub comes: (IVec3, IVec3),
}

impl Reads {
    pub fn of(params: PathParams) -> Self {
        let side = 1 + params.half_width.max(0.0).ceil() as i32;
        let head = params.head_cells();
        let standing = (IVec3::new(side, 2, side), IVec3::new(side, head + 1, side));
        let leads = (
            IVec3::new(side + 1, params.max_drop + 2, side + 1),
            IVec3::new(side + 1, head + 2, side + 1),
        );
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

    pub fn around(&self, min: IVec3, max: IVec3) -> (IVec3, IVec3) {
        (min - self.leads.0, max + self.leads.1)
    }

    pub fn readers(changed: IVec3, reach: (IVec3, IVec3)) -> (IVec3, IVec3) {
        (changed - reach.1, changed + reach.0)
    }
}

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

fn move_cost(a: IVec3, to: IVec3) -> u32 {
    match to.y - a.y {
        rise if rise > 0 => COST_JUMP,
        0 if to.x != a.x && to.z != a.z => COST_DIAG,
        0 => COST_FLAT,
        drop => COST_FLAT + drop.unsigned_abs() * COST_DROP_PER_BLOCK,
    }
}

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
