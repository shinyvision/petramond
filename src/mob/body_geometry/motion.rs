use crate::mathh::Vec3;
use crate::mob::MobSize;

use super::{
    arc_component_bounds, body_boxes, segment_centre, segment_offsets, wrap_angle, WorldBox,
};

/// Resolve one tick of terrain/dynamic-body motion for the shared mob body
/// geometry. Ordinary mobs retain the one-box resolver (including step-up).
/// A long body sweeps all of its segments with one common displacement, so
/// contact at the bow or stern conservatively clamps the whole hull. The last
/// return value is the mandatory shallow-foot healing lift, exposed separately
/// so peer solving cannot roll it back into grown terrain.
#[allow(clippy::too_many_arguments)]
pub(crate) fn resolve_body_motion<F>(
    pos: Vec3,
    yaw: f32,
    size: MobSize,
    vel: [f32; 3],
    dt: f32,
    step_height: f32,
    boxes_fn: &F,
    dyn_boxes: &[crate::collision::DynBox],
    healing_dyn_boxes: &[crate::collision::DynBox],
    ignore: u64,
) -> ([f32; 3], bool, [bool; 3], f32)
where
    F: Fn(i32, i32, i32) -> &'static [crate::block::Aabb],
{
    if size.body_segments() <= 1 {
        let hw = size.half_width;
        let mut min = [pos.x - hw, pos.y, pos.z - hw];
        let mut max = [pos.x + hw, pos.y + size.height, pos.z + hw];
        let healed = crate::collision::depenetrate_up_dyn(
            min,
            max,
            crate::collision::STEP_HEIGHT,
            boxes_fn,
            healing_dyn_boxes,
            ignore,
        );
        min[1] += healed;
        max[1] += healed;
        let (mut moved, grounded, hit) = crate::collision::resolve_body_dyn_from_depenetrated(
            min,
            max,
            vel,
            dt,
            step_height,
            boxes_fn,
            dyn_boxes,
            ignore,
        );
        moved[1] += healed;
        return (moved, grounded, hit, healed);
    }

    let body: Vec<WorldBox> = body_boxes(pos, yaw, size)
        .map(|(min, max)| (min.to_array(), max.to_array()))
        .collect();
    let mut moved = [0.0; 3];
    let mut hit = [false; 3];

    // Mirror the shared resolver's shallow-foot healing, but lift every
    // segment by one common amount and respect the tightest headroom.
    let wanted_lift = body
        .iter()
        .map(|&(min, max)| {
            crate::collision::depenetrate_up_dyn(
                min,
                max,
                step_height,
                boxes_fn,
                healing_dyn_boxes,
                ignore,
            )
        })
        .fold(0.0, f32::max);
    if wanted_lift > 0.0 {
        moved[1] = compound_sweep_axis(
            &body,
            moved,
            1,
            wanted_lift,
            boxes_fn,
            healing_dyn_boxes,
            ignore,
        )
        .max(0.0);
    }
    let healed = moved[1];

    let dy = vel[1] * dt;
    if dy != 0.0 {
        let allowed = compound_sweep_axis(&body, moved, 1, dy, boxes_fn, dyn_boxes, ignore);
        moved[1] += allowed;
        hit[1] = allowed.abs() + 1e-6 < dy.abs();
    }
    let grounded = hit[1] && dy < 0.0;

    let dx = vel[0] * dt;
    let dz = vel[2] * dt;
    let normal = compound_slide(&body, moved, dx, dz, boxes_fn, dyn_boxes, ignore);
    let mut horizontal = normal;
    if grounded && step_height > 0.0 && (normal.1 || normal.2) {
        let up = compound_sweep_axis(&body, moved, 1, step_height, boxes_fn, dyn_boxes, ignore);
        if up > 0.0 {
            let mut raised_offset = moved;
            raised_offset[1] += up;
            let raised = compound_slide(&body, raised_offset, dx, dz, boxes_fn, dyn_boxes, ignore);
            if raised.0[0] * raised.0[0] + raised.0[2] * raised.0[2]
                > normal.0[0] * normal.0[0] + normal.0[2] * normal.0[2] + 1e-9
            {
                let mut settled_offset = raised_offset;
                settled_offset[0] += raised.0[0];
                settled_offset[2] += raised.0[2];
                let down =
                    compound_sweep_axis(&body, settled_offset, 1, -up, boxes_fn, dyn_boxes, ignore);
                horizontal = ([raised.0[0], up + down, raised.0[2]], raised.1, raised.2);
            }
        }
    }
    moved[0] += horizontal.0[0];
    moved[1] += horizontal.0[1];
    moved[2] += horizontal.0[2];
    hit[0] = horizontal.1;
    hit[2] = horizontal.2;

    (moved, grounded, hit, healed)
}

