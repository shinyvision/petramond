//! The navigator: turns a destination cell into per-tick locomotion.
//!
//! Given a goal from the [brain](super::brain), it pathfinds (re-pathing when the goal
//! changes, and otherwise every [`REPATH_TICKS`] to refresh a stale route), then each
//! tick steers the mob toward the next foothold on the path — jumping when it reaches a
//! one-block step up, walking off ledges to descend. Waypoints are consumed as the mob
//! reaches them; if the mob stops making progress (wedged against geometry) the path is
//! abandoned so the brain can pick a new goal instead of pushing into a wall forever.
//!
//! This module is also the WORLD ADAPTER for the pure cell search in
//! [`path`]: it classifies each cell's real collision boxes
//! ([`cell_shape`] — Empty / Full / Partial), supplies the `solid`/`support`
//! probe pair built from that classification, sweeps the mob's actual body
//! AABB against partial shapes per candidate edge ([`navigation_step_gate`] — so
//! a 1/16 ladder panel is routed around instead of walked into, while the
//! open 15/16 of its cell stays walkable), and prices the cells other
//! entities occupy ([`NavObstacles`]) so routes bend around mobs and players
//! without ever being walled off by them.

use rustc_hash::FxHashMap;

use crate::world::{SectionCursor, World};
use petramond_math::math::{IVec3, Vec3};
use petramond_world::block::{Aabb, Block};
use petramond_world::collision;

use super::brain::AiMob;
use super::path::{self, PathParams};
use super::{def, EntityRef, PlayerAnchor};

mod budget;
mod hazards;
mod kept;
mod probe;
mod step_gate;

pub use budget::ReachBudget;
pub(super) use hazards::foothold_in_hazard;
pub use kept::KeptBoxes;
pub(super) use probe::destination_reachable;
#[cfg(any(test, feature = "test-support"))]
pub use probe::route_path;
pub use probe::{
    footholds, mob_can_reach, route_probe, site_open, walk_region, FloodAsk, ROUTE_PROBE_MAX_NODES,
    ROUTE_PROBE_TICK_BUDGET,
};
use step_gate::floor_top;
pub(super) use step_gate::navigation_step_gate;

/// Largest horizontal distance (m) within which a waypoint counts as reached. The
/// actual threshold tightens for wide mobs so they don't turn before their body has
/// cleared a corner.
const MAX_ARRIVE_XZ: f32 = 0.3;
/// Never require perfect centre hits; discrete tick movement can step over a waypoint
/// by a few centimetres.
const MIN_ARRIVE_XZ: f32 = 0.04;
/// Vertical distance (m) within which the mob is "on the waypoint's level" — so a
/// descent waypoint isn't marked reached until the mob has actually fallen to it.
const ARRIVE_Y: f32 = 1.1;
/// Begin a jump once the body's leading edge is this close to the higher waypoint's
/// centre. The actual centre-distance threshold is `half_width + this`: wider mobs
/// reach a ledge with their body before their centre gets near it.
const JUMP_TRIGGER_FRONT_XZ: f32 = 0.7;
/// Ticks of negligible movement before the path is abandoned (~2 s at 20 TPS).
const STUCK_TICKS: u32 = 40;
/// Squared per-tick displacement below which the mob counts as "not progressing".
const STUCK_EPS_SQ: f32 = 0.015 * 0.015;
/// Steered ticks without the body improving its BEST-ACHIEVED distance to the
/// held goal before the route is abandoned. Raw displacement cannot prove
/// liveness for a ballistic gait: a hopper whose ~1-block stride cannot
/// resolve a pocket smaller than itself ping-pongs between its faces at full
/// speed forever — plenty of movement, zero progress (the 2026-08-17 rabbit
/// cliff-edge trap). Net progress toward the GOAL is the definition of going
/// somewhere, and it survives repaths (unlike any per-waypoint signal — a
/// repath can legitimately re-target a just-passed cell). Sized above any
/// honest non-improving phase of a local detour, far below "forever".
const GOAL_STALL_CALLS: u32 = 60;
/// Lateral distance (m) from a route segment's line within which the body
/// counts as ON that segment for passing consumption (see
/// [`Navigator::advance_cursor`]). Wide enough to absorb a ballistic
/// landing's drift off the line and a shove; narrow enough that a parallel
/// corridor one cell over never reads as this one.
const PASS_CORRIDOR: f32 = 0.6;
/// Minimum progress (m) past a waypoint — along its outgoing segment, or
/// beyond the end of its incoming one — that counts as having PASSED it.
/// Big enough that float noise at a corner can't consume the turn before
/// the body actually rounds it; far below one tick's step for any real
/// walk speed.
const PASS_EPS: f32 = 0.05;
/// How many waypoints ahead of the cursor the passing scan examines. A
/// ballistic arc (a mod-driven hop, a knockback flight) can carry the body
/// past at most a couple of one-cell waypoints between two ticks.
const PASS_LOOKAHEAD: usize = 3;
/// Re-pathfind toward an *unchanged* goal once this many ticks have passed since the
/// current path was computed — once a second at 20 TPS. Long enough that holding a goal
/// across ticks is cheap, short enough that a path computed against an earlier world
/// state is refreshed: it invalidates a route since blocked by terrain changes, and
/// picks up a now-shorter one, instead of a mob following a stale path forever. The
/// stuck tally (above) carries across the refresh, so a mob wedged the whole time still
/// eventually gives up rather than re-pathing into the same wall indefinitely.
const REPATH_TICKS: u32 = 20;
/// Longest same-goal retry interval after repeated partial/failed searches.
const MAX_REPATH_BACKOFF_TICKS: u32 = 200;

