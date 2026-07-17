use crate::mathh::Vec3;
use crate::mob::MobSize;

use super::{
    arc_component_bounds, body_boxes, segment_centre, segment_centres, segment_offsets, wrap_angle,
};

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
