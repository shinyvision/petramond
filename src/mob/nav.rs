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
//! entities occupy ([`NavInputs`]) so routes bend around mobs and players
//! without ever being walled off by them.

use crate::world::{SectionCursor, ServerWorld};
use petramond_math::math::{IVec3, Vec3};
use petramond_world::block::{Aabb, Block};
use petramond_world::collision;

use super::path::PathParams;
use super::spatial::MobSnapshot;
use super::{def, EntityRef, PlayerAnchor};

mod budget;
mod hazards;
mod kept;
mod plan;
mod probe;
mod step_gate;

pub use budget::{PathBudget, ReachBudget};
pub(super) use hazards::foothold_in_hazard;
pub use kept::KeptBoxes;
pub(super) use plan::NavInputs;
pub use plan::NavTuning;
pub(super) use probe::destination_reachable;
#[cfg(any(test, feature = "test-support"))]
pub use probe::route_path;
pub use probe::{
    footholds, mob_can_reach, route_probe, site_open, walk_region, FloodAsk, ROUTE_PROBE_MAX_NODES,
    ROUTE_PROBE_TICK_BUDGET,
};
use step_gate::floor_top;
pub(super) use step_gate::navigation_step_gate;

const MAX_ARRIVE_XZ: f32 = 0.3;
const MIN_ARRIVE_XZ: f32 = 0.04;
const ARRIVE_Y: f32 = 1.1;
const JUMP_TRIGGER_FRONT_XZ: f32 = 0.7;
const STUCK_TICKS: u32 = 40;
const STUCK_EPS_SQ: f32 = 0.015 * 0.015;
const GOAL_STALL_CALLS: u32 = 60;
const PASS_CORRIDOR: f32 = 0.6;
const PASS_EPS: f32 = 0.05;
const PASS_LOOKAHEAD: usize = 3;
const REPATH_TICKS: u32 = 20;
const MAX_REPATH_BACKOFF_TICKS: u32 = 200;

pub struct Navigator {
    path: Vec<IVec3>,
    index: usize,
    goal: Option<IVec3>,
    path_reaches_goal: bool,
    params: PathParams,
    half_width: f32,
    height: f32,
    stuck: u32,
    last_pos: petramond_math::world_pos::WorldPos,
    goal_best: f32,
    goal_stall: u32,
    since_path: u32,
    repath_interval: u32,
    refusals: hazards::Refusals,
    repath_ticks: u32,
    pending: Option<plan::PendingSearch>,
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
            repath_ticks: REPATH_TICKS,
            pending: None,
            #[cfg(test)]
            recomputes: 0,
        }
    }

    pub fn tolerating(mut self, blocks: &'static [Block]) -> Self {
        self.params = self.params.tolerating(blocks);
        self
    }

    pub fn is_idle(&self) -> bool {
        self.goal.is_none() || (self.index >= self.path.len() && self.pending.is_none())
    }

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
        self.repath_interval = self.repath_ticks;
        self.refusals = hazards::Refusals::default();
        self.pending = None;
    }

    pub fn follow_steered(
        &mut self,
        pos: petramond_math::world_pos::WorldPos,
        on_ground: bool,
        world: &ServerWorld,
    ) -> (Vec3, bool) {
        let (wish, jump) = self.follow(pos, on_ground);
        if jump || wish == Vec3::ZERO || self.index >= self.path.len() {
            return (wish, jump);
        }
        let wp = self.path[self.index];
        if f64::from(wp.y) > pos.y + 0.5 {
            return (wish, jump);
        }
        let (dx, dz) = cell_centre_offset(wp, pos);
        let remaining = (dx * dx + dz * dz).sqrt();
        let boxes = |x: i32, y: i32, z: i32| world.data().collision_boxes_at(x, y, z);
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
            let (t_in, lat_in, len_in) = project_horizontal(pos, self.path[j - 1], self.path[j]);
            let over_in = t_in > len_in + PASS_EPS
                && lat_in <= PASS_CORRIDOR
                && (level_ok(self.path[j - 1]) || level_ok(self.path[j]));
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
            let step_up = f64::from(wp.y) > pos.y + 0.5;
            let jump = on_ground && step_up && horiz <= self.half_width + JUMP_TRIGGER_FRONT_XZ;
            return (dir, jump);
        }

        if self.path_reaches_goal {
            self.clear();
        } else {
            self.path.clear();
            self.index = 0;
        }
        (Vec3::ZERO, false)
    }

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

