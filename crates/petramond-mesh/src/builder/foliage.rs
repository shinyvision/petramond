use glam::Vec3;

use crate::face::Face;
use crate::vertex::{push_back_face, Vertex, CORNER_SHIFT};

mod corners;
pub(super) use corners::CrownCorners;

/// Maximum cosmetic overhang; terrain culling must include it.
pub const FOLIAGE_OVERHANG: f32 = 0.25;

/// Spray shape, in blocks: how far the root sinks behind the carrier face,
/// the blade's half-width and its per-seed jitter, how much narrower the tip
/// is than the root, and how much a seed may shorten the reach.
const SPRAY_ROOT_DEPTH: f32 = 0.30;
const SPRAY_HALF_WIDTH: f32 = 0.36;
const SPRAY_WIDTH_JITTER: f32 = 0.025;
const SPRAY_TIP_TAPER: f32 = 0.85;
const SPRAY_REACH_JITTER: f32 = 1.0 / 64.0;

/// One stable word per face, shared by the leaf sprays and the mesher's
/// per-face tile variation so both read the same face the same way.
pub(super) fn face_seed(world: [i32; 3], face: Face) -> u32 {
    petramond_world::tile::spatial_hash(world, face.normal_code())
}

/// Dress one freshly pushed canopy face: a spray where it meets air, and the
/// crown's corner insets (which may subdivide the face). Both mesher arms
/// call this and nothing else for a canopy face.
pub(super) fn dress_face(
    vertices: &mut Vec<Vertex>,
    start: u32,
    face: Face,
    world: [i32; 3],
    faces_air: bool,
    crown: &CrownCorners,
) {
    if faces_air {
        emit_spray(vertices, start, face, face_seed(world, face));
    }
    crown.soften_face(vertices, start, faces_air);
}

/// One small, double-sided spray on five eighths of air-facing surfaces. Reusing the
/// carrier face's lighting keeps the cutout attached visually in shade, too.
pub(super) fn emit_spray(vertices: &mut Vec<Vertex>, source: u32, face: Face, seed: u32) {
    // Retain existing sprays while filling one additional spatial hash bucket.
    if seed & 1 != 0 && seed & 7 != 1 {
        return;
    }
    let source = source as usize;
    let carrier = [
        vertices[source],
        vertices[source + 1],
        vertices[source + 2],
        vertices[source + 3],
    ];
    let center = carrier
        .iter()
        .map(|v| Vec3::from_array(v.pos))
        .sum::<Vec3>()
        * 0.25;
    let (dx, dy, dz) = face.dir();
    let normal = Vec3::new(dx as f32, dy as f32, dz as f32);
    let axis = if dy != 0 { Vec3::X } else { Vec3::Y };
    let across = normal.cross(axis);
    let angle = ((seed >> 8) & 7) as f32 * std::f32::consts::FRAC_PI_4;
    let tangent = axis * angle.cos() + across * angle.sin();
    let width = SPRAY_HALF_WIDTH + ((seed >> 12) & 3) as f32 * SPRAY_WIDTH_JITTER;
    let reach = FOLIAGE_OVERHANG - ((seed >> 16) & 3) as f32 * SPRAY_REACH_JITTER;
    let root = center - normal * SPRAY_ROOT_DEPTH;
    let tip = center + normal * reach;
    let corners = [
        root - tangent * width,
        root + tangent * width,
        tip + tangent * width * SPRAY_TIP_TAPER,
        tip - tangent * width * SPRAY_TIP_TAPER,
    ];
    let start = vertices.len() as u32;
    for mut vertex in carrier {
        let corner = ((vertex.packed >> CORNER_SHIFT) & 3) as usize;
        vertex.pos = corners[corner].to_array();
        vertices.push(vertex);
    }
    push_back_face(vertices, start);
}

#[cfg(test)]
mod tests;