#[allow(clippy::too_many_arguments)]
fn compound_slide<F>(
    body: &[WorldBox],
    offset: [f32; 3],
    dx: f32,
    dz: f32,
    boxes_fn: &F,
    dyn_boxes: &[crate::collision::DynBox],
    ignore: u64,
) -> ([f32; 3], bool, bool)
where
    F: Fn(i32, i32, i32) -> &'static [crate::block::Aabb],
{
    let x = compound_sweep_axis(body, offset, 0, dx, boxes_fn, dyn_boxes, ignore);
    let mut after_x = offset;
    after_x[0] += x;
    let z = compound_sweep_axis(body, after_x, 2, dz, boxes_fn, dyn_boxes, ignore);
    (
        [x, 0.0, z],
        x.abs() + 1e-6 < dx.abs(),
        z.abs() + 1e-6 < dz.abs(),
    )
}

#[allow(clippy::too_many_arguments)]
fn compound_sweep_axis<F>(
    body: &[WorldBox],
    offset: [f32; 3],
    axis: usize,
    delta: f32,
    boxes_fn: &F,
    dyn_boxes: &[crate::collision::DynBox],
    ignore: u64,
) -> f32
where
    F: Fn(i32, i32, i32) -> &'static [crate::block::Aabb],
{
    let mut travel = delta;
    for &(mut min, mut max) in body {
        for i in 0..3 {
            min[i] += offset[i];
            max[i] += offset[i];
        }
        let allowed =
            crate::collision::sweep_axis_dyn(min, max, axis, delta, boxes_fn, dyn_boxes, ignore);
        if delta > 0.0 {
            travel = travel.min(allowed);
        } else {
            travel = travel.max(allowed);
        }
    }
    travel
}

/// A terrain-resolved transform proposal for one solid body. `start_pos` is
/// after mandatory shallow-foot healing. Peer progress rotates first and then
/// translates; any shortened translation is terrain-validated before commit.
#[derive(Copy, Clone, Debug)]
pub(crate) struct BodyMotion {
    pub id: u64,
    pub start_pos: Vec3,
    pub start_yaw: f32,
    pub end_pos: Vec3,
    pub end_yaw: f32,
    pub size: MobSize,
}

impl BodyMotion {
    pub(crate) fn pose_at(self, fraction: f32) -> (Vec3, f32) {
        let fraction = fraction.clamp(0.0, 1.0);
        (self.pos_at(fraction), self.yaw_at(fraction))
    }

    pub(super) fn yaw_delta(self) -> f32 {
        wrap_angle(self.end_yaw - self.start_yaw)
    }

    fn translation_delta(self) -> Vec3 {
        self.end_pos - self.start_pos
    }

    fn has_rotation(self) -> bool {
        self.yaw_delta().abs() > TOI_TIME_EPS
    }

    fn has_translation(self) -> bool {
        self.translation_delta().length_squared() > TOI_TIME_EPS * TOI_TIME_EPS
    }

    fn yaw_stage_end(self) -> f32 {
        match (self.has_rotation(), self.has_translation()) {
            (true, true) => 0.5,
            (true, false) => 1.0,
            (false, _) => 0.0,
        }
    }