pub struct Navigator {
    path: Vec<IVec3>,
    /// Index of the next waypoint to walk to.
    index: usize,
    goal: Option<IVec3>,
    /// Whether the current path reaches `goal`. Failed searches still keep a
    /// best-effort partial path, but same-goal refreshes back off.
    path_reaches_goal: bool,
    params: PathParams,
    half_width: f32,
    /// The body's REAL height (m) — the edge gate sweeps the actual AABB, not
    /// the whole-cell head count.
    height: f32,
    stuck: u32,
    last_pos: petramond_math::world_pos::WorldPos,
    /// Best horizontal distance to the held goal achieved so far, and the
    /// steered ticks since it last improved — the [`GOAL_STALL_CALLS`]
    /// liveness signal. Reset on goal change only; deliberately NOT on
    /// repath (the tally must keep climbing across route refreshes, like
    /// [`stuck`](Self::stuck)).
    goal_best: f32,
    goal_stall: u32,
    /// Ticks since the current path was computed; at [`REPATH_TICKS`] the held goal is
    /// re-pathed to refresh a route gone stale (see the constant).
    since_path: u32,
    /// Current same-goal retry interval. Successful routes and goal changes reset this
    /// to [`REPATH_TICKS`]; repeated partial searches double it up to the cap.
    repath_interval: u32,
    /// What the live hazard guard refused on recent routes; persistent
    /// refusals back off like a partial route instead of recomputing.
    refusals: hazards::Refusals,
    #[cfg(test)]
    recomputes: u32,
}

impl Navigator {
    pub fn new(head: i32, half_width: f32, height: f32) -> Self {
        Navigator {
            path: Vec::new(),
            index: 0,
            goal: None,
            path_reaches_goal: false,
            params: PathParams::for_body(head, half_width),
            half_width,
            height,
            stuck: 0,
            last_pos: petramond_math::world_pos::WorldPos::ZERO,
            goal_best: f32::INFINITY,
            goal_stall: 0,
            since_path: 0,
            repath_interval: REPATH_TICKS,
            refusals: hazards::Refusals::default(),
            #[cfg(test)]
            recomputes: 0,
        }
    }

    /// This navigator for a species that tolerates the hazardous `blocks`.
    pub fn tolerating(mut self, blocks: &'static [Block]) -> Self {
        self.params = self.params.tolerating(blocks);
        self
    }

    /// No active path — the mob has arrived, given up, or was never tasked. The
    /// brain reads this (via `AiCtx::nav_idle`) to know it may pick a new goal.
    pub fn is_idle(&self) -> bool {
        self.goal.is_none() || self.index >= self.path.len()
    }

    /// The current path (foothold cells, start→goal), for tests to observe re-pathing.
    #[cfg(test)]
    pub(super) fn path(&self) -> &[IVec3] {
        &self.path
    }

    #[cfg(test)]
    pub(super) fn recomputes(&self) -> u32 {
        self.recomputes
    }

    fn clear(&mut self) {
        self.path.clear();
        self.index = 0;
        self.goal = None;
        self.path_reaches_goal = false;
        self.stuck = 0;
        self.goal_best = f32::INFINITY;
        self.goal_stall = 0;
        self.since_path = 0;
        self.repath_interval = REPATH_TICKS;
        self.refusals = hazards::Refusals::default();
    }

    /// Set the navigation goal and keep the path fresh. A *new* goal is pathed at once
    /// (resetting progress + the stuck tally); the *same* goal held across ticks costs
    /// nothing until [`REPATH_TICKS`] elapse, then it is re-pathed to refresh a route
    /// the changing world may have invalidated or shortened. `None` clears the path.
    ///
    /// Periodic and new-goal pathfinding is paused while `can_repath` is false. A
    /// falling mob keeps following its existing route instead of recomputing from
    /// transient mid-air cells.
    pub fn update_goal_when_supported(
        &mut self,
        goal: Option<IVec3>,
        start: IVec3,
        world: &World,
        can_repath: bool,
        obstacles: &NavObstacles,
    ) {
        match goal {
            None => {
                if self.goal.is_some() {
                    self.clear();
                }
            }
            Some(g) => {
                if !can_repath {
                    return;
                }
                if self.goal != Some(g) {
                    // A new goal: path to it afresh and reset the stuck tally — this is a
                    // deliberate new destination, not the same one re-evaluated. It also
                    // drops any unreachable-goal backoff from the previous cell.
                    self.repath_interval = REPATH_TICKS;
                    self.recompute(start, g, world, obstacles, true);
                    self.goal = Some(g);
                    self.stuck = 0;
                    self.goal_best = f32::INFINITY;
                    self.goal_stall = 0;
                } else {
                    // Same goal held: refresh the route at the current interval. A
                    // reachable route uses the normal cadence; repeated partial/failed
                    // routes stretch this interval to avoid exhausting A* every second for
                    // an unreachable target. The stuck tally is left to keep climbing
                    // across refreshes, so a mob wedged the whole time still abandons the
                    // goal rather than re-pathing forever.
                    self.since_path = self.since_path.saturating_add(1);
                    if self.since_path >= self.repath_interval {
                        self.recompute(start, g, world, obstacles, false);
                    }
                }
            }
        }
    }

