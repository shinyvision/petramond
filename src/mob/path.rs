//! Grid pathfinding for walking mobs: A* over **footholds** (cells a mob can
//! stand in), with movement rules that match how a mob actually moves —
//! step flat, climb exactly [`CLIMB_CELLS`], or walk off a ledge and fall up
//! to a capped height. No move climbs higher; no descent exceeds
//! [`PathParams::max_drop`].
//!
//! Pure and world-agnostic: the search takes closures plus the mob's body
//! footprint, so it is fully unit-testable against a stub world. Two occupancy
//! predicates keep cell probes honest about partial-collision shapes:
//! `solid(cell)` marks cells whose collision fills the whole cell (a body can
//! never be there), while `support(cell)` marks cells that can bear feet (any
//! collision at all — a slab, a bed, a ladder column's top). A cell with
//! partial collision is therefore routable in principle; whether a specific
//! body actually fits through a specific move is the `step_allowed` edge
//! gate's call (the world adapter sweeps the real body AABB against the real
//! collision boxes — see `mob::nav`). `cell_cost` adds a per-cell surcharge so
//! soft obstacles (other mobs, players) are routed around when a detour
//! exists without ever walling a route off.
//!
//! [`find_path_nav`] returns the foothold cells from the start toward the goal; if
//! the goal is unreachable it returns the path to the reachable cell that gets
//! **closest** to the goal (a best-effort partial path), so a mob always makes
//! progress instead of standing still.

use rustc_hash::FxHashMap;
use std::cmp::Reverse;
use std::collections::BinaryHeap;

use petramond_math::math::IVec3;
use petramond_world::block::Block;

mod box_memo;
mod reach;

pub use box_memo::{fits_a_table, BoxFacts, BoxLeads, Fact};
pub use reach::{reachable_nav, reachable_on, walk_region, BoxGraph, NavWorld, Planned, Reads};

/// Cells one climb edge rises. The body delivers it with a jump on land and a
/// shore climb from fluid footing (`entity::shore`), which reads this reach.
pub const CLIMB_CELLS: i32 = 1;

/// Cost of a flat (same-level) step. Costs are integers (scaled ×10 of "one cell")
/// so the open set can order on a total `Ord` without floats.
pub(crate) const COST_FLAT: u32 = 10;
/// A one-block jump up costs a little more than a flat step, so the route prefers
/// level ground when both reach the goal equally fast.
const COST_JUMP: u32 = 14;
/// A flat diagonal step (≈ `COST_FLAT * √2`). Diagonals are only taken on open flat
/// ground, so a clear straight-ish route is the shortest one instead of a staircase.
const COST_DIAG: u32 = 14;
/// Per-block surcharge for a descent, so a gentle route is preferred over a
/// plunge when both are otherwise equal (descents are still cheap — they're free
/// movement, just mildly discouraged when avoidable).
const COST_DROP_PER_BLOCK: u32 = 1;

/// Tuning for [`find_path_nav`]. `head` is the mob's vertical clearance in whole cells
/// (how many cells above the floor its body needs); `half_width` is its horizontal
/// body radius from centre to side; `max_drop` caps how far a descent may fall;
/// `max_nodes` bounds the search so one pathfind can't stall the tick;
/// `tolerated` lists the hazardous blocks this body's species may route through.
#[derive(Copy, Clone, Debug)]
pub struct PathParams {
    pub head: i32,
    pub half_width: f32,
    pub max_drop: i32,
    pub max_nodes: usize,
    pub tolerated: &'static [Block],
}

impl Default for PathParams {
    fn default() -> Self {
        PathParams {
            head: 1,
            half_width: 0.25,
            max_drop: 3,
            max_nodes: 4000,
            tolerated: &[],
        }
    }
}

impl PathParams {
    pub fn for_body(head: i32, half_width: f32) -> Self {
        PathParams {
            head: head.max(1),
            half_width: half_width.max(0.0),
            ..Default::default()
        }
    }

    /// These params for a species that tolerates `blocks` (see `MobDef::tolerates`).
    pub fn tolerating(self, blocks: &'static [Block]) -> Self {
        PathParams {
            tolerated: blocks,
            ..self
        }
    }

    #[inline]
    pub fn head_cells(self) -> i32 {
        self.head.max(1)
    }

    #[inline]
    fn body_half_width(self) -> f32 {
        self.half_width.max(0.0)
    }
}

