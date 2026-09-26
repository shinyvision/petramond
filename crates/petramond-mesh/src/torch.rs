//! In-world geometry for a torch: a thin 3D pole baked into the chunk mesh.
//!
//! The torch is a small box — `2/16` across, `10/16` tall — standing centered on
//! the floor or pivoted against a wall and leaning out (see
//! [`TorchPlacement::model_transform`]). Its four side faces wrap the texture's
//! center-strip body tile and the top face caps it with the flame tile; the bottom
//! is omitted (a floor torch's bottom is hidden by its support, and a wall torch's
//! is barely seen). It is flat-lit like a cross-plant — a thin object reads better
//! without per-corner ambient occlusion — and self-lit to at least its own emission
//! so it stays visibly glowing even in an unlit cave.

use glam::Vec3;
use petramond_world::tile::Tile;
use petramond_world::torch::{TorchPlacement, POLE_HALF, POLE_HEIGHT};

use super::face::Face;
use super::face_emit::FlatLit;
use super::vertex::Vertex;

/// Append the torch pole at the cell whose mesh-space origin is `origin`,
/// oriented by `placement`, textured with `side_tile` (body) + `top_tile`
/// (flame), and flat-lit by `lit`: its sky channel is the cell skylight (dims
/// with the environment sky scale) and its block channel the torch's own
/// emission COLOUR — night-invariant, so the pole keeps glowing in a dark cave
/// / at night.
pub(super) fn emit_torch(
    opaque: &mut Vec<Vertex>,
    origin: Vec3,
    placement: TorchPlacement,
    side_tile: Tile,
    top_tile: Tile,
    lit: FlatLit,
) {
    // Local model box: base at the origin, ±POLE_HALF across, POLE_HEIGHT tall. The
    // placement transform maps it into cell space; the cell's world origin is added
    // last. Using `Face::quad_box` keeps each face's corner order identical to the
    // cube mesher, so the shader maps the tile upright on every (possibly tilted)
    // face. The same transform drives the selection outline, so it hugs this pole.
    let xform = placement.model_transform();
    let lo = [-POLE_HALF, 0.0, -POLE_HALF];
    let hi = [POLE_HALF, POLE_HEIGHT, POLE_HALF];

    for (face, tile) in [
        (Face::PosX, side_tile),
        (Face::NegX, side_tile),
        (Face::PosZ, side_tile),
        (Face::NegZ, side_tile),
        (Face::PosY, top_tile),
    ] {
        for (corner, lp) in face.quad_box(lo, hi).into_iter().enumerate() {
            let wp = origin + xform.transform_point3(Vec3::from(lp));
            // Flat-lit like a cross-plant.
            opaque.push(lit.vertex(wp.to_array(), tile, corner as u32));
        }
    }
}
