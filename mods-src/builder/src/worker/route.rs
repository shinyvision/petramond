//! Whether the golem can get somewhere, asked in pieces a capped search can
//! decide.
//!
//! One route search spends at most the navigator's own budget, and a detour
//! around a house from the far corner of the site outruns it: such a search
//! answers `Undecided`, which is no answer. So reachability is asked from the
//! golem's trail — the cells it stood on, each walked to from the one before
//! — as well as from home, and a long walk goes in legs back along the trail.

use crate::fx::HashMap;
use std::collections::VecDeque;

use mod_sdk::*;

use super::tuning::route::{
    HUBS, LEG, MEMORY, NEAR, NODES, REGION_NODES, SHORT, TRAIL, VERDICT_MEMORY,
};
use super::{Body, Ctx};
use crate::content::GOLEM;
use crate::geometry::manhattan;

pub type Routes = HashMap<([i32; 3], [i32; 3], u64), (Route, u64)>;

/// Footholds of the site walked to from a cell (or that walk to it), by
/// `(cell, toward, blocked cells' hash)`, and the tick the flood was asked.
pub type Regions = HashMap<([i32; 3], bool, u64), (Region, u64)>;

/// One flood's footholds, each with the moves between it and where the flood
/// began. Asked about by the hundred thousand per plan (every cell around
/// every candidate), so the box is a flat table rather than a map.
pub struct Region {
    cells: Vec<([i32; 3], u32)>,
    min: [i32; 3],
    dims: [i32; 3],
    /// Per cell of the box, moves + 1; 0 = not a foothold of the flood.
    table: Vec<u16>,
    /// Whether the flood covered all the site's ground it reaches.
    complete: bool,
}

impl Region {
    fn new(cells: Vec<([i32; 3], u32)>, (min, max): ([i32; 3], [i32; 3]), complete: bool) -> Self {
        let dims: [i32; 3] = std::array::from_fn(|i| (max[i] - min[i] + 1).max(0));
        let mut region = Region {
            table: vec![0; (dims[0] * dims[1] * dims[2]) as usize],
            cells,
            min,
            dims,
            complete,
        };
        for i in 0..region.cells.len() {
            let (cell, moves) = region.cells[i];
            if let Some(slot) = region.slot(cell) {
                region.table[slot] = (moves + 1).min(u32::from(u16::MAX)) as u16;
            }
        }
        region
    }

    fn slot(&self, cell: [i32; 3]) -> Option<usize> {
        let d: [i32; 3] = std::array::from_fn(|i| cell[i] - self.min[i]);
        (0..3)
            .all(|i| (d[i] as u32) < self.dims[i] as u32)
            .then(|| ((d[1] * self.dims[2] + d[2]) * self.dims[0] + d[0]) as usize)
    }

    pub fn contains(&self, cell: [i32; 3]) -> bool {
        self.moves(cell).is_some()
    }

    /// Moves between the foothold and where the flood began.
    pub fn moves(&self, cell: [i32; 3]) -> Option<u32> {
        match self.table[self.slot(cell)?] {
            0 => None,
            n => Some(u32::from(n) - 1),
        }
    }

    /// Every foothold, nearest the flood's start first.
    pub fn cells(&self) -> impl Iterator<Item = [i32; 3]> + '_ {
        self.cells.iter().map(|(cell, _)| *cell)
    }
}

/// Home and the cells the golem stood on since, oldest first.
#[derive(Clone, Default)]
pub struct Trail {
    cells: VecDeque<[i32; 3]>,
    /// The latest cell stood on per stretch of the site, so a place walked
    /// long ago still stands in for home after the recent trail moved on.
    spread: HashMap<[i32; 3], [i32; 3]>,
}

impl Trail {
    pub fn stood(&mut self, cell: [i32; 3]) {
        if self.cells.back() == Some(&cell) {
            return;
        }
        self.cells.retain(|c| *c != cell);
        self.cells.push_back(cell);
        if self.cells.len() > TRAIL {
            self.cells.pop_front();
        }
        let stretch = [
            cell[0].div_euclid(6),
            cell[1].div_euclid(4),
            cell[2].div_euclid(6),
        ];
        self.spread.insert(stretch, cell);
    }