/// Direct-mapped memo for a pure per-cell predicate over ONE search.
///
/// Both cell searches ask the same cell many times over — a flood asks each
/// neighbour from up to four sides, and A*'s diagonal rule re-asks the two
/// orthogonals it just tested — while the predicate itself is a whole
/// support/solid/fluid probe stack. A fixed table keyed by a cell hash turns
/// the repeats into two array reads; a collision simply recomputes, so the
/// answer is always the predicate's own.
pub struct CellMemo<const N: usize> {
    key: [std::cell::Cell<IVec3>; N],
    val: [std::cell::Cell<bool>; N],
}

/// A cell coordinate no probe can ever ask about, so an untouched slot misses.
const MEMO_EMPTY: IVec3 = IVec3::new(i32::MIN, i32::MIN, i32::MIN);

impl<const N: usize> Default for CellMemo<N> {
    fn default() -> Self {
        assert!(N.is_power_of_two());
        CellMemo {
            key: [const { std::cell::Cell::new(MEMO_EMPTY) }; N],
            val: [const { std::cell::Cell::new(false) }; N],
        }
    }
}

impl<const N: usize> CellMemo<N> {
    #[inline]
    pub fn get(&self, c: IVec3, compute: impl FnOnce(IVec3) -> bool) -> bool {
        let h = (c.x as u32)
            .wrapping_mul(0x9E37_79B9)
            .wrapping_add((c.y as u32).wrapping_mul(0x85EB_CA6B))
            .wrapping_add((c.z as u32).wrapping_mul(0xC2B2_AE35));
        let slot = (h >> 13) as usize & (N - 1);
        if self.key[slot].get() == c {
            return self.val[slot].get();
        }
        let v = compute(c);
        self.key[slot].set(c);
        self.val[slot].set(v);
        v
    }
}

/// A memo for a pure per-cell predicate over one search.
pub trait CellCache {
    fn get(&self, c: IVec3, compute: impl FnOnce(IVec3) -> bool) -> bool;
}

impl<const N: usize> CellCache for CellMemo<N> {
    #[inline]
    fn get(&self, c: IVec3, compute: impl FnOnce(IVec3) -> bool) -> bool {
        CellMemo::get(self, c, compute)
    }
}

/// Is `cell` a foothold — a cell a mob can stand in? Its floor (the cell below)
/// blocks movement under the whole body footprint, and the `head` cells from `cell`
/// upward are clear for that footprint.
/// Shared by the pathfinder, the navigator, and wander destination picking so they
/// all agree on what "standable" means.
pub fn is_foothold(cell: IVec3, params: PathParams, solid: &impl Fn(IVec3) -> bool) -> bool {
    supported_foothold(cell, params, solid, solid)
}

/// Is `cell` a navigation foothold when fluid may support the body? Fluid support
/// only counts at the surface: submerged cells are passable, not standable waypoints.
#[cfg(test)]
fn is_navigation_foothold(
    cell: IVec3,
    params: PathParams,
    solid: &impl Fn(IVec3) -> bool,
    fluid: &impl Fn(IVec3) -> bool,
) -> bool {
    is_navigation_foothold_with(cell, params, solid, solid, fluid)
}

/// Navigation foothold probing with a separate `support` predicate: `solid`
/// marks fully-blocked cells (body clearance), while `support` marks any cell
/// that can bear feet — including partial-collision shapes (a slab, a bed, a
/// ladder column) that do not blanket-block their cell. Whether a body truly
/// fits a specific move through a partial cell is the edge gate's call.
pub fn is_navigation_foothold_with(
    cell: IVec3,
    params: PathParams,
    solid: &impl Fn(IVec3) -> bool,
    support: &impl Fn(IVec3) -> bool,
    fluid: &impl Fn(IVec3) -> bool,
) -> bool {
    let bearing = |p: IVec3| support(p) || fluid(p);
    supported_foothold(cell, params, &bearing, solid) && body_layer_clear(cell, params, fluid)
}