    /// (Re)compute the path from `start` to `goal`, resetting the waypoint cursor to the
    /// first step and the repath timer. Shared by a goal change and the periodic refresh.
    ///
    /// Both cases try to PRESERVE the waypoint the mob is already walking toward
    /// when it is still a valid immediate step toward the new route: a chased
    /// target crossing a cell boundary changes the goal several times a second,
    /// and re-picking between equal-cost first steps every time snaps the mob
    /// laterally mid-stride. Keeping the in-progress step costs at most one cell
    /// of detour; `preserve_waypoint_path` refuses anything worse.
    fn recompute(
        &mut self,
        start: IVec3,
        goal: IVec3,
        world: &World,
        obstacles: &NavObstacles,
        goal_changed: bool,
    ) {
        let cursor = world.cursor();
        let solid = nav_solid_fn(&cursor);
        let support = nav_support_fn(&cursor, self.half_width);
        let fluid = nav_fluid_fn(&cursor);
        let step_allowed = navigation_step_gate(&cursor, self.params, self.height);
        let costs = entity_cell_costs(obstacles, start);
        let escape_cost = hazards::escape_cost(&cursor, self.params, start);
        let cell_cost = |c: IVec3| costs.get(&c).copied().unwrap_or(0) + escape_cost(c);
        let old_waypoint = (self.index < self.path.len()).then(|| self.path[self.index]);
        self.path = old_waypoint
            .and_then(|wp| {
                preserve_waypoint_path(
                    start,
                    wp,
                    goal,
                    self.params,
                    &solid,
                    &support,
                    &fluid,
                    &step_allowed,
                    &cell_cost,
                )
            })
            .unwrap_or_else(|| {
                path::find_path_nav(
                    start,
                    goal,
                    self.params,
                    &solid,
                    &support,
                    &fluid,
                    &step_allowed,
                    cell_cost,
                )
            });
        self.path_reaches_goal = self.path.last().is_some_and(|&last| last == goal);
        // Index 1 = the first cell to walk to (path[0] is the start).
        self.index = if self.path.len() > 1 {
            1
        } else {
            self.path.len()
        };
        self.since_path = 0;
        let refused_again = self.refusals.replan();
        if (self.path_reaches_goal || goal_changed) && !refused_again {
            self.repath_interval = REPATH_TICKS;
        } else {
            self.repath_interval = next_repath_backoff(self.repath_interval);
        }
        #[cfg(test)]
        {
            self.recomputes += 1;
        }
    }

    /// [`follow`](Self::follow) plus collision-aware steering: the raw wish aims
    /// straight at the waypoint centre from wherever the body ACTUALLY is, but a
    /// mob standing offset from the planned line (it wandered flush against a
    /// trough; it was shoved) would then press its body diagonally into a shape
    /// the plan itself avoids. Before emitting the wish, the real body AABB is
    /// swept a short lookahead along it; a blocked axis has its component
    /// dropped, so the mob walks cleanly ALONG the obstacle's face — facing its
    /// true travel direction — instead of grinding into it until physics happens
    /// to free it. The deflection never fires on the final approach (the probe
    /// is capped at the remaining distance) nor when the plan wants a step-up
    /// jump (the ledge face ahead IS the route).
    pub fn follow_steered(
        &mut self,
        pos: petramond_math::world_pos::WorldPos,
        on_ground: bool,
        world: &World,
    ) -> (Vec3, bool) {
        let (wish, jump) = self.follow(pos, on_ground);
        if jump || wish == Vec3::ZERO || self.index >= self.path.len() {
            return (wish, jump);
        }
        let wp = self.path[self.index];
        // A step-up approach must keep pressing toward the ledge face so the
        // jump trigger and the climb keep working exactly as before.
        if f64::from(wp.y) > pos.y + 0.5 {
            return (wish, jump);
        }
        let (dx, dz) = cell_centre_offset(wp, pos);
        let remaining = (dx * dx + dz * dz).sqrt();
        let boxes = |x: i32, y: i32, z: i32| world.collision_boxes_at(x, y, z);
        (
            deflect_wish(pos, self.half_width, self.height, wish, remaining, &boxes),
            jump,
        )
    }