    fn yaw_at(self, fraction: f32) -> f32 {
        let end = self.yaw_stage_end();
        if end == 0.0 {
            return self.end_yaw;
        }
        let progress = (fraction / end).clamp(0.0, 1.0);
        wrap_angle(self.start_yaw + self.yaw_delta() * progress)
    }

    fn pos_at(self, fraction: f32) -> Vec3 {
        let start = self.yaw_stage_end();
        if !self.has_translation() {
            return self.start_pos;
        }
        let progress = ((fraction - start) / (1.0 - start)).clamp(0.0, 1.0);
        self.start_pos.lerp(self.end_pos, progress)
    }

    pub(crate) fn moves_down(self) -> bool {
        self.end_pos.y < self.start_pos.y - TOI_TIME_EPS
    }

    fn swept_bounds(self) -> MotionBounds {
        let radius = self.size.half_length.unwrap_or(self.size.half_width);
        MotionBounds {
            min: [
                self.start_pos.x.min(self.end_pos.x) - radius,
                self.start_pos.y.min(self.end_pos.y),
                self.start_pos.z.min(self.end_pos.z) - radius,
            ],
            max: [
                self.start_pos.x.max(self.end_pos.x) + radius,
                self.start_pos.y.max(self.end_pos.y) + self.size.height,
                self.start_pos.z.max(self.end_pos.z) + radius,
            ],
        }
    }
}

#[derive(Copy, Clone)]
struct MotionBounds {
    min: [f32; 3],
    max: [f32; 3],
}

impl MotionBounds {
    fn overlaps(self, other: Self) -> bool {
        (0..3).all(|axis| self.max[axis] > other.min[axis] && other.max[axis] > self.min[axis])
    }
}

/// Reusable pair solver for a tick's solid-body proposals. Candidate pairs use
/// a swept X-axis broadphase; constraint passes are synchronous, so list order
/// cannot decide which member of a pair gets the remaining gap.
#[derive(Default)]
pub(crate) struct SolidMotionSolver {
    fractions: Vec<f32>,
    next_fractions: Vec<f32>,
    bounds: Vec<MotionBounds>,
    order: Vec<usize>,
    pairs: Vec<(usize, usize)>,
}

impl SolidMotionSolver {
    #[cfg(test)]
    pub(crate) fn resolve(&mut self, motions: &[BodyMotion]) -> &[f32] {
        self.resolve_with_limits(motions, &[])
    }

    pub(crate) fn resolve_with_limits(&mut self, motions: &[BodyMotion], limits: &[f32]) -> &[f32] {
        debug_assert!(limits.is_empty() || limits.len() == motions.len());
        self.fractions.clear();
        if limits.is_empty() {
            self.fractions.resize(motions.len(), 1.0);
        } else {
            self.fractions
                .extend(limits.iter().map(|limit| limit.clamp(0.0, 1.0)));
        }
        self.next_fractions.clear();
        self.next_fractions.extend_from_slice(&self.fractions);
        self.bounds.clear();
        self.bounds
            .extend(motions.iter().copied().map(BodyMotion::swept_bounds));
        self.order.clear();
        self.order.extend(0..motions.len());
        self.order.sort_by(|&a, &b| {
            self.bounds[a].min[0]
                .total_cmp(&self.bounds[b].min[0])
                .then_with(|| motions[a].id.cmp(&motions[b].id))
        });

        self.pairs.clear();
        for (rank, &a) in self.order.iter().enumerate() {
            for &b in &self.order[rank + 1..] {
                if self.bounds[b].min[0] >= self.bounds[a].max[0] {
                    break;
                }
                if self.bounds[a].overlaps(self.bounds[b]) {
                    self.pairs.push((a, b));
                }
            }
        }
        self.pairs.sort_by_key(|&(a, b)| {
            let ids = (motions[a].id, motions[b].id);
            (ids.0.min(ids.1), ids.0.max(ids.1))
        });

        // A reduction can expose a following body to a peer that another pair
        // stopped. Synchronous repetition propagates that constraint through a
        // whole contact chain without making the answer pair-iteration ordered.
        let max_passes = motions.len().saturating_mul(2).max(1);
        for _ in 0..max_passes {
            self.next_fractions.copy_from_slice(&self.fractions);
            let mut changed = false;
            for &(a, b) in &self.pairs {
                let Some(toi) =
                    body_motion_toi(motions[a], self.fractions[a], motions[b], self.fractions[b])
                else {
                    continue;
                };
                if toi >= 1.0 - TOI_TIME_EPS {
                    continue;
                }
                let a_limit = self.fractions[a] * toi;
                let b_limit = self.fractions[b] * toi;
                if a_limit + TOI_TIME_EPS < self.next_fractions[a] {
                    self.next_fractions[a] = a_limit;
                    changed = true;
                }
                if b_limit + TOI_TIME_EPS < self.next_fractions[b] {
                    self.next_fractions[b] = b_limit;
                    changed = true;
                }
            }
            std::mem::swap(&mut self.fractions, &mut self.next_fractions);
            if !changed {
                break;
            }
        }
        &self.fractions
    }