/// Find the foothold cell a mob is standing in, given its feet position `pos` and
/// footprint `half_width`. Prefers the cell under the mob's centre; if that centre
/// overhangs an edge (its floor is air) it falls back to the foothold under a
/// footprint corner nearest the centre — the block the mob is actually resting on.
/// `None` if the mob is over no foothold (e.g. mid-air).
///
/// Without this, a mob standing at a block edge — centre over the drop, body still on
/// the block — would have a non-foothold "current cell", so [`find_path_nav`] would bail
/// and it would never path anywhere (it'd freeze at the edge).
#[cfg(test)]
fn standing_cell(
    pos: petramond_math::world_pos::WorldPos,
    half_width: f32,
    head: i32,
    solid: &impl Fn(IVec3) -> bool,
) -> Option<IVec3> {
    standing_cell_with(pos, half_width, head, solid, solid)
}

/// Standing-cell resolution with a separate `support` predicate (see
/// [`is_navigation_foothold_with`]): partial-collision blocks bear feet
/// without blanket-blocking their cell.
pub fn standing_cell_with(
    pos: petramond_math::world_pos::WorldPos,
    half_width: f32,
    head: i32,
    solid: &impl Fn(IVec3) -> bool,
    support: &impl Fn(IVec3) -> bool,
) -> Option<IVec3> {
    let params = PathParams::for_body(head, half_width);
    let feet_y = pos.y.floor() as i32;
    let centre = IVec3::new(pos.x.floor() as i32, feet_y, pos.z.floor() as i32);
    let foothold = |c: IVec3| supported_foothold(c, params, support, solid);
    // A mob standing ON a partial-collision block (a bed, stair, slab, model
    // block) has its feet inside that block's own cell — the real foothold is
    // then the cell above it (floor = that block, body space clear), checked
    // FIRST: if the cell under the feet can bear this body, the mob is resting
    // on that shape, not standing beside it. Without this the standing probe
    // fails (or claims the in-shape cell) and the mob goes goalless the moment
    // it steps onto a bed.
    let foothold_at = |c: IVec3| -> Option<IVec3> {
        if support(c) && foothold(c + IVec3::Y) {
            return Some(c + IVec3::Y);
        }
        if foothold(c) {
            return Some(c);
        }
        None
    };
    if let Some(c) = foothold_at(centre) {
        return Some(c);
    }
    // The centre overhangs — pick the footprint-corner foothold nearest the centre.
    let mut best: Option<(IVec3, f32)> = None;
    for sx in [-half_width, half_width] {
        for sz in [-half_width, half_width] {
            let corner = IVec3::new(
                (pos.x + f64::from(sx)).floor() as i32,
                feet_y,
                (pos.z + f64::from(sz)).floor() as i32,
            );
            if corner == centre {
                continue;
            }
            let Some(c) = foothold_at(corner) else {
                continue;
            };
            let (dx, dz) = (
                (f64::from(c.x) + 0.5 - pos.x) as f32,
                (f64::from(c.z) + 0.5 - pos.z) as f32,
            );
            let dist = dx * dx + dz * dz;
            if best.is_none_or(|(_, bd)| dist < bd) {
                best = Some((c, dist));
            }
        }
    }
    best.map(|(c, _)| c)
}

/// Find the fluid-surface navigation cell near a swimming mob. This deliberately
/// searches upward from the feet: submerged cells are not path waypoints, but the
/// surface just above them can be.
#[cfg(test)]
fn swimming_cell(
    pos: petramond_math::world_pos::WorldPos,
    half_width: f32,
    head: i32,
    solid: &impl Fn(IVec3) -> bool,
    fluid: &impl Fn(IVec3) -> bool,
) -> Option<IVec3> {
    swimming_cell_with(pos, half_width, head, solid, solid, fluid)
}

/// Swimming-cell resolution with the separate `support` predicate.
pub fn swimming_cell_with(
    pos: petramond_math::world_pos::WorldPos,
    half_width: f32,
    head: i32,
    solid: &impl Fn(IVec3) -> bool,
    support: &impl Fn(IVec3) -> bool,
    fluid: &impl Fn(IVec3) -> bool,
) -> Option<IVec3> {
    let params = PathParams::for_body(head, half_width);
    let feet_y = pos.y.floor() as i32;
    let x = pos.x.floor() as i32;
    let z = pos.z.floor() as i32;
    for dy in 0..=params.head_cells() + 4 {
        let c = IVec3::new(x, feet_y + dy, z);
        if is_navigation_foothold_with(c, params, solid, support, fluid) {
            return Some(c);
        }
    }
    None
}