    /// Advance the waypoint cursor from the body's LIVE position — stateless
    /// per tick and MONOTONIC over the route, so steering back to a place
    /// the body has already been past is impossible by construction (the
    /// prior approach-history overshoot detector could refuse a pass it
    /// hadn't watched closely, turning fast or ballistic movers around —
    /// the hop-off-a-ledge rubber-band, the hushjaw backtrack).
    ///
    /// Two rules, evaluated fresh from geometry every tick:
    /// - PASSING (projection): waypoint `j` is consumed once the body has
    ///   provably progressed past it along the route — its horizontal
    ///   projection lies more than [`PASS_EPS`] along `j`'s OUTGOING segment,
    ///   or beyond the far end of `j`'s INCOMING one (an overrun straight
    ///   through) — while within [`PASS_CORRIDOR`] of that segment's line
    ///   and within [`ARRIVE_Y`] of one of its endpoints' levels (a route
    ///   crossing a different storey never consumes). The farthest passed
    ///   waypoint within [`PASS_LOOKAHEAD`] wins, so an arc that clears two
    ///   cells consumes both. A body approaching a corner projects at zero
    ///   progress onto the turn's outgoing segment until it actually rounds
    ///   it, so the wide-body corner-clearance contract holds untouched.
    /// - ARRIVAL: the current target within [`arrive_xz`](Self::arrive_xz)
    ///   at level — the only rule that can consume a waypoint the body
    ///   stops ON (projection needs motion past it).
    ///
    /// Called from [`follow`](Self::follow) every steered tick and directly
    /// (as the whole bookkeeping) while steering is suspended — a hop's
    /// descent, a fall, a knockback flight — because a ballistic arc passes
    /// waypoints between steered ticks.
    pub fn advance_cursor(&mut self, pos: petramond_math::world_pos::WorldPos) {
        // Passing: scan the lookahead window; the farthest passed wins.
        let end = self
            .path
            .len()
            .saturating_sub(1)
            .min(self.index + PASS_LOOKAHEAD);
        let mut passed: Option<usize> = None;
        for j in self.index..=end {
            if j == 0 {
                continue;
            }
            let level_ok = |cell: IVec3| ((pos.y - f64::from(cell.y)) as f32).abs() <= ARRIVE_Y;
            // Overrun of the incoming segment: projection beyond its far end.
            let (t_in, lat_in, len_in) = project_horizontal(pos, self.path[j - 1], self.path[j]);
            let over_in = t_in > len_in + PASS_EPS
                && lat_in <= PASS_CORRIDOR
                && (level_ok(self.path[j - 1]) || level_ok(self.path[j]));
            // Progress along the outgoing segment (when one exists).
            let over_out = j + 1 < self.path.len() && {
                let (t_out, lat_out, _) = project_horizontal(pos, self.path[j], self.path[j + 1]);
                t_out > PASS_EPS
                    && lat_out <= PASS_CORRIDOR
                    && (level_ok(self.path[j]) || level_ok(self.path[j + 1]))
            };
            if over_in || over_out {
                passed = Some(j);
            }
        }
        if let Some(j) = passed {
            self.index = self.index.max(j + 1);
        }
        // Arrival cascade on the (possibly advanced) current target.
        let arrive_xz = self.arrive_xz();
        while self.index < self.path.len() {
            let wp = self.path[self.index];
            let (dx, dz) = cell_centre_offset(wp, pos);
            let horiz = (dx * dx + dz * dz).sqrt();
            let dy = ((pos.y - f64::from(wp.y)) as f32).abs();
            if horiz <= arrive_xz && dy <= ARRIVE_Y {
                self.index += 1;
            } else {
                break;
            }
        }
    }