    pub(crate) fn fractions(&self) -> &[f32] {
        &self.fractions
    }
}

const TOI_TIME_EPS: f32 = 1e-6;
const ROTATING_TOI_DEPTH: u8 = 17;
const ROTATING_TOI_BUDGET: usize = 4096;

fn body_motion_toi(a: BodyMotion, a_fraction: f32, b: BodyMotion, b_fraction: f32) -> Option<f32> {
    if (a_fraction == 0.0 || a.yaw_delta() == 0.0) && (b_fraction == 0.0 || b.yaw_delta() == 0.0) {
        return translating_body_toi(a, a_fraction, b, b_fraction);
    }

    let mut budget = ROTATING_TOI_BUDGET;
    rotating_body_toi(a, a_fraction, b, b_fraction, 0.0, 1.0, 0, &mut budget)
}

fn translating_body_toi(
    a: BodyMotion,
    a_fraction: f32,
    b: BodyMotion,
    b_fraction: f32,
) -> Option<f32> {
    let (a_end, _) = a.pose_at(a_fraction);
    let (b_end, _) = b.pose_at(b_fraction);
    let relative_delta = (a_end - a.start_pos) - (b_end - b.start_pos);
    let horizontal_reach = a.size.half_width + b.size.half_width;
    if horizontal_reach <= 0.0 {
        return None;
    }

    let mut first: Option<f32> = None;
    for a_offset in segment_offsets(a.size) {
        let a_start = segment_centre(a.start_pos, a.start_yaw, a_offset);
        for b_offset in segment_offsets(b.size) {
            let b_start = segment_centre(b.start_pos, b.start_yaw, b_offset);
            let relative_start = a_start - b_start;
            let Some(toi) = swept_point_toi(
                relative_start,
                relative_delta,
                [
                    (-horizontal_reach, horizontal_reach),
                    (-a.size.height, b.size.height),
                    (-horizontal_reach, horizontal_reach),
                ],
            ) else {
                continue;
            };
            first = Some(first.map_or(toi, |current| current.min(toi)));
        }
    }
    first.map(|toi| (toi - TOI_TIME_EPS).max(0.0))
}

fn swept_point_toi(start: Vec3, delta: Vec3, ranges: [(f32, f32); 3]) -> Option<f32> {
    let start = start.to_array();
    let delta = delta.to_array();
    let mut enter = 0.0f32;
    let mut exit = 1.0f32;
    let mut starts_inside = true;
    for axis in 0..3 {
        let (lo, hi) = ranges[axis];
        starts_inside &= start[axis] > lo && start[axis] < hi;
        if delta[axis].abs() <= f32::EPSILON {
            if start[axis] <= lo || start[axis] >= hi {
                return None;
            }
            continue;
        }
        let a = (lo - start[axis]) / delta[axis];
        let b = (hi - start[axis]) / delta[axis];
        enter = enter.max(a.min(b));
        exit = exit.min(a.max(b));
        if enter > exit {
            return None;
        }
    }
    if exit <= 0.0 || enter > 1.0 {
        None
    } else if enter <= 0.0 && starts_inside {
        Some(0.0)
    } else {
        Some(enter.clamp(0.0, 1.0))
    }
}