/// The cell a mob paths from: its standing foothold on dry ground, or — while its
/// body is in fluid — the fluid-surface navigation cell above its feet. The two must
/// not be conflated: in one-deep fluid the solid bed sits directly below the feet, so
/// the solid-only standing probe would claim the *submerged* feet cell, which
/// [`find_path_nav`] rejects as a start (fluid cells are passable, not standable) —
/// leaving the mob goalless, bobbing in place forever.
#[cfg(test)]
fn navigation_cell(
    pos: petramond_math::world_pos::WorldPos,
    half_width: f32,
    head: i32,
    in_fluid: bool,
    solid: &impl Fn(IVec3) -> bool,
    fluid: &impl Fn(IVec3) -> bool,
) -> Option<IVec3> {
    navigation_cell_with(pos, half_width, head, in_fluid, solid, solid, fluid)
}

/// Navigation-cell resolution with the separate `support` predicate.
pub fn navigation_cell_with(
    pos: petramond_math::world_pos::WorldPos,
    half_width: f32,
    head: i32,
    in_fluid: bool,
    solid: &impl Fn(IVec3) -> bool,
    support: &impl Fn(IVec3) -> bool,
    fluid: &impl Fn(IVec3) -> bool,
) -> Option<IVec3> {
    if in_fluid {
        swimming_cell_with(pos, half_width, head, solid, support, fluid)
    } else {
        standing_cell_with(pos, half_width, head, solid, support)
    }
}

/// Keep exact boundary contact from claiming the neighbouring cell. Collision uses
/// strict overlap too, so a half-width of exactly 0.5 still fits one cell wide.
const FOOTPRINT_EPS: f32 = 1e-4;

fn footprint_range(half_width: f32) -> std::ops::RangeInclusive<i32> {
    let hw = half_width.max(0.0);
    let min = (0.5 - hw + FOOTPRINT_EPS).floor() as i32;
    let max = (0.5 + hw - FOOTPRINT_EPS).floor() as i32;
    min..=max
}

pub fn body_layer_touches(
    cell: IVec3,
    params: PathParams,
    occupied: &impl Fn(IVec3) -> bool,
) -> bool {
    let range = footprint_range(params.body_half_width());
    for dx in range.clone() {
        for dz in range.clone() {
            if occupied(cell + IVec3::new(dx, 0, dz)) {
                return true;
            }
        }
    }
    false
}

pub fn body_layer_clear(cell: IVec3, params: PathParams, solid: &impl Fn(IVec3) -> bool) -> bool {
    !body_layer_touches(cell, params, solid)
}

/// True when every cell occupied by the mob's body footprint from `cell` upward is
/// clear of the supplied occupancy predicate.
pub fn body_clear(cell: IVec3, params: PathParams, occupied: &impl Fn(IVec3) -> bool) -> bool {
    (0..params.head_cells()).all(|dy| body_layer_clear(cell + IVec3::Y * dy, params, occupied))
}

/// True when the occupied predicate touches the body footprint or the floor/support
/// footprint under it. Useful for classifying "wet" destinations: a fluid-surface
/// waypoint has dry body clearance but fluid under the feet.
pub fn body_or_floor_touches(
    cell: IVec3,
    params: PathParams,
    occupied: &impl Fn(IVec3) -> bool,
) -> bool {
    body_layer_touches(cell - IVec3::Y, params, occupied)
        || (0..params.head_cells())
            .any(|dy| body_layer_touches(cell + IVec3::Y * dy, params, occupied))
}

fn footprint_supported(cell: IVec3, params: PathParams, support: &impl Fn(IVec3) -> bool) -> bool {
    let floor = cell - IVec3::Y;
    let range = footprint_range(params.body_half_width());
    for dx in range.clone() {
        for dz in range.clone() {
            if !support(floor + IVec3::new(dx, 0, dz)) {
                return false;
            }
        }
    }
    true
}

fn supported_foothold(
    cell: IVec3,
    params: PathParams,
    support: &impl Fn(IVec3) -> bool,
    solid: &impl Fn(IVec3) -> bool,
) -> bool {
    footprint_supported(cell, params, support) && body_clear(cell, params, solid)
}

