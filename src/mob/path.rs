//! Grid pathfinding for walking mobs: A* over **footholds** (cells a mob can
//! stand in), with movement rules that match how a mob actually moves —
//! step flat, jump up exactly one block, or walk off a ledge and fall up to a
//! capped height. No move climbs more than one block; no descent exceeds
//! [`PathParams::max_drop`].
//!
//! Pure and world-agnostic: the search takes a `solid(cell)` closure (does this
//! cell block movement?) plus the mob's headroom, so it is fully unit-testable
//! against a stub world. [`find_path`] returns the foothold cells from the start
//! toward the goal; if the goal is unreachable it returns the path to the reachable
//! cell that gets **closest** to the goal (a best-effort partial path), so a mob
//! always makes progress instead of standing still.

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap};

use crate::mathh::{IVec3, Vec3};

/// Cost of a flat (same-level) step. Costs are integers (scaled ×10 of "one cell")
/// so the open set can order on a total `Ord` without floats.
const COST_FLAT: u32 = 10;
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

/// Tuning for [`find_path`]. `head` is the mob's vertical clearance in whole cells
/// (how many cells above the floor its body needs); `max_drop` caps how far a
/// descent may fall; `max_nodes` bounds the search so one pathfind can't stall the
/// tick.
#[derive(Copy, Clone, Debug)]
pub struct PathParams {
    pub head: i32,
    pub max_drop: i32,
    pub max_nodes: usize,
}

impl Default for PathParams {
    fn default() -> Self {
        PathParams {
            head: 1,
            max_drop: 4,
            max_nodes: 4000,
        }
    }
}

/// Is `cell` a foothold — a cell a mob can stand in? Its floor (the cell below)
/// blocks movement, and the `head` cells from `cell` upward are clear for the body.
/// Shared by the pathfinder, the navigator, and wander destination picking so they
/// all agree on what "standable" means.
pub fn is_foothold(cell: IVec3, head: i32, solid: &impl Fn(IVec3) -> bool) -> bool {
    solid(cell - IVec3::Y) && (0..head).all(|i| !solid(cell + IVec3::Y * i))
}

/// Find the foothold cell a mob is standing in, given its feet position `pos` and
/// footprint `half_width`. Prefers the cell under the mob's centre; if that centre
/// overhangs an edge (its floor is air) it falls back to the foothold under a
/// footprint corner nearest the centre — the block the mob is actually resting on.
/// `None` if the mob is over no foothold (e.g. mid-air).
///
/// Without this, a mob standing at a block edge — centre over the drop, body still on
/// the block — would have a non-foothold "current cell", so [`find_path`] would bail
/// and it would never path anywhere (it'd freeze at the edge).
pub fn standing_cell(
    pos: Vec3,
    half_width: f32,
    head: i32,
    solid: &impl Fn(IVec3) -> bool,
) -> Option<IVec3> {
    let feet_y = pos.y.floor() as i32;
    let centre = IVec3::new(pos.x.floor() as i32, feet_y, pos.z.floor() as i32);
    if is_foothold(centre, head, solid) {
        return Some(centre);
    }
    // The centre overhangs — pick the footprint-corner foothold nearest the centre.
    let mut best: Option<(IVec3, f32)> = None;
    for sx in [-half_width, half_width] {
        for sz in [-half_width, half_width] {
            let c = IVec3::new((pos.x + sx).floor() as i32, feet_y, (pos.z + sz).floor() as i32);
            if c == centre || !is_foothold(c, head, solid) {
                continue;
            }
            let (dx, dz) = (c.x as f32 + 0.5 - pos.x, c.z as f32 + 0.5 - pos.z);
            let dist = dx * dx + dz * dz;
            if best.map_or(true, |(_, bd)| dist < bd) {
                best = Some((c, dist));
            }
        }
    }
    best.map(|(c, _)| c)
}