    /// This tick's locomotion: a unit horizontal `wish` direction toward the current
    /// waypoint, and whether to jump. Consumes waypoints as they're reached and
    /// abandons the path if the mob stalls.
    pub fn follow(
        &mut self,
        pos: petramond_math::world_pos::WorldPos,
        on_ground: bool,
    ) -> (Vec3, bool) {
        self.advance_cursor(pos);
        if self.index < self.path.len() {
            let wp = self.path[self.index];
            let (dx, dz) = cell_centre_offset(wp, pos);
            let horiz = (dx * dx + dz * dz).sqrt();

            // Progress / stuck tracking.
            let progress = pos - self.last_pos;
            let (progress_dx, progress_dz) = (progress.x, progress.z);
            if progress_dx * progress_dx + progress_dz * progress_dz < STUCK_EPS_SQ {
                self.stuck += 1;
            } else {
                self.stuck = 0;
            }
            self.last_pos = pos;
            if self.stuck >= STUCK_TICKS {
                self.clear();
                return (Vec3::ZERO, false);
            }
            // Goal-progress liveness (see GOAL_STALL_CALLS): moving a lot is
            // not going somewhere — abandon a route whose best-achieved
            // distance to the goal has stopped improving.
            if let Some(goal) = self.goal {
                let (gx, gz) = cell_centre_offset(goal, pos);
                let goal_dist = (gx * gx + gz * gz).sqrt();
                if goal_dist + 1e-3 < self.goal_best {
                    self.goal_best = goal_dist;
                    self.goal_stall = 0;
                } else {
                    self.goal_stall += 1;
                    if self.goal_stall >= GOAL_STALL_CALLS {
                        self.clear();
                        return (Vec3::ZERO, false);
                    }
                }
            }

            let dir = if horiz > 1e-4 {
                Vec3::new(dx / horiz, 0.0, dz / horiz)
            } else {
                Vec3::ZERO
            };
            // Jump when the next waypoint is a step up and we're grounded + close to
            // the edge, so forward speed carries the mob onto the higher block.
            let step_up = f64::from(wp.y) > pos.y + 0.5;
            let jump = on_ground && step_up && horiz <= self.half_width + JUMP_TRIGGER_FRONT_XZ;
            return (dir, jump);
        }

        // Path exhausted. A route that reached the goal has arrived and resets the
        // navigator. A partial route stays associated with the same goal so held-goal
        // retries obey the unreachable-goal backoff instead of immediately recomputing.
        if self.path_reaches_goal {
            self.clear();
        } else {
            self.path.clear();
            self.index = 0;
        }
        (Vec3::ZERO, false)
    }

    /// The route cell a falling body is over and dropping into: one of the
    /// waypoints around the cursor, at or below the feet, whose centre the
    /// body has reached along its drift `vel`. A fall keeps the sideways
    /// speed the ledge was left with, and carried on it lands a cell late —
    /// which on a flight of steps down is the next drop, and the one after.
    pub fn landing_under(
        &self,
        pos: petramond_math::world_pos::WorldPos,
        vel: Vec3,
    ) -> Option<IVec3> {
        let around = self.index.saturating_sub(1)..=self.index;
        around.filter_map(|i| self.path.get(i).copied()).find(|wp| {
            let (dx, dz) = cell_centre_offset(*wp, pos);
            f64::from(wp.y) <= pos.y + 0.01
                && dx.abs() < 0.5
                && dz.abs() < 0.5
                && dx * vel.x + dz * vel.z <= 0.0
        })
    }

    fn arrive_xz(&self) -> f32 {
        MAX_ARRIVE_XZ.min((0.5 - self.half_width).max(MIN_ARRIVE_XZ))
    }
}

/// The horizontal offset from `pos` to the centre of `cell`, taken in double
/// precision before it narrows to the local frame.
fn cell_centre_offset(cell: IVec3, pos: petramond_math::world_pos::WorldPos) -> (f32, f32) {
    (
        (f64::from(cell.x) + 0.5 - pos.x) as f32,
        (f64::from(cell.z) + 0.5 - pos.z) as f32,
    )
}

/// Horizontal projection of `pos` onto the route segment between cell centres
/// `a` and `b`: (signed distance along the segment direction from `a` in
/// metres — may be negative or beyond the length — the perpendicular distance
/// from the segment's line, and the segment's length).
fn project_horizontal(
    pos: petramond_math::world_pos::WorldPos,
    a: IVec3,
    b: IVec3,
) -> (f32, f32, f32) {
    let (dx, dz) = ((b.x - a.x) as f32, (b.z - a.z) as f32);
    let len = (dx * dx + dz * dz).sqrt();
    let (ax, az) = cell_centre_offset(a, pos);
    let (px, pz) = (-ax, -az);
    if len <= 1e-6 {
        return (0.0, (px * px + pz * pz).sqrt(), 0.0);
    }
    let (ux, uz) = (dx / len, dz / len);
    (px * ux + pz * uz, (px * uz - pz * ux).abs(), len)
}

/// How far ahead (m) the steering probe sweeps the body along the wish. About
/// half a body: far enough to react a couple of ticks before contact, short
/// enough that unrelated geometry beyond the current move never deflects.
const STEER_LOOKAHEAD: f32 = 0.4;

/// Collision-aware wish adjustment (see [`Navigator::follow_steered`]): sweep
/// the body along `wish` up to `remaining` (never past the waypoint — the
/// final approach must stay allowed to close in on a face-adjacent centre);
/// when travel is cut short, drop the blocked axis component and keep the open
/// one, re-normalised. Both blocked (a true head-on, which a valid plan
/// doesn't produce from a centred pose) keeps the original wish — the shared
/// resolver still slides, and the stuck tally + repath remain the backstop.
fn deflect_wish(
    pos: petramond_math::world_pos::WorldPos,
    half_width: f32,
    height: f32,
    wish: Vec3,
    remaining: f32,
    boxes: &impl Fn(i32, i32, i32) -> &'static [Aabb],
) -> Vec3 {
    let lookahead = STEER_LOOKAHEAD.min(remaining);
    if lookahead <= 1e-3 {
        return wish;
    }
    let hw = f64::from(half_width.max(0.0));
    // A hair above the feet so the floor being rested on never reads as a
    // cross-axis overlap under float noise.
    let min = [pos.x - hw, pos.y + 1e-3, pos.z - hw];
    let max = [pos.x + hw, pos.y + f64::from(height.max(0.5)), pos.z + hw];
    let (dx, dz) = (wish.x * lookahead, wish.z * lookahead);
    // The same step allowance walking uses: something the body would simply
    // step onto is not an obstacle worth deflecting around.
    let (_, hit_x, hit_z) =
        collision::step_horizontal(min, max, dx, dz, collision::STEP_HEIGHT, boxes);
    if !hit_x && !hit_z {
        return wish;
    }
    let deflected = Vec3::new(
        if hit_x { 0.0 } else { wish.x },
        0.0,
        if hit_z { 0.0 } else { wish.z },
    );
    if deflected.length_squared() <= 1e-4 {
        return wish;
    }
    deflected.normalize_or_zero()
}