/// Find a walkable path of foothold cells from `start` toward `goal`.
///
/// Returns the cells to walk, beginning with `start`. Reaching `goal` returns the
/// full route; if `goal` can't be reached (walled off, or not itself a foothold)
/// the path leads to the reachable cell with the smallest remaining distance to
/// `goal`. An empty `Vec` means `start` isn't a foothold (the mob isn't standing on
/// anything — the caller should just let physics settle it first).
///
/// `fluid(cell)` marks fluid. Fluid counts as **footing** (a mob swims across the
/// surface), so a route may cross a body of fluid of any depth — the kinematics float
/// the mob up while it does. Avoiding fluid is a *destination* preference (see the
/// wander behavior), not a routing constraint: the shortest path still cuts across.
#[cfg(test)]
pub(super) fn find_path(
    start: IVec3,
    goal: IVec3,
    params: PathParams,
    solid: impl Fn(IVec3) -> bool,
    fluid: impl Fn(IVec3) -> bool,
) -> Vec<IVec3> {
    find_path_nav(
        start,
        goal,
        params,
        &solid,
        &solid,
        fluid,
        |_, _| true,
        |_| 0,
    )
}

/// The full production navigation search. In addition to the basic world
/// predicates it takes:
/// - `support(cell)` — cells that can bear feet even when not fully `solid`
///   (partial-collision shapes), see [`is_navigation_foothold_with`];
/// - `step_allowed(from, to)` — the accurate edge gate: rejects a specific
///   transition between two accepted footholds when the real body AABB cannot
///   sweep that move through the real collision boxes (partial shapes, doors);
/// - `cell_cost(cell)` — a soft per-cell surcharge added when an edge ENTERS
///   that cell. Soft obstacles (other entities) get routed around when a
///   detour exists, yet never wall off the only route. Costs are in the same
///   scale as the step costs ([`COST_FLAT`] = 10 per cell); the heuristic
///   ignores them, so they only ever ADD cost and A* stays admissible.
#[allow(clippy::too_many_arguments)]
pub fn find_path_nav(
    start: IVec3,
    goal: IVec3,
    params: PathParams,
    solid: &impl Fn(IVec3) -> bool,
    support: &impl Fn(IVec3) -> bool,
    fluid: impl Fn(IVec3) -> bool,
    step_allowed: impl Fn(IVec3, IVec3) -> bool,
    cell_cost: impl Fn(IVec3) -> u32,
) -> Vec<IVec3> {
    let passable_col = |c: IVec3| body_clear(c, params, solid);
    // A cell is a foothold if its floor *supports* it (solid ground, a partial
    // shape's top, or the fluid surface) and the body fits above. Submerged
    // fluid cells are passable, not footholds.
    let memo = CellMemo::<2048>::default();
    let foothold = |c: IVec3| {
        memo.get(c, |c| {
            is_navigation_foothold_with(c, params, solid, support, &fluid)
        })
    };

    if !foothold(start) {
        return Vec::new();
    }
    if start == goal {
        return vec![start];
    }

    // Octile distance: the cost of the cheapest diagonal-then-straight route over
    // flat ground, ignoring height (vertical moves cost ≥ COST_FLAT, so this stays
    // admissible). Manhattan would over-estimate now that diagonals exist.
    let h = |c: IVec3| -> u32 {
        let dx = (c.x - goal.x).unsigned_abs();
        let dz = (c.z - goal.z).unsigned_abs();
        let (lo, hi) = if dx < dz { (dx, dz) } else { (dz, dx) };
        COST_DIAG * lo + COST_FLAT * (hi - lo)
    };

    let mut g_score: FxHashMap<IVec3, u32> = FxHashMap::default();
    let mut came_from: FxHashMap<IVec3, IVec3> = FxHashMap::default();
    let mut open: BinaryHeap<Reverse<(u32, u32, [i32; 3])>> = BinaryHeap::new();

    g_score.insert(start, 0);
    open.push(Reverse((h(start), 0, start.to_array())));

    // Best cell seen so far by heuristic, for the closest-reachable fallback.
    let mut best = start;
    let mut best_h = h(start);
    let mut expanded = 0usize;
    let mut steps: Vec<(IVec3, u32)> = Vec::with_capacity(8);

    while let Some(Reverse((_, g_at_pop, pos_arr))) = open.pop() {
        let current = IVec3::from_array(pos_arr);
        // Skip stale heap entries (a cheaper path to `current` was found after this
        // entry was queued).
        if g_at_pop > *g_score.get(&current).unwrap_or(&u32::MAX) {
            continue;
        }
        if current == goal {
            return reconstruct(&came_from, current);
        }
        let hc = h(current);
        if hc < best_h {
            best_h = hc;
            best = current;
        }

        expanded += 1;
        if expanded >= params.max_nodes {
            break;
        }

        neighbors(
            current,
            &params,
            &foothold,
            &passable_col,
            solid,
            &step_allowed,
            &mut steps,
        );
        for &(next, step_cost) in &steps {
            let tentative = g_score[&current]
                .saturating_add(step_cost)
                .saturating_add(cell_cost(next));
            if tentative < *g_score.get(&next).unwrap_or(&u32::MAX) {
                came_from.insert(next, current);
                g_score.insert(next, tentative);
                open.push(Reverse((tentative + h(next), tentative, next.to_array())));
            }
        }
    }

    // Goal unreachable within the budget: walk toward the closest cell we found.
    reconstruct(&came_from, best)
}