    /// The body was set down somewhere it did not walk to.
    pub fn broken(&mut self) {
        self.cells.clear();
    }

    fn near(&self, to: [i32; 3]) -> Vec<(Option<usize>, [i32; 3])> {
        let recent = self
            .cells
            .iter()
            .copied()
            .enumerate()
            .map(|(i, c)| (Some(i), c));
        let spread = self
            .spread
            .values()
            .copied()
            .filter(|c| !self.cells.contains(c))
            .map(|c| (None, c));
        let mut near: Vec<(Option<usize>, [i32; 3])> = recent
            .chain(spread)
            .filter(|(_, c)| *c != to && manhattan(*c, to) <= NEAR)
            .collect();
        near.sort_by_key(|(_, c)| manhattan(*c, to));
        near.truncate(HUBS);
        near
    }
}

/// Where routes are known to start from: home, and the trail walked from it.
#[derive(Clone, Copy)]
pub struct Hubs<'a> {
    pub home: [i32; 3],
    pub trail: &'a Trail,
}

impl<'a> Hubs<'a> {
    pub fn new(home: [i32; 3], trail: &'a Trail) -> Self {
        Self { home, trail }
    }
}

/// Whether `from` walks to `to` with `blocked` treated as built. `None` = no
/// route budget left this tick.
pub fn probe(ctx: &mut Ctx, from: [i32; 3], to: [i32; 3], blocked: Vec<[i32; 3]>) -> Option<Route> {
    probe_within(ctx, from, to, blocked, NODES)
}

fn probe_within(
    ctx: &mut Ctx,
    from: [i32; 3],
    to: [i32; 3],
    blocked: Vec<[i32; 3]>,
    nodes: u32,
) -> Option<Route> {
    let key = key(from, to, &blocked);
    // Every remembered answer stands for any ask: an "undecided" is only
    // ever remembered from a full search (below).
    if let Some(&(answer, at)) = ctx.routes.get(&key) {
        if ctx.now < at + MEMORY {
            return Some(answer);
        }
    }
    if *ctx.probe_nodes == 0 {
        return None;
    }
    // The host charges the nodes a search actually spends and answers `None`
    // once this tick's budget cannot cover another; stop asking until then.
    let answer = path_probe(GOLEM, from, to, blocked, nodes);
    match answer {
        // Remembered as undecided only from a full search (see above).
        Some(Route::Undecided) if nodes < NODES => {}
        Some(route) => {
            ctx.routes.insert(key, (route, ctx.now));
        }
        None => *ctx.probe_nodes = 0,
    }
    answer
}

/// The footholds of the site that walk from `from` (`toward`: to `from`) with
/// `blocked` built, cached like a route answer. `None` = no route budget left
/// this tick; `Some(None)` = no flood answers (`from` off the site, or too much
/// ground for a flood).
pub fn region<'a>(
    ctx: &'a mut Ctx,
    from: [i32; 3],
    toward: bool,
    blocked: &[[i32; 3]],
) -> Option<Option<&'a Region>> {
    let (min, max) = ctx.site;
    if (0..3).any(|i| from[i] < min[i] || from[i] > max[i]) {
        return Some(None);
    }
    let key = (from, toward, key(from, from, blocked).2);
    let fresh = ctx
        .regions
        .get(&key)
        .is_some_and(|(_, at)| ctx.now < at + MEMORY);
    if !fresh {
        if *ctx.probe_nodes == 0 {
            return None;
        }
        let region = match walk_region(
            GOLEM,
            from,
            min,
            max,
            blocked.to_vec(),
            toward,
            REGION_NODES,
        ) {
            Flood::Reached(cells) => Region::new(cells, (min, max), true),
            Flood::Exceeded => {
                trace!(
                    "TRACE flood from {from:?} (toward {toward}) exceeds {REGION_NODES} footholds"
                );
                Region::new(Vec::new(), (min, min), false)
            }
            Flood::Deferred => {
                *ctx.probe_nodes = 0;
                return None;
            }
        };
        ctx.regions.insert(key, (region, ctx.now));
    }
    Some(
        ctx.regions
            .get(&key)
            .map(|(region, _)| region)
            .filter(|r| r.complete),
    )
}

