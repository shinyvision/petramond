use glam::{Mat4, Vec3};

use crate::bbmodel::euler_quat;
use crate::block::Aabb;
use crate::mesh::face::Face;

use super::ModelCube;

// ---------------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------------

/// Cube-space thickness below which a cube is treated as an authored plane. Blockbench
/// lets artists use zero-thickness cubes for decals/details; emitting both collapsed
/// opposite faces in our depth-tested pass creates z fighting.
const FLAT_FACE_EPS: f32 = 1e-4;
/// Tiny local-space offset applied to an emitted flat-cube surface so it sits just above
/// the supporting face it was authored onto (paper on the tabletop, plans on the back).
/// Keep it visibly flat, but large enough to survive depth precision at distance.
pub(super) const FLAT_FACE_BIAS: f32 = 1.0 / 64.0;
/// Maximum gap, in footprint/world-cell units, at which a solid overlapping cube is
/// considered the surface a flat detail was authored onto.
const FLAT_SUPPORT_MAX_GAP: f32 = 0.125;

/// Whether `face` should be emitted for `cube`, plus a local-space positional bias to
/// apply to each corner before the cube's static rotation. Non-flat cubes return a zero
/// bias. A cube flat on exactly one axis keeps only one of the collapsed opposite faces,
/// preferring the face that points away from the nearest overlapping solid support.
/// Cubes flat on two or three axes have no renderable area.
pub(crate) fn render_face_bias(
    cube: &ModelCube,
    all_cubes: &[ModelCube],
    face: Face,
) -> Option<Vec3> {
    let extent = (cube.to - cube.from).abs();
    let flat = [
        extent.x <= FLAT_FACE_EPS,
        extent.y <= FLAT_FACE_EPS,
        extent.z <= FLAT_FACE_EPS,
    ];
    let flat_count = flat.into_iter().filter(|&v| v).count();
    if flat_count == 0 {
        return Some(Vec3::ZERO);
    }
    if flat_count >= 2 {
        return None;
    }

    let (axis, neg, pos) = if flat[0] {
        (0, Face::NegX, Face::PosX)
    } else if flat[1] {
        (1, Face::NegY, Face::PosY)
    } else {
        (2, Face::NegZ, Face::PosZ)
    };
    if face != neg && face != pos {
        return None;
    }

    let preferred = supported_flat_face(cube, all_cubes, axis, neg, pos).unwrap_or(pos);
    let fallback = if preferred == pos { neg } else { pos };
    let keep = match (
        cube.faces[face_slot(preferred)].is_some(),
        cube.faces[face_slot(fallback)].is_some(),
    ) {
        (true, _) => preferred,
        (false, true) => fallback,
        (false, false) => return None,
    };
    (face == keep).then_some(face_normal(keep) * FLAT_FACE_BIAS)
}

#[inline]
pub(super) fn face_slot(face: Face) -> usize {
    Face::ALL.iter().position(|&f| f == face).unwrap_or(0)
}

#[inline]
fn face_normal(face: Face) -> Vec3 {
    match face {
        Face::PosX => Vec3::X,
        Face::NegX => Vec3::NEG_X,
        Face::PosY => Vec3::Y,
        Face::NegY => Vec3::NEG_Y,
        Face::PosZ => Vec3::Z,
        Face::NegZ => Vec3::NEG_Z,
    }
}

/// Pick the side of a zero-thickness cube that points away from the closest overlapping
/// non-flat support cube. For example, a paper sitting on a tabletop keeps +Y; a poster
/// sitting on the front of a back board keeps -Z. If no plausible support is found, the
/// caller falls back to Blockbench's positive face.
fn supported_flat_face(
    cube: &ModelCube,
    all_cubes: &[ModelCube],
    axis: usize,
    neg: Face,
    pos: Face,
) -> Option<Face> {
    let plane = (cube.from[axis] + cube.to[axis]) * 0.5;
    let mut neg_gap = f32::INFINITY;
    let mut pos_gap = f32::INFINITY;

    for other in all_cubes {
        if std::ptr::eq(other, cube) {
            continue;
        }
        let other_extent = (other.to - other.from).abs();
        if other_extent[axis] <= FLAT_FACE_EPS || other_extent.min_element() <= FLAT_FACE_EPS {
            continue;
        }
        if !flat_support_overlaps(cube, other, axis) {
            continue;
        }

        let omin = other.from[axis].min(other.to[axis]);
        let omax = other.from[axis].max(other.to[axis]);
        if omax <= plane + FLAT_FACE_EPS {
            neg_gap = neg_gap.min((plane - omax).max(0.0));
        }
        if omin >= plane - FLAT_FACE_EPS {
            pos_gap = pos_gap.min((omin - plane).max(0.0));
        }
    }

    let neg_supported = neg_gap <= FLAT_SUPPORT_MAX_GAP;
    let pos_supported = pos_gap <= FLAT_SUPPORT_MAX_GAP;
    match (neg_supported, pos_supported) {
        (true, true) if neg_gap <= pos_gap => Some(pos),
        (true, true) => Some(neg),
        (true, false) => Some(pos),
        (false, true) => Some(neg),
        (false, false) => None,
    }
}

