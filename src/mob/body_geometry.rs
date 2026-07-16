//! Shared world-space mob body geometry.
//!
//! Long bodies are represented by the same overlapping, axis-aligned segments
//! for terrain movement, pairwise solid motion, soft contact, placement,
//! client picking, and server-side target validation.

use crate::mathh::Vec3;

use super::MobSize;

type WorldBox = ([f32; 3], [f32; 3]);

fn segment_offsets(size: MobSize) -> impl Iterator<Item = f32> {
    let hw = size.half_width;
    let hl = size.half_length.unwrap_or(hw);
    let segments = size.body_segments();
    let reach = hl - hw;

    (0..segments).map(move |i| {
        if segments == 1 {
            0.0
        } else {
            -reach + 2.0 * reach * (i as f32) / (segments - 1) as f32
        }
    })
}

fn segment_centre(pos: Vec3, yaw: f32, offset: f32) -> Vec3 {
    pos + Vec3::new(-yaw.sin(), 0.0, -yaw.cos()) * offset
}

fn segment_centres(pos: Vec3, yaw: f32, size: MobSize) -> impl Iterator<Item = Vec3> {
    segment_offsets(size).map(move |offset| segment_centre(pos, yaw, offset))
}

/// World-space boxes covering a mob body at `pos` and `yaw`.
///
/// Ordinary bodies yield one square box. A body with `half_length` yields a
/// run of overlapping square boxes along its facing axis, matching the solid
/// collision staircase at diagonal yaws without filling the enclosing
/// square's empty corners.
pub(crate) fn body_boxes(pos: Vec3, yaw: f32, size: MobSize) -> impl Iterator<Item = (Vec3, Vec3)> {
    let hw = size.half_width;
    segment_centres(pos, yaw, size).map(move |centre| {
        (
            Vec3::new(centre.x - hw, centre.y, centre.z - hw),
            Vec3::new(centre.x + hw, centre.y + size.height, centre.z + hw),
        )
    })
}

/// One deterministic soft separation for two complete mob bodies. Segment
/// overlaps are candidates, not independent pushes: the deepest contact wins
/// so a long body cannot multiply its shove by its segment count.
pub(crate) fn body_separation(
    a_pos: Vec3,
    a_yaw: f32,
    a_size: MobSize,
    b_pos: Vec3,
    b_yaw: f32,
    b_size: MobSize,
) -> Option<Vec3> {
    if !compound_push_bounds_overlap(a_pos, a_size, b_pos, b_size) {
        return None;
    }
    strongest_separation(
        segment_centres(a_pos, a_yaw, a_size)
            .map(|centre| crate::body::Body::new(centre, a_size.half_width, a_size.height)),
        || {
            segment_centres(b_pos, b_yaw, b_size)
                .map(|centre| crate::body::Body::new(centre, b_size.half_width, b_size.height))
        },
    )
}

/// One deterministic soft separation for a complete mob body away from an
/// ordinary entity body (currently a player).
pub(crate) fn body_separation_from_body(
    pos: Vec3,
    yaw: f32,
    size: MobSize,
    other: crate::body::Body,
) -> Option<Vec3> {
    let other_pos = Vec3::new(other.x, other.y0, other.z);
    let other_size = MobSize {
        half_width: other.hw,
        height: other.y1 - other.y0,
        half_length: None,
    };
    if !compound_push_bounds_overlap(pos, size, other_pos, other_size) {
        return None;
    }
    strongest_separation(
        segment_centres(pos, yaw, size)
            .map(|centre| crate::body::Body::new(centre, size.half_width, size.height)),
        || std::iter::once(other),
    )
}

const PEER_SUPPORT_VERTICAL_EPS: f32 = 1e-3;
const PEER_SUPPORT_HORIZONTAL_EPS: f32 = 1e-4;

