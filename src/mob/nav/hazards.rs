//! Navigation hazards: `nav_hazard` blocks (and fluids) a body never walks
//! into from safety unless its species tolerates them. Checked per route edge
//! and again on the final walking wish; a body already touching one may
//! always move, and its route pays per hazardous cell, so it escapes by the
//! shortest way out.

use petramond_math::math::{IVec3, Vec3};
use petramond_world::block::{Block, BlockTag};
use petramond_world::fluid_math::fluid_height;

use crate::mob::path::{self, PathParams};
use crate::mob::{body_boxes, MobSize};
use crate::world::SectionCursor;

const EPS: f32 = 1e-4;

fn hazardous(block: Block, tolerated: &[Block]) -> bool {
    let hazard = |b: Block| b.has_tag(BlockTag::NAV_HAZARD) && !tolerated.contains(&b);
    hazard(block) || block.fluid().is_some_and(hazard)
}

/// Whether a body standing at foothold `cell` touches a hazard. A body on
/// fluid footing floats with its feet in the floor cell; otherwise the feet
/// rest on the floor's real top.
pub(in crate::mob) fn foothold_in_hazard(
    cur: &SectionCursor<'_>,
    cell: IVec3,
    params: PathParams,
) -> bool {
    let floor = cell - IVec3::Y;
    let feet_y = if cur.fluid_cell(floor) {
        floor.y as f32
    } else {
        floor.y as f32 + super::floor_top(cur, floor)
    };
    let half = Vec3::new(params.half_width, 0.0, params.half_width);
    let centre = Vec3::new(cell.x as f32 + 0.5, feet_y, cell.z as f32 + 0.5);
    let top = Vec3::Y * (cell.y + params.head_cells()) as f32;
    body_in_hazard(
        cur,
        centre - half,
        Vec3::new(centre.x, 0.0, centre.z) + half + top,
        params.tolerated,
    )
}

/// The hazard half of the per-edge navigation gate.
pub(super) fn step_gate<'c, 'w>(
    cur: &'c SectionCursor<'w>,
    params: PathParams,
) -> impl Fn(IVec3, IVec3) -> bool + use<'c, 'w> {
    let cells = path::CellMemo::<1024>::default();
    let footholds = path::CellMemo::<1024>::default();
    move |from, to| {
        // A body already in danger may traverse its pool to reach a shore.
        // After leaving, every subsequent edge must stay safe again.
        if footholds.get(from, |c| foothold_in_hazard(cur, c, params)) {
            return true;
        }
        let half = Vec3::new(params.half_width, 0.0, params.half_width);
        let offset = Vec3::new(0.5, 0.0, 0.5);
        let min = from.min(to).as_vec3() + offset - half - Vec3::Y;
        let max = from.max(to).as_vec3() + offset + half + Vec3::Y * params.head_cells() as f32;
        !any_cell(min, max, |c| {
            cells.get(c, |c| hazardous(cur.physics_block(c), params.tolerated))
        })
    }
}

fn any_cell(min: Vec3, max: Vec3, mut test: impl FnMut(IVec3) -> bool) -> bool {
    let lo = (min + Vec3::splat(EPS)).floor().as_ivec3();
    let hi = (max - Vec3::splat(EPS)).floor().as_ivec3();
    for y in lo.y..=hi.y {
        for z in lo.z..=hi.z {
            for x in lo.x..=hi.x {
                if test(IVec3::new(x, y, z)) {
                    return true;
                }
            }
        }
    }
    false
}

/// Whether a body box touches a hazard: resting on one counts, and a fluid
/// only counts below its real surface.
fn body_in_hazard(cur: &SectionCursor<'_>, min: Vec3, max: Vec3, tolerated: &[Block]) -> bool {
    any_cell(min - Vec3::Y * 2.0 * EPS, max, |c| {
        let block = cur.physics_block(c);
        if !hazardous(block, tolerated) {
            return false;
        }
        let Some(fluid) = block.fluid() else {
            return true;
        };
        let surface =
            c.y as f32 + fluid_height(cur.fluid_meta(c), cur.physics_block(c + IVec3::Y), fluid);
        min.y + EPS < surface
    })
}

fn horizontal_overlap(min: Vec3, max: Vec3, cell: IVec3) -> f32 {
    let x = (max.x.min(cell.x as f32 + 1.0) - min.x.max(cell.x as f32)).max(0.0);
    let z = (max.z.min(cell.z as f32 + 1.0) - min.z.max(cell.z as f32)).max(0.0);
    x * z
}

/// Route-cost surcharge for every hazardous foothold an escape crosses, in
/// path cost units: steep enough that leaving by the shortest way out beats
/// any dry detour it would save.
const HAZARD_CELL_COST: u32 = path::COST_FLAT * 8;