#[allow(clippy::too_many_arguments)]
fn preserve_waypoint_path(
    start: IVec3,
    waypoint: IVec3,
    goal: IVec3,
    params: PathParams,
    solid: &impl Fn(IVec3) -> bool,
    support: &impl Fn(IVec3) -> bool,
    fluid: &impl Fn(IVec3) -> bool,
    step_allowed: &impl Fn(IVec3, IVec3) -> bool,
    cell_cost: &impl Fn(IVec3) -> u32,
) -> Option<Vec<IVec3>> {
    if waypoint == start {
        return None;
    }
    let step = path::find_path_nav(
        start,
        waypoint,
        params,
        solid,
        support,
        fluid,
        step_allowed,
        cell_cost,
    );
    if step.last() != Some(&waypoint) || step.len() > 2 {
        return None;
    }
    if waypoint == goal {
        return Some(step);
    }
    let suffix = path::find_path_nav(
        waypoint,
        goal,
        params,
        solid,
        support,
        fluid,
        step_allowed,
        cell_cost,
    );
    if suffix.first() != Some(&waypoint) || suffix.len() <= 1 {
        return None;
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
        return None;
    }
    let (ax, az) = (waypoint.x - start.x, waypoint.z - start.z);
    let (bx, bz) = (suffix[1].x - waypoint.x, suffix[1].z - waypoint.z);
    if ax * bx + az * bz < 0 {
        return None;
    }
    let mut stitched = step;
    stitched.extend_from_slice(&suffix[1..]);
    Some(stitched)
}

fn next_repath_backoff(current: u32) -> u32 {
    current
        .max(REPATH_TICKS)
        .saturating_mul(2)
        .min(MAX_REPATH_BACKOFF_TICKS)
}

/// How far ahead (centre distance) a touching body counts as "in the way" —
/// the mob's own half-width plus a pressing margin that covers the other
/// body's radius.
const UNSTICK_REACH: f32 = 0.6;
/// The fixed veer angle applied to a blocked wish.
const UNSTICK_ANGLE: f32 = std::f32::consts::FRAC_PI_3;
/// Ticks a veer keeps holding after its blocking contact stops registering.
/// Contacts are recorded from the PREVIOUS tick's overlap, so a successful
/// veer erases its own trigger one tick later; without this hold the veer
/// flip-flops — veer, straight, re-press, veer — and the wish (which the
/// body FACES) wags ±60° at a few hertz: the crowd-jitter bug. Committing to
/// the side briefly walks a small clean arc around the peer instead.
const UNSTICK_HOLD_TICKS: u8 = 8;

/// The crowd veer with its side COMMITMENT (2026-07-20; the stateless
/// version jittered). While a touching entity blocks the wish, the wish is
/// rotated [`UNSTICK_ANGLE`] to the side away from the contact (a dead-ahead
/// tie breaks on the stable id) so two mobs pushing each other slide past
/// instead of cancelling out; the chosen side then HOLDS — against contact
/// flicker and against the cross-product changing its mind mid-manoeuvre —
/// until the contact has stayed gone for [`UNSTICK_HOLD_TICKS`]. Transient
/// per-instance steering state; never persisted.
#[derive(Default)]
pub(super) struct Unstick {
    /// The committed veer side (+1 / −1); meaningful while `hold > 0`.
    side: f32,
    /// Ticks of commitment left once the blocking contact stops registering.
    hold: u8,
}