fn flat_support_overlaps(flat: &ModelCube, support: &ModelCube, flat_axis: usize) -> bool {
    for axis in 0..3 {
        if axis == flat_axis {
            continue;
        }
        let amin = flat.from[axis].min(flat.to[axis]);
        let amax = flat.from[axis].max(flat.to[axis]);
        let bmin = support.from[axis].min(support.to[axis]);
        let bmax = support.from[axis].max(support.to[axis]);
        if amax <= bmin + FLAT_FACE_EPS || bmax <= amin + FLAT_FACE_EPS {
            return false;
        }
    }
    true
}

/// Bounds of ONE cube POSED by its static tilt (its 8 corners rotated about its pivot),
/// so a rotated cube's true extent is captured. Works in any space (model or footprint).
pub(super) fn posed_cube_bounds(c: &ModelCube) -> (Vec3, Vec3) {
    let tilt = Mat4::from_translation(c.origin)
        * Mat4::from_quat(euler_quat(c.rotation))
        * Mat4::from_translation(-c.origin);
    let mut mn = Vec3::splat(f32::INFINITY);
    let mut mx = Vec3::splat(f32::NEG_INFINITY);
    for corner in box_corners(c.from, c.to) {
        let p = tilt.transform_point3(corner);
        mn = mn.min(p);
        mx = mx.max(p);
    }
    (mn, mx)
}

/// The cell-local union bbox of `boxes` clipped to the unit cell at `offset`, or `None`
/// if none reach into it. Used for a cell's targeting box (the geometry overlapping it).
pub(super) fn union_clip_to_cell(boxes: &[Aabb], offset: Vec3) -> Option<Aabb> {
    let mut mn = [f32::INFINITY; 3];
    let mut mx = [f32::NEG_INFINITY; 3];
    let mut any = false;
    for b in boxes {
        if let Some(c) = clip_to_cell(b, offset) {
            any = true;
            for i in 0..3 {
                mn[i] = mn[i].min(c.min[i]);
                mx[i] = mx[i].max(c.max[i]);
            }
        }
    }
    any.then_some(Aabb { min: mn, max: mx })
}

/// The 8 corners of box `[from, to]`.
pub(super) fn box_corners(from: Vec3, to: Vec3) -> [Vec3; 8] {
    [
        Vec3::new(from.x, from.y, from.z),
        Vec3::new(to.x, from.y, from.z),
        Vec3::new(from.x, to.y, from.z),
        Vec3::new(to.x, to.y, from.z),
        Vec3::new(from.x, from.y, to.z),
        Vec3::new(to.x, from.y, to.z),
        Vec3::new(from.x, to.y, to.z),
        Vec3::new(to.x, to.y, to.z),
    ]
}

/// The footprint cell (clamped into `0..footprint`) containing footprint-space point `p`.
pub(super) fn cell_of(p: Vec3, footprint: [u8; 3]) -> [u8; 3] {
    [
        (p.x.floor() as i32).clamp(0, footprint[0] as i32 - 1) as u8,
        (p.y.floor() as i32).clamp(0, footprint[1] as i32 - 1) as u8,
        (p.z.floor() as i32).clamp(0, footprint[2] as i32 - 1) as u8,
    ]
}

/// Clip footprint-space box `b` to the unit cell at `offset`, returning it in CELL-LOCAL
/// `0..1` coordinates, or `None` if the box doesn't reach into that cell.
pub(super) fn clip_to_cell(b: &Aabb, offset: Vec3) -> Option<Aabb> {
    let mut min = [0.0f32; 3];
    let mut max = [0.0f32; 3];
    for i in 0..3 {
        let lo = (b.min[i] - offset[i]).max(0.0);
        let hi = (b.max[i] - offset[i]).min(1.0);
        if hi - lo <= 1e-4 {
            return None;
        }
        min[i] = lo;
        max[i] = hi;
    }
    Some(Aabb { min, max })
}
