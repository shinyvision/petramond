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

use crate::vertex::BlockLightVertexExt;
use petramond_world::light::BlockLight6;
use petramond_world::mathh::Vec3;
use petramond_world::tile::Tile;
use petramond_world::torch::{TorchPlacement, POLE_HALF, POLE_HEIGHT};

use super::face::Face;
use super::vertex::{pack_vertex, Vertex};

/// Append the torch pole at the cell whose world origin is `(bx, by, bz)`, oriented
/// by `placement`, textured with `side_tile` (body) + `top_tile` (flame), tinted by
/// `tint`, and flat-lit at the packed channels: `sky6` (cell skylight, dims with
/// the environment sky scale) and `block` (the torch's own emission COLOUR —
/// night-invariant, so the pole keeps glowing in a dark cave / at night).
#[allow(clippy::too_many_arguments)]
pub(super) fn emit_torch(
    opaque: &mut Vec<Vertex>,
    bx: f32,
    by: f32,
    bz: f32,
    placement: TorchPlacement,
    side_tile: Tile,
    top_tile: Tile,
    tint: [f32; 3],
    sky6: u32,
    block: BlockLight6,
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
        for (corner, lp) in face.quad_box(lo, hi).into_iter().enumerate() {
            let wp = origin + xform.transform_point3(Vec3::new(lp[0], lp[1], lp[2]));
            opaque.push(Vertex {
                pos: [wp.x, wp.y, wp.z],
                tint: block.tint_word(tint),
                // Flat-lit (shade index 0, AO 3) like a cross-plant: no overlay.
                packed: pack_vertex(tile.index() as u32, corner as u32, 0, false, 3, sky6)
                    | block.packed_bits(),
                packed2: block.packed2_bits(),
            });
        }
    }
}