#[allow(clippy::too_many_arguments)]
fn rotating_body_toi(
    a: BodyMotion,
    a_fraction: f32,
    b: BodyMotion,
    b_fraction: f32,
    lo: f32,
    hi: f32,
    depth: u8,
    budget: &mut usize,
) -> Option<f32> {
    if *budget == 0 {
        return Some(lo);
    }
    *budget -= 1;
    if !body_motion_interval_may_overlap(a, a_fraction, b, b_fraction, lo, hi) {
        return None;
    }
    if depth >= ROTATING_TOI_DEPTH || hi - lo <= TOI_TIME_EPS {
        return Some(lo);
    }
    let mid = (lo + hi) * 0.5;
    rotating_body_toi(a, a_fraction, b, b_fraction, lo, mid, depth + 1, budget)
        .or_else(|| rotating_body_toi(a, a_fraction, b, b_fraction, mid, hi, depth + 1, budget))
}

fn body_motion_interval_may_overlap(
    a: BodyMotion,
    a_fraction: f32,
    b: BodyMotion,
    b_fraction: f32,
    lo: f32,
    hi: f32,
) -> bool {
    let horizontal_reach = a.size.half_width + b.size.half_width;
    if horizontal_reach <= 0.0 {
        return false;
    }
    for a_offset in segment_offsets(a.size) {
        for b_offset in segment_offsets(b.size) {
            let x =
                relative_segment_range(a, a_fraction, a_offset, b, b_fraction, b_offset, 0, lo, hi);
            if !ranges_overlap(x, (-horizontal_reach, horizontal_reach)) {
                continue;
            }
            let y =
                relative_segment_range(a, a_fraction, a_offset, b, b_fraction, b_offset, 1, lo, hi);
            if !ranges_overlap(y, (-a.size.height, b.size.height)) {
                continue;
            }
            let z =
                relative_segment_range(a, a_fraction, a_offset, b, b_fraction, b_offset, 2, lo, hi);
            if ranges_overlap(z, (-horizontal_reach, horizontal_reach)) {
                return true;
            }
        }
    }
    false
}

#[allow(clippy::too_many_arguments)]
fn relative_segment_range(
    a: BodyMotion,
    a_fraction: f32,
    a_offset: f32,
    b: BodyMotion,
    b_fraction: f32,
    b_offset: f32,
    axis: usize,
    lo: f32,
    hi: f32,
) -> (f32, f32) {
    let aq0 = a_fraction * lo;
    let aq1 = a_fraction * hi;
    let bq0 = b_fraction * lo;
    let bq1 = b_fraction * hi;
    let a_translation = position_component_range(a, axis, aq0, aq1);
    let b_translation = position_component_range(b, axis, bq0, bq1);
    let translation = (
        a_translation.0.min(a_translation.1) - b_translation.0.max(b_translation.1),
        a_translation.0.max(a_translation.1) - b_translation.0.min(b_translation.1),
    );
    if axis == 1 {
        return translation;
    }

    let a_rotation = offset_component_range(a, a_offset, axis, aq0, aq1);
    let b_rotation = offset_component_range(b, b_offset, axis, bq0, bq1);
    (
        translation.0 + a_rotation.0 - b_rotation.1,
        translation.1 + a_rotation.1 - b_rotation.0,
    )
}

