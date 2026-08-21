use crate::vertex::BlockLightVertexExt;
use petramond_world::block::ShapeFamily;
use petramond_world::tile::Tile;

use super::super::face::{crop_quads, cross_quads};
use petramond_world::light::BlockLight6;

use super::super::vertex::{pack_vertex, Vertex};

/// Emit a billboard plant — the X cross (two diagonal quads) or the planted
/// crop lattice (four axis-aligned quads, see `crop_quads`) — into the opaque
/// (cutout) buffer, each plane drawn in BOTH windings so the plant is visible
/// from both sides under back-face culling. Flat-lit (AO = 3, shade index 0 =
/// "top", no directional darkening), biome-tinted for grass/fern;
/// `fs_opaque`'s alpha discard handles the transparent texels exactly like
/// leaves.
#[allow(clippy::too_many_arguments)]
pub(super) fn emit_plant(
    opaque: &mut Vec<Vertex>,
    shape: ShapeFamily,
    bx: f32,
    y: f32,
    bz: f32,
    tile: Tile,
    tint: [f32; 3],
    sky6: u32,
    block: BlockLight6,
    inset: f32,
    drop: f32,
) {
    let cross;
    let crop;
    let planes: &[[[f32; 3]; 4]] = if shape == ShapeFamily::Crop {
        crop = crop_quads(bx, y, bz, inset, drop);
        &crop
    } else {
        cross = cross_quads(bx, y, bz, inset);
        &cross
    };
    // Flat-lit: shade index 0 (top, no directional darkening), AO = 3, no overlay;
    // `pack_vertex` and `BlockLight6` own the bit layouts.
    for plane in planes {
        let start = opaque.len() as u32;
        for (corner, p) in plane.iter().enumerate() {
            opaque.push(Vertex {
                pos: *p,
                tint: block.tint_word(tint),
                packed: pack_vertex(tile.index() as u32, corner as u32, 0, false, 3, sky6)
                    | block.packed_bits(),
                packed2: block.packed2_bits(),
            });
        }
        // A plant plane is seen from both sides.
        crate::vertex::push_back_face(opaque, start);
    }
}
