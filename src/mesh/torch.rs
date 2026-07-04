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

use crate::atlas::Tile;
use crate::mathh::Vec3;
use crate::torch::{TorchPlacement, POLE_HALF, POLE_HEIGHT};

use super::face::Face;
use super::vertex::{pack_vertex, pack_vertex2, Vertex};

/// Append the torch pole at the cell whose world origin is `(bx, by, bz)`, oriented
/// by `placement`, textured with `side_tile` (body) + `top_tile` (flame), tinted by
/// `tint`, and flat-lit at the packed 6-bit channels: `sky6` (cell skylight, dims
/// with the environment sky scale) and `block6` (the torch's own emission — night-
/// invariant, so the pole keeps glowing in a dark cave / at night).
#[allow(clippy::too_many_arguments)]
pub(super) fn emit_torch(
    opaque: &mut Vec<Vertex>,
    opaque_idx: &mut Vec<u32>,
    bx: f32,
    by: f32,
    bz: f32,
    placement: TorchPlacement,
    side_tile: Tile,
    top_tile: Tile,
    tint: [f32; 3],
    sky6: u32,
    block6: u32,
) {
    // Local model box: base at the origin, ±POLE_HALF across, POLE_HEIGHT tall. The
    // placement transform maps it into cell space; the cell's world origin is added
    // last. Using `Face::quad_box` keeps each face's corner order identical to the
    // cube mesher, so the shader maps the tile upright on every (possibly tilted)
    // face. The same transform drives the selection outline, so it hugs this pole.
    let xform = placement.model_transform();
    let origin = Vec3::new(bx, by, bz);
    let lo = [-POLE_HALF, 0.0, -POLE_HALF];
    let hi = [POLE_HALF, POLE_HEIGHT, POLE_HALF];

    for (face, tile) in [
        (Face::PosX, side_tile),
        (Face::NegX, side_tile),
        (Face::PosZ, side_tile),
        (Face::NegZ, side_tile),
        (Face::PosY, top_tile),
    ] {
        let start = opaque.len() as u32;
        for (corner, lp) in face.quad_box(lo, hi).into_iter().enumerate() {
            let wp = origin + xform.transform_point3(Vec3::new(lp[0], lp[1], lp[2]));
            opaque.push(Vertex {
                pos: [wp.x, wp.y, wp.z],
                tint,
                // Flat-lit (shade index 0, AO 3) like a cross-plant: no overlay.
                packed: pack_vertex(tile.index() as u32, corner as u32, 0, 0, false, 3, sky6),
                packed2: pack_vertex2(block6),
            });
        }
        opaque_idx.extend_from_slice(&[start, start + 1, start + 2, start, start + 2, start + 3]);
    }
}
