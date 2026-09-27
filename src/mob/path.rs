use rustc_hash::FxHashMap;

use petramond_math::math::IVec3;
use petramond_world::block::Block;

mod box_memo;
mod reach;
mod search;

pub use box_memo::{fits_a_table, BoxFacts, BoxLeads, Fact};
pub use reach::{reachable_nav, reachable_on, walk_region, BoxGraph, NavWorld, Planned, Reads};
pub use search::{NavSearch, SearchPoll, SearchProbes};

pub const CLIMB_CELLS: i32 = 1;

pub(crate) const COST_FLAT: u32 = 10;
const COST_JUMP: u32 = 14;
const COST_DIAG: u32 = 14;
const COST_DROP_PER_BLOCK: u32 = 1;

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

pub struct CellMemo<const N: usize> {
    key: [std::cell::Cell<IVec3>; N],
    val: [std::cell::Cell<bool>; N],
}

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

pub trait CellCache {
    fn get(&self, c: IVec3, compute: impl FnOnce(IVec3) -> bool) -> bool;
}

impl<const N: usize> CellCache for CellMemo<N> {
    #[inline]
    fn get(&self, c: IVec3, compute: impl FnOnce(IVec3) -> bool) -> bool {
        CellMemo::get(self, c, compute)
    }
}

pub fn is_foothold(cell: IVec3, params: PathParams, solid: &impl Fn(IVec3) -> bool) -> bool {
    supported_foothold(cell, params, solid, solid)
}

#[cfg(test)]
fn is_navigation_foothold(
    cell: IVec3,
    params: PathParams,
    solid: &impl Fn(IVec3) -> bool,
    fluid: &impl Fn(IVec3) -> bool,
) -> bool {
    is_navigation_foothold_with(cell, params, solid, solid, fluid)
}

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

#[cfg(test)]
fn standing_cell(
    pos: petramond_math::world_pos::WorldPos,
    half_width: f32,
    head: i32,
    solid: &impl Fn(IVec3) -> bool,
) -> Option<IVec3> {
    standing_cell_with(pos, half_width, head, solid, solid)
}

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

/// The cell a mob paths from: standing foothold on dry ground, or the fluid-surface cell above the
/// feet while its body is in fluid.
/// Don't conflate the two. In one-deep fluid the solid bed is right under the feet, so a solid-only
/// probe grabs the submerged feet cell instead. [`find_path_nav`] rejects fluid cells as a start,
/// so the mob ends up goalless, bobbing in place forever.
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

pub fn body_clear(cell: IVec3, params: PathParams, occupied: &impl Fn(IVec3) -> bool) -> bool {
    (0..params.head_cells()).all(|dy| body_layer_clear(cell + IVec3::Y * dy, params, occupied))
}

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

/// Full production nav search. Extra params over the basic world predicates:
/// - `support(cell)`: cells that can bear feet even if not fully `solid` (partial shapes), see
///   [`is_navigation_foothold_with`].
/// - `step_allowed(from, to)`: edge gate, rejects a move the real body AABB can't sweep through
///   (partial shapes, doors).
/// - `cell_cost(cell)`: soft surcharge for entering a cell. Routes around other entities but never
///   blocks the only path. Same scale as step costs ([`COST_FLAT`] = 10 per cell); the heuristic
///   ignores it, so A* stays admissible.
///
/// One-shot wrapper over [`NavSearch`], which the navigator drives directly so a search can span
/// ticks under the path budget.
#[cfg(any(test, feature = "test-support"))]
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
    NavSearch::default()
        .solve(
            start,
            goal,
            params,
            &SearchProbes {
                solid,
                support,
                fluid: &fluid,
                step_allowed: &step_allowed,
                cell_cost: &cell_cost,
            },
        )
        .0
}

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

        if foothold(side) && step_allowed(a, side) {
            out.push((side, COST_FLAT));
            continue;
        }

        if passable_col(side) {
            for dy in 1..=params.max_drop {
                let c = side - IVec3::Y * dy;
                if solid(c) {
                    break;
                }
                if foothold(c) && step_allowed(a, c) {
                    out.push((c, COST_FLAT + dy as u32 * COST_DROP_PER_BLOCK));
                    break;
                }
            }
        }
    }

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
