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
//! AABB against partial shapes per candidate edge ([`partial_step_gate`] — so
//! a 1/16 ladder panel is routed around instead of walked into, while the
//! open 15/16 of its cell stays walkable), and prices the cells other
//! entities occupy ([`NavObstacles`]) so routes bend around mobs and players
//! without ever being walled off by them.

use std::cell::RefCell;

use rustc_hash::FxHashMap;

use crate::block::Aabb;
use crate::collision;
use crate::mathh::{IVec3, Vec3};
use crate::world::World;

use super::brain::AiMob;
use super::path::{self, PathParams};
use super::{def, EntityRef, PlayerAnchor};

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
/// A waypoint also counts as reached on OVERSHOOT — the mob PASSED it: its
/// offset flipped sign since last tick, or it stopped getting closer — while
/// having come within this radius (just inside the waypoint's own cell). A
/// fast mob's per-tick step can exceed the tightened
/// [`arrive_xz`](Navigator::arrive_xz) window several times over (the hushjaw:
/// 0.24 m/tick vs a 0.05 m window), so it overflies the centre, turns 180°,
/// and orbits the waypoint instead of walking the route. The sign flip catches
/// the overfly BEFORE a single reversed step is emitted; the stalled-distance
/// signal catches symmetric orbits and grazes ("distance grew" alone misses an
/// orbit whose radius is constant). Closest approach is as near as that body
/// at that speed was ever going to get, so consuming there keeps the wide-body
/// corner-clearance contract intact: while the mob still closes in on a corner
/// waypoint nothing changes.
const OVERSHOOT_RADIUS: f32 = 0.45;
/// Minimum per-tick radial improvement that still counts as "getting closer" —
/// well below any real walk speed's per-tick step, so a slow mob's honest
/// approach can't be mistaken for an orbit.
const OVERSHOOT_EPS: f32 = 1e-3;
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