/// The per-cell route surcharge from `start`. Only a body already in danger
/// can reach a hazardous foothold at all (see [`step_gate`]), so a safe start
/// pays nothing and never probes.
pub(super) fn escape_cost<'c, 'w>(
    cur: &'c SectionCursor<'w>,
    params: PathParams,
    start: IVec3,
) -> impl Fn(IVec3) -> u32 + use<'c, 'w> {
    let escaping = foothold_in_hazard(cur, start, params);
    let footholds = path::CellMemo::<1024>::default();
    move |cell| {
        if escaping && footholds.get(cell, |c| foothold_in_hazard(cur, c, params)) {
            HAZARD_CELL_COST
        } else {
            0
        }
    }
}

/// What the live guard has refused on the navigator's recent routes.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct Refusals {
    /// The guard refused a wish on the current route.
    on_route: bool,
    /// The current route was planned after a refusal.
    route_follows_refusal: bool,
}

impl Refusals {
    /// Record a refusal; whether it should force a repath now. Only the first
    /// refusal after a route planned in peace does: the planner cannot see
    /// what the guard sees, so any further refusal, of any waypoint, waits for
    /// the scheduled repath instead of searching every tick.
    fn refuse(&mut self) -> bool {
        let force = !self.on_route && !self.route_follows_refusal;
        self.on_route = true;
        force
    }

    /// A new route replaces the current one. Whether the refusals persist
    /// across replanning, in which case the repath interval backs off.
    pub(super) fn replan(&mut self) -> bool {
        let persistent = self.on_route && self.route_follows_refusal;
        self.route_follows_refusal = self.on_route;
        self.on_route = false;
        persistent
    }
}

impl super::Navigator {
    /// Check the final walking intent, including crowd veer, against live
    /// hazards for every segment of the body. A refused wish stops the body
    /// (see [`Refusals`] for when it also asks for a fresh route).
    #[allow(clippy::too_many_arguments)]
    pub(in crate::mob) fn avoid_hazards(
        &mut self,
        pos: Vec3,
        yaw: f32,
        size: MobSize,
        wish: Vec3,
        jump: bool,
        max_step: f32,
        cur: &SectionCursor<'_>,
    ) -> (Vec3, bool) {
        if wish == Vec3::ZERO {
            return (wish, jump);
        }
        let tolerated = self.params.tolerated;
        if body_boxes(pos, yaw, size).any(|(min, max)| body_in_hazard(cur, min, max, tolerated)) {
            return (wish, jump);
        }
        let waypoint = self.path.get(self.index).copied();
        // Look as far as the step reaches toward the waypoint, and below the
        // feet to the waypoint's floor (a walk off a ledge, or the floor
        // beside the feet when no route leads the wish).
        let (delta, landing_y) = match waypoint {
            Some(wp) => {
                let remaining =
                    (Vec3::new(wp.x as f32 + 0.5, pos.y, wp.z as f32 + 0.5) - pos).length();
                let reach = super::STEER_LOOKAHEAD
                    .max(max_step)
                    .min(remaining.max(max_step));
                let floor = wp - IVec3::Y;
                (wish * reach, floor.y as f32 + super::floor_top(cur, floor))
            }
            None => (wish * super::STEER_LOOKAHEAD.max(max_step), pos.y - 1.0),
        };
        let entering = body_boxes(pos, yaw, size).any(|(min, max)| {
            segment_enters_hazard(cur, pos, min, max, delta, landing_y, tolerated)
        });
        if !entering {
            return (wish, jump);
        }
        if self.refusals.refuse() {
            self.since_path = self.repath_interval;
        }
        (Vec3::ZERO, false)
    }
}

/// Whether one body segment moving by `delta` sweeps into a hazard: new
/// hazard cells, or more overlap with one it already overhangs.
fn segment_enters_hazard(
    cur: &SectionCursor<'_>,
    pos: Vec3,
    min: Vec3,
    max: Vec3,
    delta: Vec3,
    landing_y: f32,
    tolerated: &[Block],
) -> bool {
    let height = max.y - min.y;
    let mut sweep_min = min.min(min + delta);
    let mut sweep_max = max.max(max + delta);
    // A fluid surface is navigable footing, but can be unsafe, so the landing
    // column counts; so does the headroom a jump rises through.
    sweep_min.y = pos.y.min(landing_y) - 2.0 * EPS;
    sweep_max.y = sweep_max.y.max(landing_y + height);
    any_cell(sweep_min, sweep_max, |c| {
        if !hazardous(cur.physics_block(c), tolerated) {
            return false;
        }
        // Climbing ashore can ground the leading edge while the rest of the
        // body still overhangs the pool. Permit clearing that overlap, but
        // never increasing it or sweeping into another hazardous cell.
        let before = if c.y as f32 >= max.y || (c.y + 1) as f32 <= pos.y - 2.0 * EPS {
            0.0
        } else {
            horizontal_overlap(min, max, c)
        };
        before <= EPS || horizontal_overlap(min + delta, max + delta, c) > before + EPS
    })
}

#[cfg(test)]
mod tests;