/// Append the candidate solid boxes whose top faces currently support this
/// complete body. The vertical band is deliberately tight and horizontal
/// face-touching is excluded, so a wall contact cannot masquerade as ground.
pub(crate) fn append_body_supports(
    pos: Vec3,
    yaw: f32,
    size: MobSize,
    candidates: &[crate::collision::DynBox],
    ignore: u64,
    out: &mut Vec<crate::collision::DynBox>,
) {
    out.extend(
        candidates
            .iter()
            .copied()
            .filter(|candidate| candidate.id != ignore)
            .filter(|candidate| body_box_is_supported_by(pos, yaw, size, candidate)),
    );
}

/// Whether a final compound pose rests on a peer solid's top face.
pub(crate) fn body_has_peer_support(
    pos: Vec3,
    yaw: f32,
    size: MobSize,
    candidates: &[crate::collision::DynBox],
    ignore: u64,
) -> bool {
    candidates.iter().any(|candidate| {
        candidate.id != ignore && body_box_is_supported_by(pos, yaw, size, candidate)
    })
}

fn body_box_is_supported_by(
    pos: Vec3,
    yaw: f32,
    size: MobSize,
    candidate: &crate::collision::DynBox,
) -> bool {
    let gap = pos.y - candidate.max[1];
    if !(-PEER_SUPPORT_VERTICAL_EPS..=PEER_SUPPORT_VERTICAL_EPS).contains(&gap) {
        return false;
    }
    body_boxes(pos, yaw, size).any(|(min, max)| {
        max.x > candidate.min[0] + PEER_SUPPORT_HORIZONTAL_EPS
            && min.x < candidate.max[0] - PEER_SUPPORT_HORIZONTAL_EPS
            && max.z > candidate.min[2] + PEER_SUPPORT_HORIZONTAL_EPS
            && min.z < candidate.max[2] - PEER_SUPPORT_HORIZONTAL_EPS
    })
}

fn compound_push_bounds_overlap(
    a_pos: Vec3,
    a_size: MobSize,
    b_pos: Vec3,
    b_size: MobSize,
) -> bool {
    if a_pos.y + a_size.height <= b_pos.y || b_pos.y + b_size.height <= a_pos.y {
        return false;
    }
    let dx = a_pos.x - b_pos.x;
    let dz = a_pos.z - b_pos.z;
    let a_reach = a_size
        .half_length
        .unwrap_or(a_size.half_width)
        .hypot(a_size.half_width);
    let b_reach = b_size
        .half_length
        .unwrap_or(b_size.half_width)
        .hypot(b_size.half_width);
    let reach = a_reach + b_reach;
    dx * dx + dz * dz < reach * reach
}

fn strongest_separation<A, B>(a: A, b: impl Fn() -> B) -> Option<Vec3>
where
    A: IntoIterator<Item = crate::body::Body>,
    B: IntoIterator<Item = crate::body::Body>,
{
    let mut best: Option<Vec3> = None;
    for a in a {
        for b in b() {
            let Some(candidate) = crate::body::separation(a, b) else {
                continue;
            };
            if best
                .as_ref()
                .is_none_or(|current| candidate.length_squared() > current.length_squared())
            {
                best = Some(candidate);
            }
        }
    }
    best
}

/// Whether any segment of a mob body overlaps the supplied cell-local block
/// collision boxes. This is the shared placement-occupancy query: ordinary
/// bodies take the single-segment fast path naturally, while a long body's
/// bow and stern participate exactly like its centre.
pub(crate) fn body_overlaps_block_boxes(
    pos: Vec3,
    yaw: f32,
    size: MobSize,
    cell: crate::mathh::IVec3,
    boxes: &[crate::block::Aabb],
) -> bool {
    if boxes.is_empty() {
        return false;
    }
    segment_centres(pos, yaw, size).any(|centre| {
        crate::body::Body::new(centre, size.half_width, size.height)
            .overlaps_block_boxes(cell, boxes)
    })
}