/// One waypoint's approach history: how near the mob has come and which way the
/// waypoint last lay — the overshoot detector's inputs (see follow()).
struct WaypointApproach {
    /// Which `path` index this history belongs to.
    index: usize,
    /// Closest horizontal distance achieved so far.
    best: f32,
    /// Horizontal offset toward the waypoint on the previous tick; a sign flip
    /// against the current offset means the mob passed it this tick.
    toward: (f32, f32),
}

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
    last_pos: Vec3,
    /// The overshoot detector's memory of the CURRENT waypoint's approach
    /// (see [`OVERSHOOT_RADIUS`]). Reset whenever the path or cursor changes.
    approach: Option<WaypointApproach>,
    /// Ticks since the current path was computed; at [`REPATH_TICKS`] the held goal is
    /// re-pathed to refresh a route gone stale (see the constant).
    since_path: u32,
    /// Current same-goal retry interval. Successful routes and goal changes reset this
    /// to [`REPATH_TICKS`]; repeated partial searches double it up to the cap.
    repath_interval: u32,
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
            last_pos: Vec3::ZERO,
            approach: None,
            since_path: 0,
            repath_interval: REPATH_TICKS,
            #[cfg(test)]
            recomputes: 0,
        }
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
        self.approach = None;
        self.since_path = 0;
        self.repath_interval = REPATH_TICKS;
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
        let solid = nav_solid_fn(world);
        let support = nav_support_fn(world, self.half_width);
        let water = |c: IVec3| world.water_cell_at(c.x, c.y, c.z);
        let step_allowed = partial_step_gate(world, self.params, self.height);
        let costs = entity_cell_costs(obstacles, start);
        let cell_cost = |c: IVec3| costs.get(&c).copied().unwrap_or(0);
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
                    &water,
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
                    &water,
                    &step_allowed,
                    &cell_cost,
                )
            });
        self.path_reaches_goal = self.path.last().is_some_and(|&last| last == goal);
        // Index 1 = the first cell to walk to (path[0] is the start).
        self.index = if self.path.len() > 1 {
            1
        } else {
            self.path.len()
        };
        self.approach = None;
        self.since_path = 0;
        if self.path_reaches_goal || goal_changed {
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
    pub fn follow_steered(&mut self, pos: Vec3, on_ground: bool, world: &World) -> (Vec3, bool) {
        let (wish, jump) = self.follow(pos, on_ground);
        if jump || wish == Vec3::ZERO || self.index >= self.path.len() {
            return (wish, jump);
        }
        let wp = self.path[self.index];
        // A step-up approach must keep pressing toward the ledge face so the
        // jump trigger and the climb keep working exactly as before.
        if wp.y as f32 > pos.y + 0.5 {
            return (wish, jump);
        }
        let (dx, dz) = (wp.x as f32 + 0.5 - pos.x, wp.z as f32 + 0.5 - pos.z);
        let remaining = (dx * dx + dz * dz).sqrt();
        let boxes = |x: i32, y: i32, z: i32| world.collision_boxes_at(x, y, z);
        (
            deflect_wish(pos, self.half_width, self.height, wish, remaining, &boxes),
            jump,
        )
    }

    /// This tick's locomotion: a unit horizontal `wish` direction toward the current
    /// waypoint, and whether to jump. Consumes waypoints as they're reached and
    /// abandons the path if the mob stalls.
    pub fn follow(&mut self, pos: Vec3, on_ground: bool) -> (Vec3, bool) {
        let arrive_xz = self.arrive_xz();
        while self.index < self.path.len() {
            let wp = self.path[self.index];
            let (tx, tz) = (wp.x as f32 + 0.5, wp.z as f32 + 0.5);
            let (dx, dz) = (tx - pos.x, tz - pos.z);
            let horiz = (dx * dx + dz * dz).sqrt();
            let dy = (pos.y - wp.y as f32).abs();

            if horiz <= arrive_xz && dy <= ARRIVE_Y {
                self.approach = None;
                self.index += 1;
                continue; // reached this waypoint; aim at the next
            }

            // Overshoot: consume a waypoint the mob has PASSED. A fast mob's
            // per-tick step can be several times the arrive window, so it may
            // never land inside it; without this it turns 180° back and
            // orbits the waypoint (often at a symmetric distance either side)
            // instead of walking the route. Two passing signals, both gated on
            // having come within [`OVERSHOOT_RADIUS`]: the waypoint's offset
            // FLIPPED sign since last tick (the overfly itself — caught before
            // a single reversed step is emitted), or the mob has stopped
            // getting closer (the symmetric orbit / a graze).
            if dy <= ARRIVE_Y {
                match &mut self.approach {
                    Some(a) if a.index == self.index => {
                        let flipped = a.toward.0 * dx + a.toward.1 * dz < 0.0;
                        let improving = horiz + OVERSHOOT_EPS < a.best;
                        if (flipped || !improving) && a.best.min(horiz) <= OVERSHOOT_RADIUS {
                            self.approach = None;
                            self.index += 1;
                            continue;
                        }
                        // Not passing yet: still far out and honestly closing
                        // in (or wedged — the stuck tally owns that case).
                        a.best = a.best.min(horiz);
                        a.toward = (dx, dz);
                    }
                    _ => {
                        self.approach = Some(WaypointApproach {
                            index: self.index,
                            best: horiz,
                            toward: (dx, dz),
                        });
                    }
                }
            }

            // Progress / stuck tracking.
            let progress_dx = pos.x - self.last_pos.x;
            let progress_dz = pos.z - self.last_pos.z;
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

            let dir = if horiz > 1e-4 {
                Vec3::new(dx / horiz, 0.0, dz / horiz)
            } else {
                Vec3::ZERO
            };
            // Jump when the next waypoint is a step up and we're grounded + close to
            // the edge, so forward speed carries the mob onto the higher block.
            let step_up = wp.y as f32 > pos.y + 0.5;
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

    fn arrive_xz(&self) -> f32 {
        MAX_ARRIVE_XZ.min((0.5 - self.half_width).max(MIN_ARRIVE_XZ))
    }
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
    pos: Vec3,
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
    let hw = half_width.max(0.0);
    // A hair above the feet so the floor being rested on never reads as a
    // cross-axis overlap under float noise.
    let min = [pos.x - hw, pos.y + 1e-3, pos.z - hw];
    let max = [pos.x + hw, pos.y + height.max(0.5), pos.z + hw];
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
    water: &impl Fn(IVec3) -> bool,
    step_allowed: &impl Fn(IVec3, IVec3) -> bool,
    cell_cost: &impl Fn(IVec3) -> u32,
) -> Option<Vec<IVec3>> {
    if waypoint == start {
        return None;
    }
    let step = path::find_path_nav(
        start, waypoint, params, solid, support, water, step_allowed, cell_cost,
    );
    if step.last() != Some(&waypoint) || step.len() > 2 {
        return None;
    }
    if waypoint == goal {
        return Some(step);
    }
    let suffix = path::find_path_nav(
        waypoint, goal, params, solid, support, water, step_allowed, cell_cost,
    );
    if suffix.first() != Some(&waypoint) || suffix.len() <= 1 {
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
        pos: Vec3,
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
    pos: Vec3,
    half_width: f32,
    contacts: &[EntityRef],
    target: Option<EntityRef>,
    mobs: &[AiMob],
    players: &[PlayerAnchor],
) -> Option<Vec3> {
    let mut best: Option<(Vec3, f32)> = None;
    let mut consider = |other: Vec3| {
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
        if self_id % 2 == 0 {
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
    Vec3::new(wish.x * cos + wish.z * sin, 0.0, wish.z * cos - wish.x * sin)
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
    /// fits a specific move is [`partial_step_gate`]'s call.
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

fn cell_shape(world: &World, c: IVec3) -> CellShape {
    classify_boxes(world.collision_boxes_at(c.x, c.y, c.z))
}

/// The coarse `solid` probe for cell navigation: only FULL cells block a cell
/// outright. Partial shapes are the edge gate's business — treating them as
/// solid walls off routes a body actually fits through (a ladder corridor),
/// while treating them as air walks mobs into their boxes forever.
/// The one BY-DESIGN exception is the fence: a fence cell always reads solid,
/// so no route steps through it and the one-block jump from the ground is no
/// foothold jump either (see [`nav_support_fn`] for the step-up caveat) — a
/// lone fence is a wall here or no pen would hold.
pub(super) fn nav_solid_fn(world: &World) -> impl Fn(IVec3) -> bool + '_ {
    move |c: IVec3| {
        if crate::fence::is_fence(world.physics_block(c.x, c.y, c.z)) {
            return true;
        }
        cell_shape(world, c) == CellShape::Full
    }
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
pub(super) fn nav_support_fn(world: &World, half_width: f32) -> impl Fn(IVec3) -> bool + '_ {
    let hw = half_width.max(0.05).min(0.5);
    let (lo, hi) = (0.5 - hw, 0.5 + hw);
    move |c: IVec3| {
        let boxes = world.collision_boxes_at(c.x, c.y, c.z);
        match classify_boxes(boxes) {
            CellShape::Empty => false,
            CellShape::Full => true,
            CellShape::Partial => boxes
                .iter()
                .any(|b| b.min[0] < hi && b.max[0] > lo && b.min[2] < hi && b.max[2] > lo),
        }
    }
}

/// Node budget for one goal-reachability probe (wander picks, the mod ABI's
/// `MobCanReach`). Destinations sit within local seek radii, so an honest
/// route needs far fewer expansions than the navigator's full budget — and it
/// is the UNREACHABLE probe that pays the entire budget before failing, so a
/// small cap keeps the worst tick cheap. A spot the budget can't prove
/// reachable counts as unreachable.
pub const REACH_PROBE_NODES: usize = 600;

/// Whether a body (`params`, physical `height`) standing at foothold `start`
/// can genuinely path to `dest` within [`REACH_PROBE_NODES`]. Entity
/// soft-costs are deliberately absent: bodies never make a spot unreachable,
/// and the navigator prices them when it routes for real. This is the
/// DESTINATION-honesty gate — the pathfinder deliberately walks best-effort
/// partial routes toward unreachable goals (chases must crowd their target),
/// which parks a mob against the obstacle when the goal was a picked CELL, so
/// every cell-picking policy must ask this first.
pub(super) fn destination_reachable(
    world: &World,
    start: IVec3,
    dest: IVec3,
    mut params: PathParams,
    height: f32,
) -> bool {
    params.max_nodes = REACH_PROBE_NODES;
    let solid = nav_solid_fn(world);
    let support = nav_support_fn(world, params.half_width);
    let water = |c: IVec3| world.water_cell_at(c.x, c.y, c.z);
    let step_allowed = partial_step_gate(world, params, height);
    path::find_path_nav(start, dest, params, &solid, &support, &water, step_allowed, |_| 0).last()
        == Some(&dest)
}

/// [`destination_reachable`] for a live mob instance: probes from the mob's
/// current navigation cell with its real body. `false` when the mob is not on
/// a foothold (airborne — nothing is provable, callers retry later). The
/// `MobCanReach` HostCall's engine seam.
pub fn mob_can_reach(world: &World, mob: &super::Instance, dest: IVec3) -> bool {
    let d = super::def(mob.kind);
    let params = PathParams::for_body(d.size.head_cells(), d.size.half_width);
    let solid = nav_solid_fn(world);
    let support = nav_support_fn(world, d.size.half_width);
    let water = |c: IVec3| world.water_cell_at(c.x, c.y, c.z);
    let feet = crate::mathh::voxel_at(mob.pos);
    let in_water = water(feet) || water(feet - IVec3::Y);
    let start = path::navigation_cell_with(
        mob.pos,
        d.size.half_width,
        d.size.head_cells(),
        in_water,
        &solid,
        &support,
        &water,
    )
    .unwrap_or(feet);
    destination_reachable(world, start, dest, params, d.size.height)
}

/// The collision boxes navigation must sweep against in `c` — the PARTIAL
/// shapes only. Full cubes are already resolved exactly by the cell probes and
/// empty cells contribute nothing, so both answer an empty slice.
fn nav_partial_boxes(world: &World, c: IVec3) -> &'static [Aabb] {
    let boxes = world.collision_boxes_at(c.x, c.y, c.z);
    match classify_boxes(boxes) {
        CellShape::Partial => boxes,
        CellShape::Empty | CellShape::Full => &[],
    }
}

/// The top of the standing surface under foothold `cell`, in `[0, 1]` above
/// the floor cell's base: 1.0 for a full cube (or water/air — feet at the
/// cell base), a partial floor's highest box top otherwise (a slab-top
/// foothold stands half a block below its cell base).
fn floor_top(world: &World, floor: IVec3) -> f32 {
    let boxes = world.collision_boxes_at(floor.x, floor.y, floor.z);
    if boxes.is_empty() {
        return 1.0;
    }
    boxes
        .iter()
        .fold(0.0f32, |acc, b| acc.max(b.max[1]))
        .clamp(0.0, 1.0)
}

/// The accurate per-edge movement gate: accepts a step between two footholds
/// only when the mob's REAL body AABB (its true half-width and height, feet at
/// the true floor height) can sweep that move through the real collision boxes
/// of every PARTIAL cell it crosses — with the ordinary [`collision::STEP_HEIGHT`]
/// allowance, so a low shape (a slab lying in the way) is stepped over while a
/// tall thin one (a ladder panel, a pane, a closed door) blocks exactly where
/// it physically blocks. The destination pose is checked too, so a jump-up
/// into a partial shape is refused instead of bonked into.
///
/// Cells with no partial collision cost one memoized classification each, so
/// over plain terrain the gate is a cheap table lookup and the sweep only runs
/// where partial shapes actually are. Shared with `mob::confined`, whose
/// reachability fill must agree with the routes this gate admits (a lone
/// fence refuses the jump from below; a step beside it opens the way over).
pub(super) fn partial_step_gate<'w>(
    world: &'w World,
    params: PathParams,
    height: f32,
) -> impl Fn(IVec3, IVec3) -> bool + 'w {
    let cache: RefCell<FxHashMap<IVec3, &'static [Aabb]>> = RefCell::new(FxHashMap::default());
    let height = height.max(0.5);
    move |from: IVec3, to: IVec3| {
        let dx = (to.x - from.x) as f32;
        let dz = (to.z - from.z) as f32;
        if dx == 0.0 && dz == 0.0 {
            return true;
        }
        let boxes_at = |x: i32, y: i32, z: i32| -> &'static [Aabb] {
            *cache
                .borrow_mut()
                .entry(IVec3::new(x, y, z))
                .or_insert_with(|| nav_partial_boxes(world, IVec3::new(x, y, z)))
        };
        // Fast path: no partial shape anywhere the body could touch during this
        // step (both columns, floor through head, padded for wide bodies).
        let diagonal = dx != 0.0 && dz != 0.0;
        let half_width = params.half_width.max(0.0);
        let pad = ((half_width - 0.5).max(0.0)).ceil() as i32;
        let body_y = from.y.min(to.y);
        let y_lo = body_y - 1;
        let y_hi = from.y.max(to.y) + params.head_cells();
        let mut any_partial = false;
        'scan: for x in (from.x.min(to.x) - pad)..=(from.x.max(to.x) + pad) {
            for z in (from.z.min(to.z) - pad)..=(from.z.max(to.z) + pad) {
                for y in y_lo..=y_hi {
                    if !boxes_at(x, y, z).is_empty() {
                        // A DIAGONAL step near a partial shape at body level is
                        // refused outright: the sweep below is axis-ordered
                        // (X then Z), which can clear an L-shaped path while
                        // the TRUE straight diagonal the mob walks clips the
                        // shape's corner — the walking-against-a-trough bug.
                        // Cardinal detours around the shape stay available
                        // (and are what a watching player expects to see).
                        // Partial FLOORS (a slab underfoot) don't trigger
                        // this; only boxes the body itself could touch do.
                        if diagonal && y >= body_y {
                            return false;
                        }
                        any_partial = true;
                        if !diagonal {
                            break 'scan;
                        }
                    }
                }
            }
        }
        if !any_partial {
            return true;
        }

        // Accurate sweep: the body starts standing at `from` (feet on the real
        // floor top) and must travel the full horizontal move.
        let feet = (from.y - 1) as f32 + floor_top(world, from - IVec3::Y);
        let cx = from.x as f32 + 0.5;
        let cz = from.z as f32 + 0.5;
        let min = [cx - half_width, feet, cz - half_width];
        let max = [cx + half_width, feet + height, cz + half_width];
        let (moved, _, _) =
            collision::step_horizontal(min, max, dx, dz, collision::STEP_HEIGHT, boxes_at);
        if (moved[0] - dx).abs() >= 1e-3 || (moved[2] - dz).abs() >= 1e-3 {
            return false;
        }

        // Destination pose: standing at `to` must not intersect a partial shape
        // (the sweep runs at `from`'s level, so a jump-up's landing pose needs
        // its own check).
        let dest_feet = (to.y - 1) as f32 + floor_top(world, to - IVec3::Y);
        let tx = to.x as f32 + 0.5;
        let tz = to.z as f32 + 0.5;
        let dmin = [tx - half_width, dest_feet + 1e-3, tz - half_width];
        let dmax = [tx + half_width, dest_feet + height, tz + half_width];
        !collision::aabb_hits_cells(dmin, dmax, boxes_at)
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
    let origin = Vec3::new(start.x as f32 + 0.5, start.y as f32, start.z as f32 + 0.5);
    let mut mark = |min: Vec3, max: Vec3| {
        let centre = (min + max) * 0.5;
        let (ddx, ddz) = (centre.x - origin.x, centre.z - origin.z);
        if ddx * ddx + ddz * ddz > ENTITY_AVOID_RANGE * ENTITY_AVOID_RANGE {
            return;
        }
        for x in (min.x.floor() as i32)..=(max.x.floor() as i32) {
            for y in (min.y.floor() as i32)..=(max.y.floor() as i32) {
                for z in (min.z.floor() as i32)..=(max.z.floor() as i32) {
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
        let half = s.half_length.unwrap_or(s.half_width).max(s.half_width);
        mark(
            m.pos - Vec3::new(half, 0.0, half),
            m.pos + Vec3::new(half, s.height, half),
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
mod tests {
    use super::*;
    use crate::block::Block;
    use crate::facing::Facing;

    #[test]
    fn idle_until_given_a_goal() {
        let nav = Navigator::new(1, 0.25, 0.9);
        assert!(nav.is_idle());
    }

    #[test]
    fn arriving_consumes_waypoints_then_goes_idle() {
        let mut nav = Navigator::new(1, 0.25, 0.9);
        // Hand-build a 2-step path so we don't need a World: start (0,1,0) -> (1,1,0).
        nav.path = vec![IVec3::new(0, 1, 0), IVec3::new(1, 1, 0)];
        nav.index = 1;
        nav.goal = Some(IVec3::new(1, 1, 0));
        nav.path_reaches_goal = true;
        // Standing on the waypoint: it's consumed and the nav goes idle.
        let on_wp = Vec3::new(1.5, 1.0, 0.5);
        let (wish, jump) = nav.follow(on_wp, true);
        assert_eq!(wish, Vec3::ZERO);
        assert!(!jump);
        assert!(nav.is_idle(), "arrived -> idle");
    }

    #[test]
    fn steers_toward_a_distant_waypoint() {
        let mut nav = Navigator::new(1, 0.25, 0.9);
        nav.path = vec![IVec3::new(0, 1, 0), IVec3::new(5, 1, 0)];
        nav.index = 1;
        nav.goal = Some(IVec3::new(5, 1, 0));
        let (wish, jump) = nav.follow(Vec3::new(0.5, 1.0, 0.5), true);
        assert!(wish.x > 0.9, "heads +X toward the waypoint: {wish:?}");
        assert!(!jump, "flat move needs no jump");
    }

    #[test]
    fn jumps_when_close_to_a_step_up() {
        let mut nav = Navigator::new(1, 0.22, 0.9);
        // Waypoint one block up and just ahead.
        nav.path = vec![IVec3::new(0, 1, 0), IVec3::new(1, 2, 0)];
        nav.index = 1;
        nav.goal = Some(IVec3::new(1, 2, 0));
        let (_wish, jump) = nav.follow(Vec3::new(0.7, 1.0, 0.5), true);
        assert!(jump, "should jump for a nearby one-block step up");
        // But not while airborne.
        let (_w2, jump_air) = nav.follow(Vec3::new(0.7, 1.0, 0.5), false);
        assert!(!jump_air, "no jump while off the ground");
    }

    #[test]
    fn vertical_bobbing_does_not_count_as_navigation_progress() {
        let mut nav = Navigator::new(1, 0.25, 0.9);
        nav.path = vec![IVec3::new(0, 1, 0), IVec3::new(5, 1, 0)];
        nav.index = 1;
        nav.goal = Some(IVec3::new(5, 1, 0));
        nav.last_pos = Vec3::new(0.5, 1.0, 0.5);

        for tick in 0..STUCK_TICKS {
            let y = 1.0 + if tick % 2 == 0 { 0.2 } else { -0.2 };
            let (wish, _jump) = nav.follow(Vec3::new(0.5, y, 0.5), false);
            if tick + 1 < STUCK_TICKS {
                assert!(wish.x > 0.9, "still trying to move horizontally");
                assert!(!nav.is_idle(), "not abandoned before the stuck limit");
            }
        }

        assert!(
            nav.is_idle(),
            "bobbing in place should abandon the bad route so wander can choose again"
        );
    }

    #[test]
    fn jump_trigger_accounts_for_body_width() {
        let mut nav = Navigator::new(1, 0.45, 0.9);
        // Waypoint one block up in the adjacent cell. A wide mob standing in the lower
        // cell is already close to the ledge with its front edge even though its centre
        // is still a full block from the target centre.
        nav.path = vec![IVec3::new(0, 1, 0), IVec3::new(1, 2, 0)];
        nav.index = 1;
        nav.goal = Some(IVec3::new(1, 2, 0));
        let (_wish, jump) = nav.follow(Vec3::new(0.5, 1.0, 0.5), true);
        assert!(
            jump,
            "wider bodies jump before colliding with the step face"
        );
    }

    #[test]
    fn wide_mob_does_not_turn_before_clearing_a_corner() {
        let mut nav = Navigator::new(1, 0.45, 0.9);
        // The route turns north at (1,1,0). A sheep-width body at x=1.25 would still
        // clip a block in the inner corner if it started the turn, so it must keep
        // steering east until much closer to the waypoint centre.
        nav.path = vec![
            IVec3::new(0, 1, 0),
            IVec3::new(1, 1, 0),
            IVec3::new(1, 1, 1),
        ];
        nav.index = 1;
        nav.goal = Some(IVec3::new(1, 1, 1));
        let (wish, jump) = nav.follow(Vec3::new(1.25, 1.0, 0.5), true);
        assert!(
            wish.x > 0.9 && wish.z.abs() < 0.1,
            "wide mob should keep clearing the corner before turning: {wish:?}"
        );
        assert!(!jump);
    }

    /// A single chunk with a solid grass floor at `y = 63`, so footholds sit at
    /// `y = 64` across it — enough terrain for `find_path` to route over.
    fn flat_world() -> World {
        use crate::chunk::{Chunk, ChunkPos};
        let mut world = World::new(0, 2);
        world.insert_chunk_for_test(ChunkPos::new(0, 0), Chunk::new(0, 0));
        for x in 0..12 {
            for z in 0..4 {
                world.set_block_world(x, 63, z, Block::Grass);
            }
        }
        world
    }

    fn pillar_world() -> (World, IVec3, IVec3) {
        let mut world = flat_world();
        let start = IVec3::new(1, 64, 1);
        let goal = IVec3::new(8, 66, 1);
        world.set_block_world(goal.x, 64, goal.z, Block::Stone);
        world.set_block_world(goal.x, 65, goal.z, Block::Stone);
        (world, start, goal)
    }

    fn world_with_door_in_wall(open: bool) -> (World, IVec3, IVec3, IVec3) {
        let mut world = flat_world();
        let door = IVec3::new(4, 64, 1);
        for x in 0..12 {
            if x == door.x {
                continue;
            }
            world.set_block_world(x, 64, door.z, Block::Stone);
            world.set_block_world(x, 65, door.z, Block::Stone);
        }
        assert!(world.place_door(door, Block::OakDoor, Facing::South));
        if open {
            assert_eq!(world.toggle_door(door), Some(door));
        }
        (world, IVec3::new(4, 64, 0), IVec3::new(4, 64, 2), door)
    }

    #[test]
    fn closed_door_blocks_the_crossing_edge() {
        let (world, start, goal, door) = world_with_door_in_wall(false);
        let mut nav = Navigator::new(1, 0.25, 0.9);

        nav.update_goal_when_supported(Some(goal), start, &world, true, &NavObstacles::none());

        assert_ne!(
            nav.path().last(),
            Some(&goal),
            "closed door must not route through the wall opening: {:?}",
            nav.path()
        );
        assert!(
            nav.path().last().is_some_and(|p| p.z <= door.z),
            "partial route should stop on the near side or in the door cell: {:?}",
            nav.path()
        );
    }

    #[test]
    fn open_door_allows_the_cleared_crossing_edge() {
        let (world, start, goal, door) = world_with_door_in_wall(true);
        let mut nav = Navigator::new(1, 0.25, 0.9);

        nav.update_goal_when_supported(Some(goal), start, &world, true, &NavObstacles::none());

        assert_eq!(nav.path().last(), Some(&goal), "open door is routeable");
        assert!(
            nav.path().contains(&door),
            "the route should pass through the door cell: {:?}",
            nav.path()
        );
    }

    #[test]
    fn open_door_still_blocks_the_swung_edge() {
        let mut world = flat_world();
        let door = IVec3::new(4, 64, 1);
        assert!(world.place_door(door, Block::OakDoor, Facing::South));
        assert_eq!(world.toggle_door(door), Some(door));
        let start = IVec3::new(3, 64, 1);
        let goal = IVec3::new(5, 64, 1);
        let mut nav = Navigator::new(1, 0.25, 0.9);

        nav.update_goal_when_supported(Some(goal), start, &world, true, &NavObstacles::none());

        assert_eq!(nav.path().last(), Some(&goal), "a detour remains possible");
        assert!(
            !nav.path()
                .windows(2)
                .any(|w| w[0] == start && w[1] == door),
            "the open door's swung slab sits on the west edge, so the route must not enter straight from the west: {:?}",
            nav.path()
        );
    }

    #[test]
    fn a_ladder_panel_blocks_only_the_edge_it_physically_blocks() {
        // The regression this rework exists for: a ladder's block row has NO
        // collision (its 1/16 panel resolves per-facing at the world level), so
        // cell navigation used to read its cell as fully open and walk mobs
        // straight into the panel forever. The edge gate sweeps the real body
        // AABB: crossing the panel's face is refused, while the open 15/16 of
        // the same cell stays routable.
        let mut world = flat_world();
        let ladder = IVec3::new(4, 64, 1);
        for x in 0..12 {
            if x == ladder.x {
                continue;
            }
            world.set_block_world(x, 64, ladder.z, Block::Stone);
            world.set_block_world(x, 65, ladder.z, Block::Stone);
        }
        // `Block::Ladder` faces north: its panel hugs the z = 2 face of its cell.
        world.set_block_world(ladder.x, ladder.y, ladder.z, Block::Ladder);
        let start = IVec3::new(4, 64, 0);
        let mut nav = Navigator::new(1, 0.25, 0.9);

        // Entering the ladder cell from the open side is fine — the cell is NOT
        // a blanket wall.
        nav.update_goal_when_supported(Some(ladder), start, &world, true, &NavObstacles::none());
        assert_eq!(
            nav.path().last(),
            Some(&ladder),
            "the open 15/16 of a ladder cell stays routable: {:?}",
            nav.path()
        );

        // Crossing the panel's face is refused: the far side is unreachable and
        // the best-effort route never steps through the panel.
        let mut nav = Navigator::new(1, 0.25, 0.9);
        let goal = IVec3::new(4, 64, 2);
        nav.update_goal_when_supported(Some(goal), start, &world, true, &NavObstacles::none());
        assert_ne!(
            nav.path().last(),
            Some(&goal),
            "the mob must not plan through the ladder panel: {:?}",
            nav.path()
        );
        assert!(
            nav.path().iter().all(|c| c.z <= ladder.z),
            "the best-effort route stops on the near side of the panel: {:?}",
            nav.path()
        );
    }

    #[test]
    fn a_touching_body_ahead_veers_the_wish_to_a_side() {
        let pos = Vec3::new(0.5, 64.0, 0.5);
        let wish = Vec3::new(1.0, 0.0, 0.0);
        let blocking = [AiMob {
            id: 2,
            kind: crate::mob::Mob::Sheep,
            pos: Vec3::new(1.4, 64.0, 0.5),
            active: true,
            tags: Default::default(),
        }];
        let contacts = [EntityRef::Mob(2)];

        // Dead ahead: the wish veers sideways (id-picked side), keeping forward speed.
        let out = Unstick::default().steer(wish, pos, 1, 0.45, &contacts, None, &blocking, &[]);
        assert!(out.z.abs() > 0.5, "veers sideways around the body: {out:?}");
        assert!(out.x > 0.0, "keeps forward progress: {out:?}");

        // No contact (and no running commitment): the wish passes through untouched.
        assert_eq!(
            Unstick::default().steer(wish, pos, 1, 0.45, &[], None, &blocking, &[]),
            wish
        );

        // The brain's target is never dodged.
        assert_eq!(
            Unstick::default().steer(
                wish,
                pos,
                1,
                0.45,
                &contacts,
                Some(EntityRef::Mob(2)),
                &blocking,
                &[]
            ),
            wish
        );

        // A body BEHIND the travel direction is not ours to dodge.
        let behind = [AiMob {
            id: 2,
            kind: crate::mob::Mob::Sheep,
            pos: Vec3::new(-0.4, 64.0, 0.5),
            active: true,
            tags: Default::default(),
        }];
        assert_eq!(
            Unstick::default().steer(wish, pos, 1, 0.45, &contacts, None, &behind, &[]),
            wish
        );
    }

    #[test]
    fn the_veer_commits_to_its_side_instead_of_flip_flopping() {
        // Contacts are recorded from the PREVIOUS tick's overlap, so a veer
        // that works erases its own trigger next tick. The latch must keep
        // rounding the peer on the SAME side while the commitment runs down —
        // the stateless version snapped straight, re-pressed, and wagged the
        // wish (and the body facing) every other tick.
        let pos = Vec3::new(0.5, 64.0, 0.5);
        let wish = Vec3::new(1.0, 0.0, 0.0);
        let blocking = [AiMob {
            id: 2,
            kind: crate::mob::Mob::Sheep,
            pos: Vec3::new(1.4, 64.0, 0.5),
            active: true,
            tags: Default::default(),
        }];
        let contacts = [EntityRef::Mob(2)];

        let mut latch = Unstick::default();
        let veered = latch.steer(wish, pos, 1, 0.45, &contacts, None, &blocking, &[]);
        assert!(veered.z.abs() > 0.5, "the contact tick veers: {veered:?}");

        // Contact gone: the commitment keeps the SAME veer, no snap-back.
        for _ in 0..UNSTICK_HOLD_TICKS {
            assert_eq!(
                latch.steer(wish, pos, 1, 0.45, &[], None, &blocking, &[]),
                veered,
                "the committed side holds through contact flicker"
            );
        }
        // Commitment exhausted: the wish runs straight again.
        assert_eq!(
            latch.steer(wish, pos, 1, 0.45, &[], None, &blocking, &[]),
            wish,
            "the veer expires once the contact has stayed gone"
        );

        // A body slightly to the OTHER side of the new travel line must not
        // flip the committed side while the commitment is live.
        let mut latch = Unstick::default();
        let first = latch.steer(wish, pos, 1, 0.45, &contacts, None, &blocking, &[]);
        let other_side = [AiMob {
            pos: Vec3::new(1.3, 64.0, 0.4 - first.z.signum() * 0.2),
            ..blocking[0].clone()
        }];
        let second = latch.steer(wish, pos, 1, 0.45, &contacts, None, &other_side, &[]);
        assert_eq!(
            first.z.signum(),
            second.z.signum(),
            "a live commitment pins the veer side: {first:?} vs {second:?}"
        );
    }

    #[test]
    fn a_one_high_block_wall_is_routable_but_a_one_high_fence_wall_is_not() {
        // Ordinary one-block steps stay jumpable; a fence of the same height
        // must never route, or no fenced pen would hold (the by-design pen
        // rule in `nav_solid_fn`/`nav_support_fn`).
        let start = IVec3::new(4, 64, 0);
        let goal = IVec3::new(4, 64, 2);

        let mut stone_world = flat_world();
        for x in 0..12 {
            stone_world.set_block_world(x, 64, 1, Block::Stone);
        }
        let mut nav = Navigator::new(1, 0.25, 0.9);
        nav.update_goal_when_supported(Some(goal), start, &stone_world, true, &NavObstacles::none());
        assert_eq!(
            nav.path().last(),
            Some(&goal),
            "a one-block stone step stays routable: {:?}",
            nav.path()
        );

        let mut fence_world = flat_world();
        for x in 0..12 {
            fence_world.set_block_world(x, 64, 1, Block::OakFence);
        }
        let mut nav = Navigator::new(1, 0.25, 0.9);
        nav.update_goal_when_supported(Some(goal), start, &fence_world, true, &NavObstacles::none());
        assert_ne!(
            nav.path().last(),
            Some(&goal),
            "a one-high fence wall must not be routable: {:?}",
            nav.path()
        );
        assert!(
            nav.path().iter().all(|c| c.z < 1),
            "the best-effort route stops on the near side of the fence: {:?}",
            nav.path()
        );
    }

    #[test]
    fn mob_can_reach_answers_the_fence_honestly() {
        // The `MobCanReach` HostCall's engine seam: a cell beyond a fence
        // line is not reachable, a cell on the mob's own side is — the
        // honesty gate mod destination policies (grazing) build on.
        let mut world = flat_world();
        for x in 0..12 {
            world.set_block_world(x, 64, 1, Block::OakFence);
        }
        let mob = crate::mob::Instance::new(
            crate::mob::Mob::Sheep,
            Vec3::new(4.5, 64.0, 0.5),
            0.0,
            1,
        );
        assert!(
            !mob_can_reach(&world, &mob, IVec3::new(4, 64, 2)),
            "grass beyond the fence is not a reachable destination"
        );
        assert!(
            mob_can_reach(&world, &mob, IVec3::new(8, 64, 0)),
            "a cell on the mob's own side is reachable"
        );
    }

    #[test]
    fn a_step_beside_the_fence_opens_the_route_over_it() {
        // The pen rule's honest exception: with a block placed in front of the
        // fence, the mob may jump onto it and walk over the fence top.
        let mut world = flat_world();
        for x in 0..12 {
            world.set_block_world(x, 64, 1, Block::OakFence);
        }
        world.set_block_world(4, 64, 0, Block::Dirt);
        // The mob starts beside the step on the ground, goal beyond the fence.
        let start = IVec3::new(3, 64, 0);
        let goal = IVec3::new(4, 64, 2);
        let mut nav = Navigator::new(1, 0.25, 0.9);
        nav.update_goal_when_supported(Some(goal), start, &world, true, &NavObstacles::none());
        assert_eq!(
            nav.path().last(),
            Some(&goal),
            "the route should go over the fence via the step: {:?}",
            nav.path()
        );
        assert!(
            nav.path().contains(&IVec3::new(4, 65, 1)),
            "the route walks the fence top: {:?}",
            nav.path()
        );
    }

    #[test]
    fn an_offset_body_deflects_along_a_partial_shapes_face_instead_of_pressing_in() {
        // The walking-against-the-trough bug: a sheep that wandered flush
        // against a partial-collision block (here a chest) gets a waypoint
        // past it. The raw wish from its OFFSET position presses the body
        // diagonally into the shape; steered following must drop the blocked
        // axis and walk cleanly along the face instead.
        let mut world = flat_world();
        world.set_block_world(4, 64, 1, Block::Chest);
        let mut nav = Navigator::new(2, 0.45, 1.4);
        nav.path = vec![IVec3::new(3, 64, 0), IVec3::new(5, 64, 0)];
        nav.index = 1;
        nav.goal = Some(IVec3::new(5, 64, 0));
        nav.path_reaches_goal = true;
        // Body centre at z = 0.95: its 0.9-wide body overlaps the chest's
        // row, so heading straight east grinds into the chest's west face.
        let pos = Vec3::new(3.5, 64.0, 0.95);

        let (raw, _) = nav.follow(pos, true);
        assert!(raw.x > 0.9, "the raw wish presses east into the chest: {raw:?}");

        let mut nav = Navigator::new(2, 0.45, 1.4);
        nav.path = vec![IVec3::new(3, 64, 0), IVec3::new(5, 64, 0)];
        nav.index = 1;
        nav.goal = Some(IVec3::new(5, 64, 0));
        nav.path_reaches_goal = true;
        let (wish, jump) = nav.follow_steered(pos, true, &world);
        assert!(!jump);
        assert!(
            wish.x.abs() < 0.05 && wish.z < -0.9,
            "steering drops the blocked axis and clears the chest's row first: {wish:?}"
        );
    }

    #[test]
    fn steering_never_deflects_the_final_approach_to_a_wall_adjacent_waypoint() {
        // The probe is capped at the remaining distance: a waypoint whose far
        // side is a wall must still be walked INTO the arrive window, or mobs
        // hover forever one body-length short of wall-adjacent destinations.
        let mut world = flat_world();
        world.set_block_world(5, 64, 0, Block::Stone);
        world.set_block_world(5, 65, 0, Block::Stone);
        let mut nav = Navigator::new(2, 0.45, 1.4);
        nav.path = vec![IVec3::new(3, 64, 0), IVec3::new(4, 64, 0)];
        nav.index = 1;
        nav.goal = Some(IVec3::new(4, 64, 0));
        nav.path_reaches_goal = true;
        let (wish, _) = nav.follow_steered(Vec3::new(4.42, 64.0, 0.5), true, &world);
        assert!(
            wish.x > 0.9,
            "the final approach keeps closing on the wall-adjacent centre: {wish:?}"
        );
    }

    #[test]
    fn no_diagonal_step_cuts_past_a_partial_shapes_corner() {
        // The gate's sweep is axis-ordered, but a mob walks a diagonal as a
        // straight line — which can clip a partial shape's corner the L-shaped
        // sweep cleared. Diagonals near body-level partial shapes are refused
        // outright, so the route around a trough/chest is honest cardinal
        // steps a real body can walk.
        let mut world = flat_world();
        world.set_block_world(4, 64, 1, Block::Chest);
        let start = IVec3::new(3, 64, 1);
        let goal = IVec3::new(4, 64, 0);
        let mut nav = Navigator::new(2, 0.45, 1.4);
        nav.update_goal_when_supported(Some(goal), start, &world, true, &NavObstacles::none());
        assert_eq!(nav.path().last(), Some(&goal), "the goal stays reachable");
        assert!(
            nav.path()
                .windows(2)
                .all(|w| (w[1].x - w[0].x).abs() + (w[1].z - w[0].z).abs() <= 1),
            "no diagonal step beside the chest — cardinal detour only: {:?}",
            nav.path()
        );
    }

    #[test]
    fn routes_bend_around_another_mobs_body() {
        let world = flat_world();
        let start = IVec3::new(1, 64, 1);
        let goal = IVec3::new(8, 64, 1);
        let blocker_cell = IVec3::new(4, 64, 1);
        let mobs = [AiMob {
            id: 7,
            kind: crate::mob::Mob::Sheep,
            pos: Vec3::new(4.5, 64.0, 1.5),
            active: true,
            tags: Default::default(),
        }];
        let obstacles = NavObstacles {
            self_id: 1,
            target: None,
            mobs: &mobs,
            players: &[],
        };
        let mut nav = Navigator::new(1, 0.25, 0.9);
        nav.update_goal_when_supported(Some(goal), start, &world, true, &obstacles);
        assert_eq!(nav.path().last(), Some(&goal), "still reaches the goal");
        assert!(
            !nav.path().contains(&blocker_cell),
            "the route bends around the standing mob: {:?}",
            nav.path()
        );
    }

    #[test]
    fn a_blocked_corridor_is_rounded_rather_than_pushed_through() {
        // A 1-wide corridor with a sheep standing in it and a gap in one wall
        // before AND after her: squeezing past her body costs one crowded cell
        // (200), the gap detour costs a handful of flat steps (~40). The route
        // must take the gap, not the shove.
        let mut world = flat_world();
        for x in 0..12 {
            if x != 3 && x != 5 {
                world.set_block_world(x, 64, 0, Block::Stone);
            }
            world.set_block_world(x, 64, 2, Block::Stone);
        }
        let start = IVec3::new(1, 64, 1);
        let goal = IVec3::new(8, 64, 1);
        let blocker_cell = IVec3::new(4, 64, 1);
        let mobs = [AiMob {
            id: 7,
            kind: crate::mob::Mob::Sheep,
            pos: Vec3::new(4.5, 64.0, 1.5),
            active: true,
            tags: Default::default(),
        }];
        let obstacles = NavObstacles {
            self_id: 1,
            target: None,
            mobs: &mobs,
            players: &[],
        };
        let mut nav = Navigator::new(1, 0.25, 0.9);
        nav.update_goal_when_supported(Some(goal), start, &world, true, &obstacles);
        assert_eq!(nav.path().last(), Some(&goal), "still reaches the goal");
        assert!(
            !nav.path().contains(&blocker_cell),
            "the route rounds the corridor through the gaps: {:?}",
            nav.path()
        );
    }

    #[test]
    fn a_truly_blocked_crowd_still_resolves_by_paying_the_cost() {
        // The other half of the contract: no detour at all (sealed corridor)
        // must never deadlock the search — the mob squeezes through as a last
        // resort instead of freezing.
        let mut world = flat_world();
        for x in 0..12 {
            world.set_block_world(x, 64, 0, Block::Stone);
            world.set_block_world(x, 64, 2, Block::Stone);
        }
        let start = IVec3::new(1, 64, 1);
        let goal = IVec3::new(8, 64, 1);
        let mobs = [AiMob {
            id: 7,
            kind: crate::mob::Mob::Sheep,
            pos: Vec3::new(4.5, 64.0, 1.5),
            active: true,
            tags: Default::default(),
        }];
        let obstacles = NavObstacles {
            self_id: 1,
            target: None,
            mobs: &mobs,
            players: &[],
        };
        let mut nav = Navigator::new(1, 0.25, 0.9);
        nav.update_goal_when_supported(Some(goal), start, &world, true, &obstacles);
        assert_eq!(
            nav.path().last(),
            Some(&goal),
            "a packed corridor still resolves by squeezing through: {:?}",
            nav.path()
        );
    }

    #[test]
    fn the_locked_target_is_never_avoided() {
        // A zombie chasing prey must path TO it, not around it: the current
        // target is exempt from entity avoidance.
        let world = flat_world();
        let start = IVec3::new(1, 64, 1);
        let goal = IVec3::new(8, 64, 1);
        let on_the_way = IVec3::new(4, 64, 1);
        let mobs = [AiMob {
            id: 7,
            kind: crate::mob::Mob::Sheep,
            pos: Vec3::new(4.5, 64.0, 1.5),
            active: true,
            tags: Default::default(),
        }];
        let obstacles = NavObstacles {
            self_id: 1,
            target: Some(EntityRef::Mob(7)),
            mobs: &mobs,
            players: &[],
        };
        let mut nav = Navigator::new(1, 0.25, 0.9);
        nav.update_goal_when_supported(Some(goal), start, &world, true, &obstacles);
        assert!(
            nav.path().contains(&on_the_way),
            "the targeted mob's cells cost nothing extra: {:?}",
            nav.path()
        );
    }

    #[test]
    fn players_are_soft_obstacles_unless_targeted() {
        let world = flat_world();
        let start = IVec3::new(1, 64, 1);
        let goal = IVec3::new(8, 64, 1);
        let player_cell = IVec3::new(4, 64, 1);
        let anchor = PlayerAnchor {
            body: Some(crate::body::Body::new(Vec3::new(4.5, 64.0, 1.5), 0.3, 1.8)),
            ..Default::default()
        };
        let players = [anchor];

        let mut nav = Navigator::new(1, 0.25, 0.9);
        let avoid = NavObstacles {
            self_id: 1,
            target: None,
            mobs: &[],
            players: &players,
        };
        nav.update_goal_when_supported(Some(goal), start, &world, true, &avoid);
        assert_eq!(nav.path().last(), Some(&goal));
        assert!(
            !nav.path().contains(&player_cell),
            "an untargeted player is routed around: {:?}",
            nav.path()
        );

        let mut nav = Navigator::new(1, 0.25, 0.9);
        let chase = NavObstacles {
            self_id: 1,
            target: Some(EntityRef::Player(players[0].id)),
            mobs: &[],
            players: &players,
        };
        nav.update_goal_when_supported(Some(goal), start, &world, true, &chase);
        assert!(
            nav.path().contains(&player_cell),
            "the hunted player is pathed TO, not around: {:?}",
            nav.path()
        );
    }

    #[test]
    fn re_paths_a_held_goal_when_the_world_changes() {
        // Hold one goal while the world changes underneath: the navigator must keep the
        // first route until REPATH_TICKS elapse, then refresh it to route around new
        // terrain — the whole point of periodic re-pathing.
        let mut world = flat_world();
        let start = IVec3::new(1, 64, 1);
        let goal = IVec3::new(8, 64, 1);
        let mut nav = Navigator::new(1, 0.25, 0.9);

        // Initial path: a straight run along z = 1, passing through (4, 64, 1).
        nav.update_goal_when_supported(Some(goal), start, &world, true, &NavObstacles::none());
        let stale: Vec<IVec3> = nav.path().to_vec();
        let blocked = IVec3::new(4, 64, 1);
        assert!(
            stale.contains(&blocked),
            "the open route runs straight through {blocked:?}"
        );

        // Drop a 2-high wall across that cell (its foothold + the cell above it), so the
        // straight route is no longer walkable — a detour via z = 0 / z = 2 remains.
        world.set_block_world(blocked.x, blocked.y, blocked.z, Block::Stone);
        world.set_block_world(blocked.x, blocked.y + 1, blocked.z, Block::Stone);

        // For the first REPATH_TICKS-1 held ticks the stale route is kept verbatim (no
        // per-tick re-pathing — holding a goal stays cheap).
        for _ in 0..REPATH_TICKS - 1 {
            nav.update_goal_when_supported(Some(goal), start, &world, true, &NavObstacles::none());
            assert_eq!(
                nav.path(),
                stale.as_slice(),
                "no re-path before the interval elapses"
            );
        }

        // The REPATH_TICKS-th held tick refreshes the route, which now avoids the wall.
        nav.update_goal_when_supported(Some(goal), start, &world, true, &NavObstacles::none());
        assert_ne!(
            nav.path(),
            stale.as_slice(),
            "re-paths once the interval elapses"
        );
        assert!(
            !nav.path().contains(&blocked),
            "the refreshed route avoids the newly-walled cell: {:?}",
            nav.path()
        );
    }

    #[test]
    fn fast_wide_mob_consumes_overflown_waypoints_instead_of_orbiting() {
        // The hushjaw jitter regression: half_width 0.45 tightens arrive_xz to
        // 0.05 m while 4.8 m/s walks 0.24 m per tick — the mob can overfly a
        // waypoint it can never land inside. Overshoot consumption must let it
        // walk a straight route with ZERO 180° turn-backs; without it this
        // exact setup orbits the first waypoint forever (measured: 1990
        // reversals in 2000 ticks).
        let mut nav = Navigator::new(2, 0.45, 1.4);
        nav.path = (0..=8).map(|x| IVec3::new(x, 1, 0)).collect();
        nav.index = 1;
        nav.goal = Some(IVec3::new(8, 1, 0));
        nav.path_reaches_goal = true;

        let step = 4.8 * 0.05; // hushjaw speed × tick dt
        let mut pos = Vec3::new(0.5, 1.0, 0.5);
        let mut last_dir: Option<Vec3> = None;
        let mut reversals = 0;
        for _ in 0..200 {
            let (wish, _jump) = nav.follow(pos, true);
            if wish == Vec3::ZERO {
                break;
            }
            if let Some(prev) = last_dir {
                if wish.x * prev.x + wish.z * prev.z < -0.5 {
                    reversals += 1;
                }
            }
            last_dir = Some(wish);
            pos += wish * step;
        }

        assert!(
            nav.is_idle(),
            "the straight 8-cell route completes within the tick budget"
        );
        assert_eq!(
            reversals, 0,
            "no 180° turn-backs while following a straight route at speed"
        );
    }

    #[test]
    fn overshoot_does_not_consume_a_waypoint_still_being_approached() {
        // The wide-corner contract's counterpart: while the distance to the
        // waypoint is still SHRINKING, overshoot must not fire — a wide mob
        // keeps clearing the corner exactly as before.
        let mut nav = Navigator::new(1, 0.45, 0.9);
        nav.path = vec![
            IVec3::new(0, 1, 0),
            IVec3::new(1, 1, 0),
            IVec3::new(1, 1, 1),
        ];
        nav.index = 1;
        nav.goal = Some(IVec3::new(1, 1, 1));
        // Two approaching ticks toward the corner waypoint (1,1,0): both must
        // keep steering east at it, not consume it early.
        for x in [1.1_f32, 1.25] {
            let (wish, _) = nav.follow(Vec3::new(x, 1.0, 0.5), true);
            assert!(
                wish.x > 0.9 && wish.z.abs() < 0.1,
                "still clearing the corner at x={x}: {wish:?}"
            );
        }
    }

    #[test]
    fn changed_goal_repath_preserves_the_current_waypoint_when_still_valid() {
        // A chased target crossing a cell boundary CHANGES the goal several
        // times a second. The refresh must keep steering at the waypoint the
        // mob is mid-stride toward (when it remains a valid immediate step),
        // not snap laterally between equal-cost first steps.
        let world = flat_world();
        let start = IVec3::new(1, 64, 1);
        let old_waypoint = IVec3::new(2, 64, 1);
        let first_goal = IVec3::new(2, 64, 3);
        let moved_goal = IVec3::new(3, 64, 3);

        let mut nav = Navigator::new(1, 0.25, 0.9);
        nav.path = vec![start, old_waypoint, IVec3::new(2, 64, 2), first_goal];
        nav.index = 1;
        nav.goal = Some(first_goal);
        nav.path_reaches_goal = true;

        nav.update_goal_when_supported(Some(moved_goal), start, &world, true, &NavObstacles::none());
        assert_eq!(
            nav.path().get(1),
            Some(&old_waypoint),
            "a changed goal keeps the in-progress step when still valid"
        );
        assert_eq!(
            nav.path().last(),
            Some(&moved_goal),
            "and the preserved route still reaches the new goal"
        );
    }

    #[test]
    fn same_goal_repath_preserves_the_current_waypoint_when_still_valid() {
        // A periodic refresh can find an equally-good path whose first step is a
        // different lateral cell. The mob should not snap sideways on the refresh tick
        // if the waypoint it was already walking toward is still a valid immediate step.
        let world = flat_world();
        let start = IVec3::new(1, 64, 1);
        let old_waypoint = IVec3::new(2, 64, 1);
        let goal = IVec3::new(2, 64, 3);
        let params = PathParams::for_body(1, 0.25);
        let direct = path::find_path(
            start,
            goal,
            params,
            |c| world.blocks_movement_at(c.x, c.y, c.z),
            |c| world.water_cell_at(c.x, c.y, c.z),
        );
        assert_ne!(
            direct.get(1),
            Some(&old_waypoint),
            "fixture must have a direct refresh route that would pick a different first step"
        );

        let mut nav = Navigator::new(1, 0.25, 0.9);
        nav.path = vec![start, old_waypoint, IVec3::new(2, 64, 2), goal];
        nav.index = 1;
        nav.goal = Some(goal);
        nav.path_reaches_goal = true;
        nav.since_path = REPATH_TICKS - 1;

        nav.update_goal_when_supported(Some(goal), start, &world, true, &NavObstacles::none());
        assert_eq!(
            nav.path().get(1),
            Some(&old_waypoint),
            "same-goal refresh keeps steering toward the current valid waypoint"
        );
    }

    #[test]
    fn unreachable_goal_backs_off_consecutive_same_goal_repaths() {
        let (world, start, goal) = pillar_world();
        let mut nav = Navigator::new(1, 0.25, 0.9);

        nav.update_goal_when_supported(Some(goal), start, &world, true, &NavObstacles::none());
        assert_eq!(nav.recomputes(), 1);
        assert_ne!(
            nav.path().last(),
            Some(&goal),
            "the pillar top is unreachable, so the route is partial"
        );

        for _ in 0..REPATH_TICKS - 1 {
            nav.update_goal_when_supported(Some(goal), start, &world, true, &NavObstacles::none());
        }
        assert_eq!(nav.recomputes(), 1, "no retry before the base interval");

        nav.update_goal_when_supported(Some(goal), start, &world, true, &NavObstacles::none());
        assert_eq!(
            nav.recomputes(),
            2,
            "first same-goal retry happens at the base interval"
        );

        for _ in 0..(REPATH_TICKS * 2 - 1) {
            nav.update_goal_when_supported(Some(goal), start, &world, true, &NavObstacles::none());
        }
        assert_eq!(
            nav.recomputes(),
            2,
            "a second failed retry waits for the doubled backoff interval"
        );

        nav.update_goal_when_supported(Some(goal), start, &world, true, &NavObstacles::none());
        assert_eq!(
            nav.recomputes(),
            3,
            "the doubled interval eventually permits a retry"
        );
    }

    #[test]
    fn goal_cell_change_resets_unreachable_backoff_immediately() {
        let (world, start, unreachable) = pillar_world();
        let reachable = IVec3::new(2, 64, 1);
        let mut nav = Navigator::new(1, 0.25, 0.9);

        nav.update_goal_when_supported(Some(unreachable), start, &world, true, &NavObstacles::none());
        for _ in 0..REPATH_TICKS {
            nav.update_goal_when_supported(Some(unreachable), start, &world, true, &NavObstacles::none());
        }
        assert_eq!(
            nav.recomputes(),
            2,
            "the unreachable goal has entered backoff"
        );

        nav.update_goal_when_supported(Some(reachable), start, &world, true, &NavObstacles::none());
        assert_eq!(
            nav.recomputes(),
            3,
            "a different goal cell is pathed immediately despite the old goal's backoff"
        );
        assert_eq!(
            nav.path().last(),
            Some(&reachable),
            "the changed reachable goal gets a complete route"
        );
    }

    #[test]
    fn reachable_goal_keeps_the_base_repath_interval() {
        let world = flat_world();
        let start = IVec3::new(1, 64, 1);
        let goal = IVec3::new(8, 64, 1);
        let mut nav = Navigator::new(1, 0.25, 0.9);

        nav.update_goal_when_supported(Some(goal), start, &world, true, &NavObstacles::none());
        assert_eq!(nav.recomputes(), 1);
        assert_eq!(nav.path().last(), Some(&goal));

        for expected in 2..=3 {
            for _ in 0..REPATH_TICKS - 1 {
                nav.update_goal_when_supported(Some(goal), start, &world, true, &NavObstacles::none());
            }
            assert_eq!(
                nav.recomputes(),
                expected - 1,
                "no early reachable-goal repath"
            );
            nav.update_goal_when_supported(Some(goal), start, &world, true, &NavObstacles::none());
            assert_eq!(
                nav.recomputes(),
                expected,
                "reachable held goals keep the normal cadence"
            );
        }
    }

    #[test]
    fn held_goal_does_not_repath_while_airborne() {
        let mut world = flat_world();
        let start = IVec3::new(1, 64, 1);
        let goal = IVec3::new(8, 64, 1);
        let mut nav = Navigator::new(1, 0.25, 0.9);
        nav.update_goal_when_supported(Some(goal), start, &world, true, &NavObstacles::none());
        let stale: Vec<IVec3> = nav.path().to_vec();
        let blocked = IVec3::new(4, 64, 1);
        assert!(stale.contains(&blocked));

        world.set_block_world(blocked.x, blocked.y, blocked.z, Block::Stone);
        world.set_block_world(blocked.x, blocked.y + 1, blocked.z, Block::Stone);

        for _ in 0..REPATH_TICKS - 1 {
            nav.update_goal_when_supported(Some(goal), start, &world, true, &NavObstacles::none());
        }
        assert_eq!(nav.path(), stale.as_slice());

        for _ in 0..5 {
            nav.update_goal_when_supported(Some(goal), start, &world, false, &NavObstacles::none());
            assert_eq!(nav.path(), stale.as_slice(), "mid-air repath is paused");
        }

        nav.update_goal_when_supported(Some(goal), start, &world, true, &NavObstacles::none());
        assert_ne!(
            nav.path(),
            stale.as_slice(),
            "repath resumes once the mob is supported again"
        );
    }

    #[test]
    fn re_path_does_not_reset_the_stuck_tally() {
        // A mob that never moves stays stuck across re-paths: the stuck counter must keep
        // climbing through a refresh (not reset to zero each interval), so a wedged mob
        // still abandons its goal instead of re-pathing into the same wall forever.
        let world = flat_world();
        let start = IVec3::new(1, 64, 1);
        let goal = IVec3::new(8, 64, 1);
        let mut nav = Navigator::new(1, 0.25, 0.9);
        nav.update_goal_when_supported(Some(goal), start, &world, true, &NavObstacles::none());

        // Drive enough held ticks to cross several re-path intervals AND the stuck limit,
        // following from a fixed position each tick so no progress is ever made.
        let wedged = Vec3::new(1.5, 64.0, 1.5);
        let mut gave_up = false;
        for _ in 0..STUCK_TICKS + REPATH_TICKS {
            nav.update_goal_when_supported(Some(goal), start, &world, true, &NavObstacles::none());
            nav.follow(wedged, true);
            if nav.is_idle() {
                gave_up = true;
                break;
            }
        }
        assert!(
            gave_up,
            "a permanently-wedged mob still abandons its goal despite re-pathing"
        );
    }
}