fn offset_component_range(
    motion: BodyMotion,
    offset: f32,
    axis: usize,
    lo: f32,
    hi: f32,
) -> (f32, f32) {
    let from = motion.yaw_at(lo);
    let to = motion.yaw_at(hi);
    match axis {
        0 => arc_component_bounds(from, to, std::f32::consts::FRAC_PI_2, |yaw| {
            -yaw.sin() * offset
        }),
        2 => arc_component_bounds(from, to, 0.0, |yaw| -yaw.cos() * offset),
        _ => (0.0, 0.0),
    }
}

fn position_component_range(motion: BodyMotion, axis: usize, lo: f32, hi: f32) -> (f32, f32) {
    let a = motion.pos_at(lo)[axis];
    let b = motion.pos_at(hi)[axis];
    (a.min(b), a.max(b))
}

/// Clamp a peer-selected motion prefix before the first terrain contact along
/// its abstract yaw-then-translation path. The ordinary resolver's endpoint
/// is terrain-safe, but its axis-ordered slide can differ from the straight
/// translation used by the peer solver; this conservative interval sweep
/// closes that corner-cutting gap before the pose is committed.
pub(crate) fn terrain_safe_motion_prefix<F>(motion: BodyMotion, requested: f32, boxes_fn: &F) -> f32
where
    F: Fn(i32, i32, i32) -> &'static [crate::block::Aabb],
{
    let requested = requested.clamp(0.0, 1.0);
    if requested <= TOI_TIME_EPS {
        return 0.0;
    }
    let mut budget = ROTATING_TOI_BUDGET;
    motion_terrain_toi(motion, boxes_fn, 0.0, requested, 0, &mut budget)
        .map_or(requested, |toi| (toi - TOI_TIME_EPS).max(0.0))
}

fn motion_terrain_toi<F>(
    motion: BodyMotion,
    boxes_fn: &F,
    lo: f32,
    hi: f32,
    depth: u8,
    budget: &mut usize,
) -> Option<f32>
where
    F: Fn(i32, i32, i32) -> &'static [crate::block::Aabb],
{
    if *budget == 0 {
        return Some(lo);
    }
    *budget -= 1;
    if !motion_interval_hits_terrain(motion, boxes_fn, lo, hi) {
        return None;
    }
    if depth >= ROTATING_TOI_DEPTH || hi - lo <= TOI_TIME_EPS {
        return Some(lo);
    }
    let mid = (lo + hi) * 0.5;
    motion_terrain_toi(motion, boxes_fn, lo, mid, depth + 1, budget)
        .or_else(|| motion_terrain_toi(motion, boxes_fn, mid, hi, depth + 1, budget))
}

fn motion_interval_hits_terrain<F>(motion: BodyMotion, boxes_fn: &F, lo: f32, hi: f32) -> bool
where
    F: Fn(i32, i32, i32) -> &'static [crate::block::Aabb],
{
    segment_offsets(motion.size).any(|offset| {
        let x = motion_segment_component_range(motion, offset, 0, lo, hi);
        let y = motion_segment_component_range(motion, offset, 1, lo, hi);
        let z = motion_segment_component_range(motion, offset, 2, lo, hi);
        let min = [
            x.0 - motion.size.half_width,
            y.0,
            z.0 - motion.size.half_width,
        ];
        let max = [
            x.1 + motion.size.half_width,
            y.1 + motion.size.height,
            z.1 + motion.size.half_width,
        ];
        crate::collision::aabb_hits_cells(min, max, boxes_fn)
    })
}

fn motion_segment_component_range(
    motion: BodyMotion,
    offset: f32,
    axis: usize,
    lo: f32,
    hi: f32,
) -> (f32, f32) {
    let translation = position_component_range(motion, axis, lo, hi);
    if axis == 1 {
        return translation;
    }
    let rotation = offset_component_range(motion, offset, axis, lo, hi);
    (translation.0 + rotation.0, translation.1 + rotation.1)
}

fn ranges_overlap(a: (f32, f32), b: (f32, f32)) -> bool {
    a.1 > b.0 && b.1 > a.0
}