/// Find a walkable path of foothold cells from `start` toward `goal`.
///
/// Returns the cells to walk, beginning with `start`. Reaching `goal` returns the
/// full route; if `goal` can't be reached (walled off, or not itself a foothold)
/// the path leads to the reachable cell with the smallest remaining distance to
/// `goal`. An empty `Vec` means `start` isn't a foothold (the mob isn't standing on
/// anything — the caller should just let physics settle it first).
///
/// `water(cell)` marks water. Water counts as **footing** (a mob swims across the
/// surface), so a route may cross a body of water of any depth — the kinematics float
/// the mob up while it does. Avoiding water is a *destination* preference (see the
/// wander behavior), not a routing constraint: the shortest path still cuts across.
pub fn find_path(
    start: IVec3,
    goal: IVec3,
    params: PathParams,
    solid: impl Fn(IVec3) -> bool,
    water: impl Fn(IVec3) -> bool,
) -> Vec<IVec3> {
    let passable_col = |c: IVec3| (0..params.head).all(|i| !solid(c + IVec3::Y * i));
    // A cell is a foothold if its floor *supports* it (solid ground or water to swim
    // on) and the body fits above. Floor support widens to include water; clearance
    // stays solid-only so a head submerged in water doesn't block standing.
    let foothold = |c: IVec3| {
        let floor = c - IVec3::Y;
        (solid(floor) || water(floor)) && (0..params.head).all(|i| !solid(c + IVec3::Y * i))
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

    let mut g_score: HashMap<IVec3, u32> = HashMap::new();
    let mut came_from: HashMap<IVec3, IVec3> = HashMap::new();
    let mut open: BinaryHeap<Reverse<(u32, u32, [i32; 3])>> = BinaryHeap::new();

    g_score.insert(start, 0);
    open.push(Reverse((h(start), 0, start.to_array())));

    // Best cell seen so far by heuristic, for the closest-reachable fallback.
    let mut best = start;
    let mut best_h = h(start);
    let mut expanded = 0usize;

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

        for (next, step_cost) in neighbors(current, &params, &foothold, &passable_col, &solid) {
            let tentative = g_score[&current].saturating_add(step_cost);
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
fn neighbors(
    a: IVec3,
    params: &PathParams,
    foothold: &impl Fn(IVec3) -> bool,
    passable_col: &impl Fn(IVec3) -> bool,
    solid: &impl Fn(IVec3) -> bool,
) -> Vec<(IVec3, u32)> {
    const DIRS: [(i32, i32); 4] = [(1, 0), (-1, 0), (0, 1), (0, -1)];
    let mut out = Vec::with_capacity(4);
    for (dx, dz) in DIRS {
        let side = a + IVec3::new(dx, 0, dz);

        // Jump up one block: the higher cell is a foothold and there's clearance
        // above the mob's head at the start to rise. (If the higher cell is a
        // foothold, `side` itself is solid, so a flat step is impossible anyway.)
        let up = side + IVec3::Y;
        if foothold(up) && !solid(a + IVec3::Y * params.head) {
            out.push((up, COST_JUMP));
            continue;
        }

        // Flat step: the neighbour at the same level is a foothold.
        if foothold(side) {
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
                if foothold(c) {
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
        if foothold(target) && foothold(o1) && foothold(o2) {
            out.push((target, COST_DIAG));
        }
    }
    out
}

/// Walk `came_from` back from `end` to the start and return the cells in
/// start→end order.
fn reconstruct(came_from: &HashMap<IVec3, IVec3>, end: IVec3) -> Vec<IVec3> {
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
mod tests {
    use super::*;

    /// A solid world from a predicate, with a floor plane at `y < floor_y` always
    /// solid so footholds exist. Extra solid cells are added via the set.
    struct Stub {
        floor_y: i32,
        solid: std::collections::HashSet<(i32, i32, i32)>,
    }
    impl Stub {
        fn new(floor_y: i32) -> Self {
            Stub {
                floor_y,
                solid: std::collections::HashSet::new(),
            }
        }
        fn add(&mut self, c: IVec3) {
            self.solid.insert((c.x, c.y, c.z));
        }
        fn solid_at(&self, c: IVec3) -> bool {
            c.y < self.floor_y || self.solid.contains(&(c.x, c.y, c.z))
        }
    }

    fn params() -> PathParams {
        PathParams::default()
    }

    #[test]
    fn flat_path_on_open_ground_is_a_straight_manhattan_run() {
        // Floor at y<1, so footholds are at y==1 everywhere.
        let w = Stub::new(1);
        let start = IVec3::new(0, 1, 0);
        let goal = IVec3::new(3, 1, 0);
        let path = find_path(start, goal, params(), |c| w.solid_at(c), |_| false);
        assert_eq!(path.first(), Some(&start));
        assert_eq!(path.last(), Some(&goal));
        // Straight line along +X (goal shares the start's Z): 3 steps, no diagonals.
        assert_eq!(path.len(), 4);
        assert!(path.iter().all(|c| c.y == 1), "stays on the flat floor");
    }

    #[test]
    fn takes_diagonals_over_open_ground() {
        let w = Stub::new(1);
        let start = IVec3::new(0, 1, 0);
        let goal = IVec3::new(3, 1, 3);
        let path = find_path(start, goal, params(), |c| w.solid_at(c), |_| false);
        assert_eq!(path.first(), Some(&start));
        assert_eq!(path.last(), Some(&goal));
        // 3 diagonal steps (4 cells), not the 6-step cardinal staircase.
        assert_eq!(path.len(), 4, "open ground should route diagonally: {path:?}");
        assert!(
            path.windows(2)
                .any(|w| (w[1].x - w[0].x).abs() == 1 && (w[1].z - w[0].z).abs() == 1),
            "expected a diagonal step: {path:?}"
        );
    }

    #[test]
    fn does_not_cut_an_obstacle_corner() {
        // A 2-high pillar at (1, _, 0). Going (0,1,0) -> (1,1,1) must route orthogonally
        // around the corner, never slicing the diagonal across it.
        let mut w = Stub::new(1);
        w.add(IVec3::new(1, 1, 0));
        w.add(IVec3::new(1, 2, 0));
        let start = IVec3::new(0, 1, 0);
        let goal = IVec3::new(1, 1, 1);
        let path = find_path(start, goal, params(), |c| w.solid_at(c), |_| false);
        assert_eq!(path.last(), Some(&goal));
        assert!(
            !path.windows(2).any(|w| w[0] == start && w[1] == goal),
            "must not cut the obstacle corner: {path:?}"
        );
        // Any diagonal it does take has both orthogonal cells clear.
        for w2 in path.windows(2) {
            let (a, b) = (w2[0], w2[1]);
            if (b.x - a.x).abs() == 1 && (b.z - a.z).abs() == 1 {
                assert!(!w.solid_at(IVec3::new(b.x, a.y, a.z)), "corner cut via X");
                assert!(!w.solid_at(IVec3::new(a.x, a.y, b.z)), "corner cut via Z");
            }
        }
    }

    #[test]
    fn steps_up_a_one_block_rise() {
        // Floor at y<1. Raise a 1-block step at x>=2 (top surface y==2 there).
        let mut w = Stub::new(1);
        for x in 2..=4 {
            w.add(IVec3::new(x, 1, 0)); // a block at y=1 -> foothold on top at y=2
        }
        let start = IVec3::new(0, 1, 0);
        let goal = IVec3::new(4, 2, 0);
        let path = find_path(start, goal, params(), |c| w.solid_at(c), |_| false);
        assert_eq!(path.last(), Some(&goal), "reaches the raised goal");
        // The path must include a +1 transition (a jump), never a +2.
        for w2 in path.windows(2) {
            assert!(w2[1].y - w2[0].y <= 1, "no move climbs more than one block");
        }
        assert!(path.iter().any(|c| c.y == 2), "climbs onto the step");
    }

    #[test]
    fn a_two_high_wall_cannot_be_climbed() {
        // A 2-tall wall at x==2 spanning ALL z (so there's no detour around it); with
        // head=1 the only crossing would be a 2-block climb, which isn't allowed.
        let solid = |c: IVec3| c.y < 1 || (c.x == 2 && (c.y == 1 || c.y == 2));
        let start = IVec3::new(0, 1, 0);
        let goal = IVec3::new(4, 1, 0);
        let path = find_path(start, goal, params(), solid, |_| false);
        // Unreachable: best-effort path stops on the near side (x < 2), never crosses.
        assert!(path.last().unwrap().x < 2, "must not climb a 2-high wall: {:?}", path.last());
    }

    #[test]
    fn descends_a_ledge_within_the_drop_limit() {
        // High floor for x<=1 (top y==4), low floor for x>=2 (top y==1): a 3-block drop.
        let mut w = Stub::new(1);
        for x in -1..=1 {
            for y in 1..4 {
                w.add(IVec3::new(x, y, 0));
            }
        }
        let start = IVec3::new(0, 4, 0);
        let goal = IVec3::new(3, 1, 0);
        let path = find_path(start, goal, params(), |c| w.solid_at(c), |_| false);
        assert_eq!(path.last(), Some(&goal), "walks off the ledge to the low ground");
        // The drop happens in one move (walk off), descending 3.
        assert!(path.windows(2).any(|w2| w2[0].y - w2[1].y == 3), "single 3-block descent: {path:?}");
    }

    #[test]
    fn avoids_a_drop_greater_than_the_limit() {
        // High floor (top y==6) for x<2; a deep pit (top y==1) for x>=2 — a 5-block
        // cliff, > max_drop 4, so the near edge is a dead end downward.
        let solid = |c: IVec3| if c.x >= 2 { c.y < 1 } else { c.y < 6 };
        let start = IVec3::new(0, 6, 0);
        let goal = IVec3::new(3, 1, 0);
        let path = find_path(start, goal, params(), solid, |_| false);
        // 6 - 1 = 5 block drop > 4: must NOT step off into the pit.
        assert!(path.last().unwrap().x < 2, "must not take a >4 drop: {:?}", path.last());
    }

    /// Two land platforms (top y==0 → foothold y==1) split by a deep trench at
    /// x in 1..=3 whose bed sits at y<=-6 — a 6-block drop, deeper than `max_drop`, so
    /// it can't be walked down-and-up dry.
    fn deep_trench_solid(c: IVec3) -> bool {
        if (1..=3).contains(&c.x) {
            c.y <= -6 // bed far below the surface
        } else {
            c.y <= 0 // land top at y==0
        }
    }

    #[test]
    fn crosses_deep_water_at_the_surface() {
        // Fill the trench with water up to the surface (y=-5..=0). Water counts as
        // footing, so the surface foothold at y==1 is continuous and the route is a
        // straight flat run across — depth (well over one block) is no wall.
        let water = |c: IVec3| (1..=3).contains(&c.x) && (-5..=0).contains(&c.y);
        let start = IVec3::new(0, 1, 0);
        let goal = IVec3::new(4, 1, 0);
        let path = find_path(start, goal, params(), deep_trench_solid, water);
        assert_eq!(path.first(), Some(&start));
        assert_eq!(path.last(), Some(&goal), "reaches the far shore across the water");
        assert!(path.iter().all(|c| c.y == 1), "crosses at the surface level: {path:?}");
    }

    #[test]
    fn the_same_trench_dry_is_an_uncrossable_gap() {
        // The same geometry with no water: the 6-deep drop exceeds max_drop and there's
        // no surface footing, so the far shore is unreachable — confirming it's the
        // water-as-footing rule that makes the crossing, not the geometry.
        let start = IVec3::new(0, 1, 0);
        let goal = IVec3::new(4, 1, 0);
        let path = find_path(start, goal, params(), deep_trench_solid, |_| false);
        assert!(path.last().unwrap().x < 1, "no footing over the dry gap: {:?}", path.last());
    }

    #[test]
    fn unreachable_goal_returns_closest_partial_path() {
        // Goal floats in the air (no foothold) far away; expect a partial path that
        // heads toward it and stops on solid ground.
        let w = Stub::new(1);
        let start = IVec3::new(0, 1, 0);
        let goal = IVec3::new(20, 50, 0); // unreachable (in the sky)
        let path = find_path(start, goal, params(), |c| w.solid_at(c), |_| false);
        assert_eq!(path.first(), Some(&start));
        assert!(path.len() > 1, "makes progress toward the goal");
        // Ends on a foothold heading the right way (+X), never reaching the sky goal.
        assert!(path.last().unwrap().x > 0);
        assert!(path.last().unwrap().y < 50);
    }

    #[test]
    fn standing_cell_finds_the_block_under_an_overhanging_centre() {
        // One block at (0,0,0), top surface at y=1.
        let solid = |c: IVec3| c == IVec3::new(0, 0, 0);
        // Centre just past the +X edge (cell (1,1,0) overhangs air), but the footprint
        // still rests on the block -> returns that block's foothold (0,1,0).
        let cell = standing_cell(Vec3::new(1.1, 1.0, 0.5), 0.25, 1, &solid);
        assert_eq!(cell, Some(IVec3::new(0, 1, 0)), "edge overhang resolves to the block");
        // Centre squarely on the block -> the centre cell.
        let on = standing_cell(Vec3::new(0.5, 1.0, 0.5), 0.25, 1, &solid);
        assert_eq!(on, Some(IVec3::new(0, 1, 0)));
        // Over nothing (mid-air) -> None.
        let off = standing_cell(Vec3::new(5.0, 1.0, 5.0), 0.25, 1, &solid);
        assert_eq!(off, None);
    }

    #[test]
    fn start_equals_goal_is_a_singleton() {
        let w = Stub::new(1);
        let c = IVec3::new(5, 1, 5);
        assert_eq!(find_path(c, c, params(), |p| w.solid_at(p), |_| false), vec![c]);
    }

    #[test]
    fn non_foothold_start_returns_empty() {
        // Start floating with no floor below: not a foothold.
        let w = Stub::new(1);
        let floating = IVec3::new(0, 10, 0);
        assert!(find_path(floating, IVec3::new(1, 1, 0), params(), |c| w.solid_at(c), |_| false).is_empty());
    }

    #[test]
    fn routes_around_an_obstacle() {
        // A wall with a one-cell gap forces a detour; the path must reach the goal
        // and avoid the solid wall cells.
        let mut w = Stub::new(1);
        for z in -2..=2 {
            if z != 2 {
                w.add(IVec3::new(2, 1, z)); // wall at x=2 except a gap at z=2
                w.add(IVec3::new(2, 2, z)); // 2 high so it can't be jumped
            }
        }
        let start = IVec3::new(0, 1, 0);
        let goal = IVec3::new(4, 1, 0);
        let path = find_path(start, goal, params(), |c| w.solid_at(c), |_| false);
        assert_eq!(path.last(), Some(&goal), "finds the gap and reaches the goal");
        assert!(
            path.iter().all(|c| !w.solid_at(*c)),
            "path never enters a solid cell"
        );
    }
}