/// How far a search between home and `cell` looks before the site's flood is
/// asked instead: not far inside the site, where the flood answers exactly.
fn within(ctx: &Ctx, cell: [i32; 3]) -> u32 {
    let (min, max) = ctx.site;
    if (0..3).all(|i| (min[i]..=max[i]).contains(&cell[i])) {
        SHORT
    } else {
        NODES
    }
}

pub fn remembered(ctx: &Ctx, from: [i32; 3], to: [i32; 3]) -> Option<Route> {
    ctx.routes
        .get(&key(from, to, &[]))
        .filter(|(_, at)| ctx.now < at + MEMORY)
        .map(|(route, _)| *route)
}

fn key(from: [i32; 3], to: [i32; 3], blocked: &[[i32; 3]]) -> ([i32; 3], [i32; 3], u64) {
    use std::hash::{Hash, Hasher};
    // The same cells in another order are the same question.
    let mut blocked = blocked.to_vec();
    blocked.sort_unstable();
    let mut hasher = crate::fx::FxHasher::default();
    blocked.hash(&mut hasher);
    (from, to, hasher.finish())
}

/// Whether `from` gets back to home (or to a trail cell near it) with
/// `blocked` built. `Undecided` only when no piece could be decided.
pub fn out(ctx: &mut Ctx, hubs: Hubs, from: [i32; 3], blocked: &[[i32; 3]]) -> Option<Route> {
    if from == hubs.home {
        return Some(Route::Open);
    }
    let direct = probe_within(ctx, from, hubs.home, blocked.to_vec(), within(ctx, from))?;
    if direct != Route::Undecided {
        return Some(direct);
    }
    // Too far round for one search: the site's flood toward home decides.
    if let Some(home) = region(ctx, hubs.home, true, blocked)? {
        return Some(if home.contains(from) {
            Route::Open
        } else {
            Route::Closed
        });
    }
    // No flood answers (too much ground for one): the search in full, then.
    let full = probe(ctx, from, hubs.home, blocked.to_vec())?;
    if full != Route::Undecided {
        return Some(full);
    }
    for (_, hub) in hubs.trail.near(from) {
        if !blocked.contains(&hub) && probe(ctx, from, hub, blocked.to_vec())? == Route::Open {
            return Some(Route::Open);
        }
    }
    trace!("TRACE route out of {from:?} undecided");
    Some(Route::Undecided)
}

/// Whether the golem gets from home (or a trail cell near `to`) to `to`.
fn into(ctx: &mut Ctx, hubs: Hubs, to: [i32; 3]) -> Option<bool> {
    match probe_within(ctx, hubs.home, to, Vec::new(), within(ctx, to))? {
        Route::Open => return Some(true),
        Route::Closed => return Some(false),
        Route::Undecided => {}
    }
    if let Some(home) = region(ctx, hubs.home, false, &[])? {
        return Some(home.contains(to));
    }
    match probe(ctx, hubs.home, to, Vec::new())? {
        Route::Open => return Some(true),
        Route::Closed => return Some(false),
        Route::Undecided => {}
    }
    for (_, hub) in hubs.trail.near(to) {
        if probe(ctx, hub, to, Vec::new())? == Route::Open {
            return Some(true);
        }
    }
    Some(false)
}

/// Whether the golem walks out to `to` and back again. Out first: from an
/// enclosed stance that search floods the enclosure and is cheap either way.
pub fn round_trip(ctx: &mut Ctx, hubs: Hubs, to: [i32; 3]) -> Option<bool> {
    if to == hubs.home {
        return Some(true);
    }
    let back = out(ctx, hubs, to, &[])?;
    // No answer is no verdict: remembering it as a failure skipped good
    // stances for a while.
    if back == Route::Undecided {
        return Some(false);
    }
    let reached = back == Route::Open && into(ctx, hubs, to)?;
    let verdict = if reached { Route::Open } else { Route::Closed };
    ctx.routes
        .insert(verdict_key(to, hubs.home), (verdict, ctx.now));
    Some(reached)
}