/// Whether a whole mob body can be created at this pose without intersecting
/// terrain or a live solid entity. Every covered cell must have final physics
/// state: unresolved or in-flight terrain is not permission to spawn through
/// whatever its generated or saved content may later restore.
pub(crate) fn body_pose_fits<F, K>(
    pos: Vec3,
    yaw: f32,
    size: MobSize,
    boxes_fn: &F,
    known: &K,
    dyn_boxes: &[crate::collision::DynBox],
) -> bool
where
    F: Fn(i32, i32, i32) -> &'static [crate::block::Aabb],
    K: Fn(i32, i32, i32) -> bool,
{
    if !pos.is_finite() || !yaw.is_finite() {
        return false;
    }
    body_boxes(pos, yaw, size).all(|(min, max)| {
        let min = min.to_array();
        let max = max.to_array();
        aabb_cells_known(min, max, known)
            && !crate::collision::aabb_hits_cells(min, max, boxes_fn)
            && !crate::collision::aabb_hits_dynamic(
                min,
                max,
                dyn_boxes,
                crate::collision::NOT_AN_ENTITY,
            )
    })
}

fn aabb_cells_known<K>(min: [f32; 3], max: [f32; 3], known: &K) -> bool
where
    K: Fn(i32, i32, i32) -> bool,
{
    let hi = max.map(|edge| {
        let floor = edge.floor();
        floor as i32 - i32::from(edge == floor)
    });
    for x in (min[0].floor() as i32)..=hi[0] {
        for y in (min[1].floor() as i32)..=hi[1] {
            for z in (min[2].floor() as i32)..=hi[2] {
                if !known(x, y, z) {
                    return false;
                }
            }
        }
    }
    true
}

/// Clamp an absolute yaw request to the collision-free prefix of its shortest
/// rotation. Ordinary square bodies are rotation-invariant and accept the
/// request directly. Long bodies sweep every shared segment through a bounded,
/// conservative arc AABB, so a bow cannot rotate through terrain or another
/// solid body between two otherwise-clear endpoint poses.
#[allow(clippy::too_many_arguments)]
pub(crate) fn clamp_body_yaw<F>(
    pos: Vec3,
    current: f32,
    requested: f32,
    size: MobSize,
    boxes_fn: &F,
    dyn_boxes: &[crate::collision::DynBox],
    ignore: u64,
) -> f32
where
    F: Fn(i32, i32, i32) -> &'static [crate::block::Aabb],
{
    if !current.is_finite() || !requested.is_finite() {
        return current;
    }
    let start = wrap_angle(current);
    let target = wrap_angle(requested);
    let delta = wrap_angle(target - start);
    let reach = size.half_length.unwrap_or(size.half_width) - size.half_width;
    if reach <= 0.0 || delta == 0.0 {
        return target;
    }

    // Keep the outermost segment centre's arc travel to at most half a
    // segment radius per probe. Row validation bounds reach/half_width and
    // therefore bounds this loop independently of absolute body size.
    let probes = (reach * delta.abs() / (size.half_width * 0.5))
        .ceil()
        .max(1.0) as usize;
    let mut accepted = start;
    for step in 1..=probes {
        let from = start + delta * (step - 1) as f32 / probes as f32;
        let to = start + delta * step as f32 / probes as f32;
        if rotation_step_hits(pos, from, to, size, boxes_fn, dyn_boxes, ignore) {
            break;
        }
        accepted = wrap_angle(to);
    }
    accepted
}

#[allow(clippy::too_many_arguments)]
fn rotation_step_hits<F>(
    pos: Vec3,
    from: f32,
    to: f32,
    size: MobSize,
    boxes_fn: &F,
    dyn_boxes: &[crate::collision::DynBox],
    ignore: u64,
) -> bool
where
    F: Fn(i32, i32, i32) -> &'static [crate::block::Aabb],
{
    segment_offsets(size).any(|offset| {
        let (min_x, max_x) = arc_component_bounds(from, to, std::f32::consts::FRAC_PI_2, |yaw| {
            segment_centre(pos, yaw, offset).x
        });
        let (min_z, max_z) =
            arc_component_bounds(from, to, 0.0, |yaw| segment_centre(pos, yaw, offset).z);
        let min = [min_x - size.half_width, pos.y, min_z - size.half_width];
        let max = [
            max_x + size.half_width,
            pos.y + size.height,
            max_z + size.half_width,
        ];
        crate::collision::aabb_hits_cells(min, max, boxes_fn)
            || crate::collision::aabb_hits_dynamic(min, max, dyn_boxes, ignore)
    })
}