/// The walkable neighbours of foothold `a`: for each cardinal direction, exactly one
/// of step-flat / jump-up-one / descend (first ground within `max_drop`), or nothing
/// if that direction is blocked.
#[allow(clippy::too_many_arguments)]
fn neighbors(
    a: IVec3,
    params: &PathParams,
    foothold: &impl Fn(IVec3) -> bool,
    passable_col: &impl Fn(IVec3) -> bool,
    solid: &impl Fn(IVec3) -> bool,
    step_allowed: &impl Fn(IVec3, IVec3) -> bool,
    out: &mut Vec<(IVec3, u32)>,
) {
    const DIRS: [(i32, i32); 4] = [(1, 0), (-1, 0), (0, 1), (0, -1)];
    out.clear();
    for (dx, dz) in DIRS {
        let side = a + IVec3::new(dx, 0, dz);

        // Climb: the higher cell is a foothold and every layer the head rises
        // through above the start is clear. One move per direction; a climb
        // wins when it exists.
        let up = side + IVec3::Y * CLIMB_CELLS;
        if foothold(up)
            && step_allowed(a, up)
            && (0..CLIMB_CELLS).all(|rise| {
                body_layer_clear(a + IVec3::Y * (params.head_cells() + rise), *params, solid)
            })
        {
            out.push((up, COST_JUMP));
            continue;
        }

        // Flat step: the neighbour at the same level is a foothold.
        if foothold(side) && step_allowed(a, side) {
            out.push((side, COST_FLAT));
            continue;
        }

        // Descend: step into `side` (body must fit) and fall to the first foothold
        // within `max_drop`. A solid cell in the fall column blocks the descent;
        // running past `max_drop` means it's a cliff (no move that direction).
        if passable_col(side) {
            for dy in 1..=params.max_drop {
                let c = side - IVec3::Y * dy;
                if solid(c) {
                    break; // hit a wall/ground that isn't cleanly standable-into
                }
                if foothold(c) && step_allowed(a, c) {
                    out.push((c, COST_FLAT + dy as u32 * COST_DROP_PER_BLOCK));
                    break;
                }
            }
        }
    }

    // Flat diagonals: taken only across a fully-flat 2×2 of footholds — the diagonal
    // target AND both orthogonal neighbours are footholds at this level. That forbids
    // cutting an obstacle's corner or slicing over a gap, and keeps jumps/falls
    // cardinal, yet lets a mob take the short straight-ish route over open ground.
    const DIAGS: [(i32, i32); 4] = [(1, 1), (1, -1), (-1, 1), (-1, -1)];
    for (dx, dz) in DIAGS {
        let target = a + IVec3::new(dx, 0, dz);
        let o1 = a + IVec3::new(dx, 0, 0);
        let o2 = a + IVec3::new(0, 0, dz);
        if foothold(target) && foothold(o1) && foothold(o2) && step_allowed(a, target) {
            out.push((target, COST_DIAG));
        }
    }
}

/// Walk `came_from` back from `end` to the start and return the cells in
/// start→end order.
fn reconstruct(came_from: &FxHashMap<IVec3, IVec3>, end: IVec3) -> Vec<IVec3> {
    let mut path = vec![end];
    let mut node = end;
    while let Some(&prev) = came_from.get(&node) {
        path.push(prev);
        node = prev;
    }
    path.reverse();
    path
}

#[cfg(test)]
mod tests;