/// The route map entry holding a round trip's verdict, apart from any search.
fn verdict_key(to: [i32; 3], home: [i32; 3]) -> ([i32; 3], [i32; 3], u64) {
    key(to, home, &[[i32::MIN; 3]])
}

/// Whether a round trip to `to` failed within route memory: skipped without
/// spending a search, so the best-ranked dead ends cannot starve the rest.
pub fn failed_recently(ctx: &Ctx, to: [i32; 3], home: [i32; 3]) -> bool {
    let verdict = ctx
        .routes
        .get(&verdict_key(to, home))
        .is_some_and(|(route, at)| *route != Route::Open && ctx.now < at + VERDICT_MEMORY);
    verdict || remembered(ctx, to, home) == Some(Route::Closed)
}

/// The cell to walk to now on the way to `to`: `to` itself, or a trail cell
/// back toward it when the whole way is more than one search decides.
pub fn leg(ctx: &mut Ctx, hubs: Hubs, body: &Body, to: [i32; 3]) -> Option<Option<[i32; 3]>> {
    if to == body.cell {
        return Some(Some(to));
    }
    // Far off, the navigator's own search gives up on routes a probe still
    // finds, so go in legs a short search always walks: each the nearby
    // foothold fewest moves from `to` by the flood back from it.
    if manhattan(body.cell, to) > LEG {
        if let Some(toward) = region(ctx, to, true, &[])? {
            let Some(here) = toward.moves(body.cell) else {
                return Some(None);
            };
            let mut legs: Vec<([i32; 3], u32)> = toward
                .cells
                .iter()
                .filter(|(c, moves)| *moves < here && manhattan(*c, body.cell) <= LEG)
                .copied()
                .collect();
            legs.sort_by_key(|(c, moves)| (*moves, *c));
            for (cell, _) in legs.into_iter().take(4) {
                if probe(ctx, body.cell, cell, Vec::new())? == Route::Open {
                    return Some(Some(cell));
                }
            }
        }
    }
    // Always a fresh answer: on a stale "open" the navigator crowds into
    // whatever now blocks the goal.
    ctx.routes.remove(&key(body.cell, to, &[]));
    match probe(ctx, body.cell, to, Vec::new())? {
        Route::Open => return Some(Some(to)),
        Route::Closed => return Some(mob_can_reach(body.id, to).then_some(to)),
        Route::Undecided => {}
    }
    let len = hubs.trail.cells.len();
    for (index, hub) in hubs.trail.near(to) {
        if hub == body.cell || probe(ctx, hub, to, Vec::new())? != Route::Open {
            continue;
        }
        let Some(index) = index else {
            if probe(ctx, body.cell, hub, Vec::new())? == Route::Open {
                return Some(Some(hub));
            }
            continue;
        };
        // Back along the trail toward the hub: the earliest of a few cells
        // between it and the golem that one search reaches.
        for step in 0..4 {
            let at = index + (len - 1 - index) * step / 4;
            let cell = hubs.trail.cells[at];
            if cell != body.cell && probe(ctx, body.cell, cell, Vec::new())? == Route::Open {
                return Some(Some(cell));
            }
        }
    }
    Some(mob_can_reach(body.id, to).then_some(to))
}

/// Moves from `from` to `to` by the site's flood from `from`; where no flood
/// answers or it does not reach, twice the distance as the crow flies.
pub fn moves_or_guess(ctx: &mut Ctx, from: [i32; 3], to: [i32; 3]) -> i32 {
    region(ctx, from, false, &[])
        .flatten()
        .and_then(|region| region.moves(to))
        .map_or(crate::geometry::manhattan(to, from) * 2, |m| m as i32)
}