impl Unstick {
    /// Veer `wish` around a touching entity it drives into. The brain's
    /// current TARGET never deflects — a hunter means to reach its prey.
    /// Deterministic: the same scene and latch state always veer the same way.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn steer(
        &mut self,
        wish: Vec3,
        pos: petramond_math::world_pos::WorldPos,
        self_id: u64,
        half_width: f32,
        contacts: &[EntityRef],
        target: Option<EntityRef>,
        mobs: &[AiMob],
        players: &[PlayerAnchor],
    ) -> Vec3 {
        if wish == Vec3::ZERO {
            // Standing: let any leftover commitment expire so it can't bend
            // the first step of the next walk half a second later.
            self.hold = self.hold.saturating_sub(1);
            return wish;
        }
        match blocking_bearing(wish, pos, half_width, contacts, target, mobs, players) {
            Some(d) => {
                // A live block refreshes the commitment; the side is only
                // (re)chosen when no commitment is running.
                if self.hold == 0 {
                    self.side = veer_side(wish, d, self_id);
                }
                self.hold = UNSTICK_HOLD_TICKS;
                veer(wish, self.side)
            }
            None if self.hold > 0 => {
                // The contact cleared — keep rounding the peer on the same
                // side while the commitment runs down, instead of snapping
                // straight and pressing right back into it.
                self.hold -= 1;
                veer(wish, self.side)
            }
            None => wish,
        }
    }
}

/// The bearing of the nearest touching entity that blocks `wish` (touching
/// bodies only — far bodies are the soft route costs' business), or `None`
/// when nothing ahead is pressed against.
fn blocking_bearing(
    wish: Vec3,
    pos: petramond_math::world_pos::WorldPos,
    half_width: f32,
    contacts: &[EntityRef],
    target: Option<EntityRef>,
    mobs: &[AiMob],
    players: &[PlayerAnchor],
) -> Option<Vec3> {
    let mut best: Option<(Vec3, f32)> = None;
    let mut consider = |other: petramond_math::world_pos::WorldPos| {
        let d = other - pos;
        let dist2 = d.x * d.x + d.z * d.z;
        if best.is_none_or(|(_, bd)| dist2 < bd) {
            best = Some((d, dist2));
        }
    };
    for c in contacts {
        if Some(*c) == target {
            continue;
        }
        match c {
            EntityRef::Mob(id) => {
                if let Some(m) = mobs.iter().find(|m| m.id == *id && m.active) {
                    consider(m.pos);
                }
            }
            EntityRef::Player(id) => {
                if let Some(p) = players.iter().find(|p| p.id == *id) {
                    consider(p.pos);
                }
            }
        }
    }
    let (d, dist2) = best?;
    let dist = dist2.sqrt();
    if dist > half_width + UNSTICK_REACH {
        return None;
    }
    let facing = if dist > 1e-3 {
        (d.x * wish.x + d.z * wish.z) / dist
    } else {
        1.0
    };
    // A contact behind the travel direction is not ours to dodge.
    (facing > 0.0).then_some(d)
}

/// The side to veer AWAY from a contact at bearing `d`; a dead-ahead contact
/// (cross ≈ 0) picks a side by the mob's stable id, so a head-on pair stops
/// being anti-parallel and the pushes gain a lateral component.
fn veer_side(wish: Vec3, d: Vec3, self_id: u64) -> f32 {
    let cross = wish.x * d.z - wish.z * d.x;
    if cross.abs() < 1e-3 {
        if self_id.is_multiple_of(2) {
            1.0
        } else {
            -1.0
        }
    } else {
        cross.signum()
    }
}

fn veer(wish: Vec3, side: f32) -> Vec3 {
    let (sin, cos) = (side * UNSTICK_ANGLE).sin_cos();
    Vec3::new(
        wish.x * cos + wish.z * sin,
        0.0,
        wish.z * cos - wish.x * sin,
    )
}

/// How a cell's real collision reads for navigation.
#[derive(Copy, Clone, PartialEq)]
enum CellShape {
    /// No collision boxes: freely passable, bears nothing.
    Empty,
    /// One box filling the whole cell: a body can never be inside it.
    Full,
    /// Any other box set (a ladder panel, a pane, a chest, a slab, a door, a
    /// model block's legs): routable in principle — whether a specific body
    /// fits a specific move is [`navigation_step_gate`]'s call.
    Partial,
}

fn classify_boxes(boxes: &[Aabb]) -> CellShape {
    if boxes.is_empty() {
        CellShape::Empty
    } else if boxes.len() == 1 && boxes[0].min == [0.0; 3] && boxes[0].max == [1.0; 3] {
        CellShape::Full
    } else {
        CellShape::Partial
    }
}

/// The coarse `solid` probe for cell navigation: only FULL cells block a cell
/// outright. Partial shapes are the edge gate's business — treating them as
/// solid walls off routes a body actually fits through (a ladder corridor),
/// while treating them as air walks mobs into their boxes forever.
/// The one BY-DESIGN exception is a shape that DECLARES itself nav-solid through
/// its [`ShapeSim::nav_reads_solid`](petramond_world::block::ShapeSim) facet — the fence
/// family, and any custom shape with `nav_solid` set. Such a cell always
/// reads solid, so no route steps through it and the one-block jump from the
/// ground is no foothold jump either (see [`nav_support_fn`] for the step-up
/// caveat) — a lone fence/hedge is a wall here or no pen would hold.
pub(super) fn nav_solid_fn<'c, 'w>(
    cur: &'c SectionCursor<'w>,
) -> impl Fn(IVec3) -> bool + use<'c, 'w> {
    move |c: IVec3| {
        // ONE cell read serves both questions: the nav-solid declaration and
        // the box classification are per-id facts for every shape whose
        // collision is state-free, and the rest resolve from the block we
        // already hold.
        let block = cur.physics_block(c);
        if block.nav_reads_solid() {
            return true;
        }
        classify_boxes(cur.boxes_of(c, block)) == CellShape::Full
    }
}