fn arc_component_bounds(
    from: f32,
    to: f32,
    critical_base: f32,
    value: impl Fn(f32) -> f32,
) -> (f32, f32) {
    let lo = from.min(to);
    let hi = from.max(to);
    let a = value(from);
    let b = value(to);
    let mut min = a.min(b);
    let mut max = a.max(b);
    let first = ((lo - critical_base) / std::f32::consts::PI).ceil() as i32;
    let last = ((hi - critical_base) / std::f32::consts::PI).floor() as i32;
    for k in first..=last {
        let at = critical_base + k as f32 * std::f32::consts::PI;
        let v = value(at);
        min = min.min(v);
        max = max.max(v);
    }
    (min, max)
}

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

fn wrap_angle(angle: f32) -> f32 {
    let wrapped = angle.rem_euclid(std::f32::consts::TAU);
    if wrapped > std::f32::consts::PI {
        wrapped - std::f32::consts::TAU
    } else {
        wrapped
    }
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

    fn yaw_delta(self) -> f32 {
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

/// The nearest body intersected by a ray before `max_dist`.
///
/// Callers choose which bodies participate and attach any key they need. The
/// body-box emitter and nearest-hit rule stay shared between client picking
/// and authoritative server validation.
pub(crate) fn closest_body_ray_hit<K: Ord>(
    eye: Vec3,
    dir: Vec3,
    max_dist: f32,
    bodies: impl IntoIterator<Item = (K, Vec3, f32, MobSize)>,
) -> Option<(K, f32)> {
    let mut best = None;
    for (key, pos, yaw, size) in bodies {
        let hit = body_boxes(pos, yaw, size)
            .filter_map(|(min, max)| crate::player::ray_vs_aabb(eye, dir, min, max))
            .min_by(f32::total_cmp);
        if let Some(t) = hit.filter(|t| *t <= max_dist) {
            let nearer = best.as_ref().is_none_or(|(best_key, best_t)| {
                t.total_cmp(best_t).then_with(|| key.cmp(best_key)).is_lt()
            });
            if nearer {
                best = Some((key, t));
            }
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    fn long_body() -> MobSize {
        MobSize {
            half_width: 0.5,
            height: 1.0,
            half_length: Some(1.5),
        }
    }

    fn bodies_overlap(
        a_pos: Vec3,
        a_yaw: f32,
        a_size: MobSize,
        b_pos: Vec3,
        b_yaw: f32,
        b_size: MobSize,
    ) -> bool {
        let mut b_boxes = Vec::new();
        super::super::solid_boxes(2, b_pos, b_yaw, b_size, &mut b_boxes);
        body_boxes(a_pos, a_yaw, a_size).any(|(min, max)| {
            crate::collision::aabb_hits_dynamic(min.to_array(), max.to_array(), &b_boxes, 1)
        })
    }

    #[test]
    fn a_long_body_does_not_fill_its_enclosing_square_corners() {
        let size = long_body();
        let pos = Vec3::ZERO;
        let eye = Vec3::new(1.0, 0.5, 3.0);
        let dir = -Vec3::Z;
        let enclosing_extent = size.half_length.unwrap();
        let enclosing_min = Vec3::new(pos.x - enclosing_extent, pos.y, pos.z - enclosing_extent);
        let enclosing_max = Vec3::new(
            pos.x + enclosing_extent,
            pos.y + size.height,
            pos.z + enclosing_extent,
        );

        assert!(
            crate::player::ray_vs_aabb(eye, dir, enclosing_min, enclosing_max).is_some(),
            "the former enclosing square would select this empty corner"
        );
        assert_eq!(
            closest_body_ray_hit(eye, dir, crate::player::REACH, [(7, pos, 0.0, size)]),
            None,
            "segmented body geometry leaves the corner empty"
        );
    }

    #[test]
    fn a_long_body_blocks_placement_at_its_bow_and_stern() {
        let size = long_body();
        let pos = Vec3::ZERO;
        let bow_cell = crate::mathh::IVec3::new(0, 0, -2);
        let full = crate::block::Block::Stone.collision_boxes();

        assert!(
            !crate::body::Body::new(pos, size.half_width, size.height)
                .overlaps_block_boxes(bow_cell, full),
            "the legacy centre square does not reach the bow cell"
        );
        assert!(
            body_overlaps_block_boxes(pos, 0.0, size, bow_cell, full),
            "the shared segmented body prevents bow-clipping placement"
        );
        assert!(
            body_overlaps_block_boxes(pos, 0.0, size, crate::mathh::IVec3::new(0, 0, 1), full,),
            "the stern participates too"
        );
    }

    #[test]
    fn a_bow_only_soft_contact_produces_one_compound_shove() {
        let hull = long_body();
        let soft = MobSize {
            half_width: 0.25,
            height: 0.8,
            half_length: None,
        };
        let hull_pos = Vec3::ZERO;
        let soft_pos = Vec3::new(0.0, 0.0, -1.6);

        assert!(
            crate::body::separation(
                crate::body::Body::new(hull_pos, hull.half_width, hull.height),
                crate::body::Body::new(soft_pos, soft.half_width, soft.height),
            )
            .is_none(),
            "the former centre-only snapshot misses this bow contact"
        );
        let shove = body_separation(hull_pos, 0.0, hull, soft_pos, 0.0, soft)
            .expect("the compound bow touches the soft body");
        assert!(
            shove.z > 0.0,
            "the hull is separated away from the bow contact"
        );

        let deepest_single = segment_centres(hull_pos, 0.0, hull)
            .filter_map(|centre| {
                crate::body::separation(
                    crate::body::Body::new(centre, hull.half_width, hull.height),
                    crate::body::Body::new(soft_pos, soft.half_width, soft.height),
                )
            })
            .max_by(|a, b| a.length_squared().total_cmp(&b.length_squared()))
            .unwrap();
        assert_eq!(
            shove, deepest_single,
            "segment contacts select one pair separation instead of accumulating"
        );
    }

    #[test]
    fn diagonal_square_contact_survives_the_push_broadphase() {
        let size = MobSize {
            half_width: 0.5,
            height: 1.0,
            half_length: None,
        };
        assert!(
            body_separation(Vec3::ZERO, 0.0, size, Vec3::new(0.9, 0.0, 0.9), 0.0, size,).is_some(),
            "overlapping square corners cannot be culled by a circular broadphase"
        );
    }

    #[test]
    fn two_driven_long_solids_share_toi_and_never_pass_through_on_later_ticks() {
        let size = long_body();
        let yaw = -std::f32::consts::FRAC_PI_2;
        let mut a_pos = Vec3::new(-3.0, 0.0, 0.0);
        let mut b_pos = Vec3::new(3.0, 0.0, 0.0);
        let mut solver = SolidMotionSolver::default();

        let first = [
            BodyMotion {
                id: 10,
                start_pos: a_pos,
                start_yaw: yaw,
                end_pos: a_pos + Vec3::X * 2.0,
                end_yaw: yaw,
                size,
            },
            BodyMotion {
                id: 20,
                start_pos: b_pos,
                start_yaw: yaw,
                end_pos: b_pos - Vec3::X * 2.0,
                end_yaw: yaw,
                size,
            },
        ];
        let forward = solver.resolve(&first).to_vec();
        let reverse = solver.resolve(&[first[1], first[0]]).to_vec();
        assert!((forward[0] - forward[1]).abs() < 1e-6);
        assert!((forward[0] - reverse[1]).abs() < 1e-6);
        assert!((forward[1] - reverse[0]).abs() < 1e-6);

        for _ in 0..8 {
            let motions = [
                BodyMotion {
                    id: 10,
                    start_pos: a_pos,
                    start_yaw: yaw,
                    end_pos: a_pos + Vec3::X * 2.0,
                    end_yaw: yaw,
                    size,
                },
                BodyMotion {
                    id: 20,
                    start_pos: b_pos,
                    start_yaw: yaw,
                    end_pos: b_pos - Vec3::X * 2.0,
                    end_yaw: yaw,
                    size,
                },
            ];
            let fractions = solver.resolve(&motions);
            a_pos = motions[0].pose_at(fractions[0]).0;
            b_pos = motions[1].pose_at(fractions[1]).0;

            assert!(a_pos.x < b_pos.x, "the stable identities never cross");
            assert!(
                !bodies_overlap(a_pos, yaw, size, b_pos, yaw, size),
                "committed compound bodies remain non-overlapping: {a_pos:?} {b_pos:?}"
            );
        }
    }

    #[test]
    fn two_turning_long_solids_stop_before_their_bows_overlap() {
        let size = long_body();
        let motions = [
            BodyMotion {
                id: 10,
                start_pos: Vec3::new(-1.4, 0.0, 0.0),
                start_yaw: 0.0,
                end_pos: Vec3::new(-1.4, 0.0, 0.0),
                end_yaw: -std::f32::consts::FRAC_PI_2,
                size,
            },
            BodyMotion {
                id: 20,
                start_pos: Vec3::new(1.4, 0.0, 0.0),
                start_yaw: 0.0,
                end_pos: Vec3::new(1.4, 0.0, 0.0),
                end_yaw: std::f32::consts::FRAC_PI_2,
                size,
            },
        ];
        let mut solver = SolidMotionSolver::default();
        let forward = solver.resolve(&motions).to_vec();
        let reverse = solver.resolve(&[motions[1], motions[0]]).to_vec();

        assert!(forward.iter().all(|fraction| *fraction < 1.0));
        assert!((forward[0] - forward[1]).abs() < 1e-5);
        assert!((forward[0] - reverse[1]).abs() < 1e-5);
        assert!((forward[1] - reverse[0]).abs() < 1e-5);
        let (a_pos, a_yaw) = motions[0].pose_at(forward[0]);
        let (b_pos, b_yaw) = motions[1].pose_at(forward[1]);
        assert!(
            !bodies_overlap(a_pos, a_yaw, size, b_pos, b_yaw, size),
            "simultaneous yaw commits a non-overlapping prefix"
        );
    }

    #[test]
    fn a_peer_truncated_turn_and_translation_stays_clear_of_shore() {
        let size = long_body();
        let square = MobSize {
            half_width: 0.5,
            height: 1.0,
            half_length: None,
        };
        let shore = |x: i32, y: i32, z: i32| {
            if (x, y, z) == (2, 0, -2) {
                crate::block::Block::Stone.collision_boxes()
            } else {
                crate::block::Block::Air.collision_boxes()
            }
        };
        let start = Vec3::ZERO;
        let requested_yaw = -std::f32::consts::FRAC_PI_2;
        let end_yaw = clamp_body_yaw(start, 0.0, requested_yaw, size, &shore, &[], 10);
        assert!(
            (end_yaw - requested_yaw).abs() < 1e-6,
            "the yaw stage clears shore"
        );
        let (moved, _, _, _) = resolve_body_motion(
            start,
            end_yaw,
            size,
            [2.0, 0.0, 0.0],
            1.0,
            0.0,
            &shore,
            &[],
            &[],
            10,
        );
        assert!((moved[0] - 2.0).abs() < 1e-6, "the X sweep clears shore");
        let motion = BodyMotion {
            id: 10,
            start_pos: start,
            start_yaw: 0.0,
            end_pos: start + Vec3::from(moved),
            end_yaw,
            size,
        };
        let peer = BodyMotion {
            id: 20,
            start_pos: Vec3::new(2.6, 0.0, -0.4),
            start_yaw: 0.0,
            end_pos: Vec3::new(2.6, 0.0, -0.4),
            end_yaw: 0.0,
            size: square,
        };
        let mut solver = SolidMotionSolver::default();
        let fraction = solver.resolve(&[motion, peer])[0];
        assert!(
            (0.5..1.0).contains(&fraction),
            "the peer stops the translation after yaw completes: {fraction}"
        );
        let safe = terrain_safe_motion_prefix(motion, fraction, &shore);
        assert!(
            (safe - fraction).abs() < 1e-5,
            "the staged peer prefix follows the terrain-validated yaw then sweep"
        );
        let (pos, yaw) = motion.pose_at(safe);
        assert!(body_pose_fits(pos, yaw, size, &shore, &|_, _, _| true, &[],));

        let old_coupled_pos = start.lerp(motion.end_pos, fraction);
        let old_coupled_yaw = wrap_angle(motion.start_yaw + motion.yaw_delta() * fraction);
        assert!(
            !body_pose_fits(
                old_coupled_pos,
                old_coupled_yaw,
                size,
                &shore,
                &|_, _, _| true,
                &[],
            ),
            "the former coupled prefix clipped this shore corner"
        );
    }

    #[test]
    fn a_peer_truncated_axis_slide_is_clamped_before_its_terrain_corner() {
        let size = MobSize {
            half_width: 0.2,
            height: 1.0,
            half_length: None,
        };
        let corner = |x: i32, y: i32, z: i32| {
            if (x, y, z) == (1, 0, 1) {
                crate::block::Block::Stone.collision_boxes()
            } else {
                crate::block::Block::Air.collision_boxes()
            }
        };
        let start = Vec3::new(0.5, 0.0, 0.5);
        let (moved, _, _, _) = resolve_body_motion(
            start,
            0.0,
            size,
            [2.0, 0.0, 2.0],
            1.0,
            0.0,
            &corner,
            &[],
            &[],
            10,
        );
        assert_eq!(
            moved,
            [2.0, 0.0, 2.0],
            "the terrain resolver's X-then-Z route goes around the block"
        );
        let motion = BodyMotion {
            id: 10,
            start_pos: start,
            start_yaw: 0.0,
            end_pos: start + Vec3::from(moved),
            end_yaw: 0.0,
            size,
        };
        let peer = BodyMotion {
            id: 20,
            start_pos: motion.end_pos,
            start_yaw: 0.0,
            end_pos: motion.end_pos,
            end_yaw: 0.0,
            size,
        };
        let mut solver = SolidMotionSolver::default();
        let peer_fraction = solver.resolve(&[motion, peer])[0];
        assert!(peer_fraction < 1.0);
        let safe = terrain_safe_motion_prefix(motion, peer_fraction, &corner);
        assert!(
            safe + 1e-4 < peer_fraction,
            "the straight peer path is stopped before cutting the X/Z corner"
        );

        let fractions = solver.resolve_with_limits(&[motion, peer], &[safe, 1.0]);
        let (pos, yaw) = motion.pose_at(fractions[0]);
        let (peer_pos, peer_yaw) = peer.pose_at(fractions[1]);
        assert!(body_pose_fits(
            pos,
            yaw,
            size,
            &corner,
            &|_, _, _| true,
            &[],
        ));
        assert!(
            !bodies_overlap(pos, yaw, size, peer_pos, peer_yaw, size),
            "pair solving is rerun after the terrain limit"
        );
    }

    #[test]
    fn checked_body_fit_rejects_shore_and_solid_entity_overlap() {
        let size = long_body();
        let pos = Vec3::ZERO;
        let known = |_: i32, _: i32, _: i32| true;
        let shore = |x: i32, y: i32, z: i32| {
            if (x, y, z) == (0, 0, -2) {
                crate::block::Block::Stone.collision_boxes()
            } else {
                crate::block::Block::Air.collision_boxes()
            }
        };
        assert!(
            !body_pose_fits(pos, 0.0, size, &shore, &known, &[]),
            "terrain touching only the bow rejects the complete pose"
        );

        let air = |_: i32, _: i32, _: i32| crate::block::Block::Air.collision_boxes();
        let obstacle = crate::collision::DynBox {
            id: 7,
            min: [-0.5, 0.0, -1.5],
            max: [0.5, 1.0, -0.5],
        };
        assert!(
            !body_pose_fits(pos, 0.0, size, &air, &known, &[obstacle]),
            "another solid body rejects the spawn atomically"
        );
        assert!(body_pose_fits(pos, 0.0, size, &air, &known, &[]));

        let face_touch_is_not_covered = |x: i32, _: i32, _: i32| x != 1;
        assert!(body_pose_fits(
            Vec3::new(0.5, 0.0, 0.0),
            0.0,
            size,
            &air,
            &face_touch_is_not_covered,
            &[],
        ));
    }

    #[test]
    fn equal_distance_ray_hits_choose_the_lower_stable_id_in_any_input_order() {
        let size = long_body();
        let pos = Vec3::ZERO;
        let eye = Vec3::new(0.0, 0.5, 3.0);
        let dir = -Vec3::Z;

        let forward = closest_body_ray_hit(
            eye,
            dir,
            crate::player::REACH,
            [(2_u64, pos, 0.0, size), (9_u64, pos, 0.0, size)],
        );
        let reverse = closest_body_ray_hit(
            eye,
            dir,
            crate::player::REACH,
            [(9_u64, pos, 0.0, size), (2_u64, pos, 0.0, size)],
        );

        assert_eq!(forward.map(|(id, _)| id), Some(2));
        assert_eq!(reverse.map(|(id, _)| id), Some(2));
    }

    #[test]
    fn a_long_body_bow_stops_at_shore_before_its_centre_box_arrives() {
        let size = long_body();
        let boxes = |x: i32, y: i32, _z: i32| {
            if x == 2 && y == 0 {
                crate::block::Block::Stone.collision_boxes()
            } else {
                &[]
            }
        };
        // yaw -PI/2 faces +X. The bow already reaches x=1.5, leaving only
        // half a block before the shore at x=2; the old centre square would
        // have travelled 1.5 blocks before noticing it.
        let (moved, _, hit, _) = resolve_body_motion(
            Vec3::ZERO,
            -std::f32::consts::FRAC_PI_2,
            size,
            [2.0, 0.0, 0.0],
            1.0,
            crate::collision::STEP_HEIGHT,
            &boxes,
            &[],
            &[],
            1,
        );

        assert!(hit[0], "the bow, not just the centre, hits the shore");
        assert!(
            (0.49..=0.51).contains(&moved[0]),
            "the hull stops flush at shore: moved {}",
            moved[0]
        );
    }

    #[test]
    fn a_long_body_cannot_rotate_its_bow_through_shore() {
        let size = long_body();
        let boxes = |x: i32, y: i32, _z: i32| {
            if x == 1 && y == 0 {
                crate::block::Block::Stone.collision_boxes()
            } else {
                &[]
            }
        };
        let pos = Vec3::ZERO;
        let requested = -std::f32::consts::FRAC_PI_2;
        assert!(
            body_boxes(pos, requested, size).any(|(min, max)| {
                crate::collision::aabb_hits_cells(min.to_array(), max.to_array(), boxes)
            }),
            "the unvalidated quarter-turn would put the bow in shore"
        );

        let accepted = clamp_body_yaw(pos, 0.0, requested, size, &boxes, &[], 1);
        assert_ne!(accepted, requested, "the clipping rotation is clamped");
        assert!(
            body_boxes(pos, accepted, size).all(|(min, max)| {
                !crate::collision::aabb_hits_cells(min.to_array(), max.to_array(), boxes)
            }),
            "every accepted body segment remains outside terrain"
        );
    }

    #[test]
    fn a_long_body_touching_shore_can_rotate_away() {
        let size = long_body();
        let boxes = |x: i32, y: i32, _z: i32| {
            if x == 2 && y == 0 {
                crate::block::Block::Stone.collision_boxes()
            } else {
                &[]
            }
        };
        let pos = Vec3::new(0.5, 0.0, 0.0);
        let current = -std::f32::consts::FRAC_PI_2;
        let accepted = clamp_body_yaw(pos, current, 0.0, size, &boxes, &[], 1);

        assert_eq!(accepted, 0.0, "contact does not pin a hull turning away");
    }
}