fn cell_centre_offset(cell: IVec3, pos: petramond_math::world_pos::WorldPos) -> (f32, f32) {
    (
        (f64::from(cell.x) + 0.5 - pos.x) as f32,
        (f64::from(cell.z) + 0.5 - pos.z) as f32,
    )
}

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

const STEER_LOOKAHEAD: f32 = 0.4;

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
    let min = [pos.x - hw, pos.y + 1e-3, pos.z - hw];
    let max = [pos.x + hw, pos.y + f64::from(height.max(0.5)), pos.z + hw];
    let (dx, dz) = (wish.x * lookahead, wish.z * lookahead);
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
/// body FACES) wags ±60° at a few hertz. Committing to
/// the side briefly walks a small clean arc around the peer instead.
const UNSTICK_HOLD_TICKS: u8 = 8;

#[derive(Default)]
pub(super) struct Unstick {
    side: f32,
    hold: u8,
}

impl Unstick {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn steer(
        &mut self,
        wish: Vec3,
        pos: petramond_math::world_pos::WorldPos,
        self_id: u64,
        half_width: f32,
        contacts: &[EntityRef],
        target: Option<EntityRef>,
        mobs: &MobSnapshot,
        players: &[PlayerAnchor],
    ) -> Vec3 {
        if wish == Vec3::ZERO {
            self.hold = self.hold.saturating_sub(1);
            return wish;
        }
        match blocking_bearing(wish, pos, half_width, contacts, target, mobs, players) {
            Some(d) => {
                if self.hold == 0 {
                    self.side = veer_side(wish, d, self_id);
                }
                self.hold = UNSTICK_HOLD_TICKS;
                veer(wish, self.side)
            }
            None if self.hold > 0 => {
                self.hold -= 1;
                veer(wish, self.side)
            }
            None => wish,
        }
    }
}

fn blocking_bearing(
    wish: Vec3,
    pos: petramond_math::world_pos::WorldPos,
    half_width: f32,
    contacts: &[EntityRef],
    target: Option<EntityRef>,
    mobs: &MobSnapshot,
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
                if let Some(m) = mobs.live(*id) {
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
    (facing > 0.0).then_some(d)
}

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

#[derive(Copy, Clone, PartialEq)]
enum CellShape {
    Empty,
    Full,
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

pub(super) fn nav_solid_fn<'c, 'w>(
    cur: &'c SectionCursor<'w>,
) -> impl Fn(IVec3) -> bool + use<'c, 'w> {
    move |c: IVec3| {
        let block = cur.physics_block(c);
        if block.nav_reads_solid() {
            return true;
        }
        classify_boxes(cur.boxes_of(c, block)) == CellShape::Full
    }
}

pub(super) fn nav_fluid_fn<'c, 'w>(
    cur: &'c SectionCursor<'w>,
) -> impl Fn(IVec3) -> bool + use<'c, 'w> {
    move |c: IVec3| cur.fluid_cell(c)
}

pub(super) fn fluid_footing(
    cur: &SectionCursor<'_>,
    pos: petramond_math::world_pos::WorldPos,
) -> Option<Block> {
    let feet = pos.block();
    cur.physics_block(feet)
        .fluid()
        .or_else(|| cur.physics_block(feet - IVec3::Y).fluid())
}

pub(super) fn nav_loaded_fn<'c, 'w>(
    cur: &'c SectionCursor<'w>,
) -> impl Fn(IVec3) -> bool + use<'c, 'w> {
    move |c: IVec3| cur.cell_final(c)
}

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

#[cfg(test)]
mod tests;
