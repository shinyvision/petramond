use glam::Vec3;
use petramond_world::block::PlantPlanes;
use petramond_world::tile::Tile;

use super::super::face::{crop_quads, cross_quads};
use super::super::face_emit::FlatLit;
use super::super::vertex::Vertex;

/// X-cross or crop-lattice plant (see `crop_quads`) into the cutout buffer. Both windings get
/// drawn so it shows from either side. No directional shading, and grass and fern take the
/// biome tint. `base` is the cell's mesh-space origin.
pub(super) fn emit_plant(
    opaque: &mut Vec<Vertex>,
    layout: PlantPlanes,
    base: Vec3,
    tile: Tile,
    lit: FlatLit,
    inset: f32,
    drop: f32,
) {
    let Vec3 { x: bx, y, z: bz } = base;
    let cross;
    let crop;
    let planes: &[[[f32; 3]; 4]] = match layout {
        PlantPlanes::Crop => {
            crop = crop_quads(bx, y, bz, inset, drop);
            &crop
        }
        PlantPlanes::Cross => {
            cross = cross_quads(bx, y, bz, inset);
            &cross
        }
    };
    for plane in planes {
        let start = opaque.len() as u32;
        for (corner, p) in plane.iter().enumerate() {
            opaque.push(lit.vertex(*p, tile, corner as u32));
        }
        crate::vertex::push_back_face(opaque, start);
    }
}