/// The `fluid` probe every navigation search shares.
pub(super) fn nav_fluid_fn<'c, 'w>(
    cur: &'c SectionCursor<'w>,
) -> impl Fn(IVec3) -> bool + use<'c, 'w> {
    move |c: IVec3| cur.fluid_cell(c)
}

/// The fluid a body at `pos` stands in or rests on, by cell: its feet cell or
/// the one below. This is navigation footing, deliberately wider than physical
/// immersion — a swimmer bobbing clear of its probe still routes from the
/// fluid surface.
pub(super) fn fluid_footing(
    cur: &SectionCursor<'_>,
    pos: petramond_math::world_pos::WorldPos,
) -> Option<Block> {
    let feet = pos.block();
    cur.physics_block(feet)
        .fluid()
        .or_else(|| cur.physics_block(feet - IVec3::Y).fluid())
}

/// The streaming-finality probe cell searches gate on.
pub(super) fn nav_loaded_fn<'c, 'w>(
    cur: &'c SectionCursor<'w>,
) -> impl Fn(IVec3) -> bool + use<'c, 'w> {
    move |c: IVec3| cur.cell_final(c)
}

/// The `support` probe: can this cell bear the feet of a body CENTRED in its
/// column? A full cube always can; a partial shape only when one of its boxes
/// horizontally overlaps the centred footprint — a slab, a bed, or a chest
/// does, while a door's or a ladder's thin EDGE panel does not (a body cannot
/// rest its feet on a 1/16 sliver it doesn't even cover). Without the overlap
/// test, routes confidently "stand" on top of closed doors. Pairs with
/// [`nav_solid_fn`] through the `*_with` probes in [`path`].
/// A fence top DOES support (the post overlaps the centre): a lone fence stays
/// uncrossable because its cell is `solid` and the edge gate refuses the
/// one-block sweep from the ground — while a step placed beside the fence
/// opens the honest flat route over its top, as it physically should.
pub(super) fn nav_support_fn<'c, 'w>(
    cur: &'c SectionCursor<'w>,
    half_width: f32,
) -> impl Fn(IVec3) -> bool + use<'c, 'w> {
    let hw = half_width.clamp(0.05, 0.5);
    let (lo, hi) = (0.5 - hw, 0.5 + hw);
    move |c: IVec3| {
        let boxes = cur.collision_boxes(c);
        match classify_boxes(boxes) {
            CellShape::Empty => false,
            CellShape::Full => true,
            CellShape::Partial => boxes
                .iter()
                .any(|b| b.min[0] < hi && b.max[0] > lo && b.min[2] < hi && b.max[2] > lo),
        }
    }
}

/// Cost of routing through a cell another entity's body occupies — about a
/// 20-cell detour ([`path`]'s flat step costs 10), so ANY local way around a
/// standing mob or player (over a trough, around a pen-mate) always beats
/// pressing through them, while a genuinely packed crowd still resolves by
/// paying it (the search never deadlocks, and the surcharge stays out of the
/// heuristic so A* remains admissible).
const ENTITY_CELL_COST: u32 = 200;
/// Entities farther than this from the path start are ignored when pricing
/// cells — far bodies cannot matter to a local route.
const ENTITY_AVOID_RANGE: f32 = 32.0;

/// Soft obstacles the pathfinder routes around: the OTHER entities near this
/// mob. The mob's current TARGET is exempt — a zombie chasing the player must
/// path TO the player, not around them — and so is the mob itself.
pub(super) struct NavObstacles<'a> {
    pub self_id: u64,
    pub target: Option<EntityRef>,
    pub mobs: &'a [AiMob],
    pub players: &'a [PlayerAnchor],
}

impl NavObstacles<'static> {
    /// No obstacles — tests and callers without an entity snapshot.
    #[cfg(test)]
    pub fn none() -> Self {
        NavObstacles {
            self_id: 0,
            target: None,
            mobs: &[],
            players: &[],
        }
    }
}

/// Price the cells covered by every avoided entity's body AABB. Overlapping
/// bodies stack, so the middle of a herd costs more than its edge.
fn entity_cell_costs(avoid: &NavObstacles, start: IVec3) -> FxHashMap<IVec3, u32> {
    let mut costs: FxHashMap<IVec3, u32> = FxHashMap::default();
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
    for m in avoid.mobs {
        if !m.active || m.id == avoid.self_id || avoid.target == Some(EntityRef::Mob(m.id)) {
            continue;
        }
        let s = def(m.kind).size;
        // A long body (a boat) marks its enclosing square — conservative, and
        // its rigid hull is a real obstacle a route should bend around.
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
    costs
}

#[cfg(test)]
mod tests;
