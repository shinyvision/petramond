use glam::Vec3;
use petramond_world::tile::Tile;
use petramond_world::torch::{TorchPlacement, POLE_HALF, POLE_HEIGHT};

use super::face::Face;
use super::face_emit::FlatLit;
use super::vertex::Vertex;

pub(super) fn emit_torch(
    opaque: &mut Vec<Vertex>,
    origin: Vec3,
    placement: TorchPlacement,
    side_tile: Tile,
    top_tile: Tile,
    lit: FlatLit,
) {
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
            opaque.push(lit.vertex(wp.to_array(), tile, corner as u32));
        }
    }
}
