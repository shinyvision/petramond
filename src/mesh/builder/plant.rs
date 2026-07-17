use crate::atlas::Tile;
use crate::block::RenderShape;

use super::super::face::{crop_quads, cross_quads};
use super::super::vertex::{pack_tint, pack_vertex, pack_vertex2, Vertex};

/// Emit a billboard plant — the X cross (two diagonal quads) or the planted
/// crop lattice (four axis-aligned quads, see `crop_quads`) — into the opaque
/// (cutout) buffer, each plane drawn in BOTH windings so the plant is visible
/// from both sides under back-face culling. Flat-lit (AO = 3, shade index 0 =
/// "top", no directional darkening), biome-tinted for grass/fern;
/// `fs_opaque`'s alpha discard handles the transparent texels exactly like
/// leaves.
pub(super) fn emit_plant(
    opaque: &mut Vec<Vertex>,
    opaque_idx: &mut Vec<u32>,
    shape: RenderShape,
    bx: f32,
    y: f32,
    bz: f32,
    tile: Tile,
    tint: [f32; 3],
    sky6: u32,
    block6: u32,
) {
    let cross;
    let crop;
    let planes: &[[[f32; 3]; 4]] = if shape == RenderShape::Crop {
        crop = crop_quads(bx, y, bz);
        &crop
    } else {
        cross = cross_quads(bx, y, bz);
        &cross
    };
    // Flat-lit: shade index 0 (top, no directional darkening), AO = 3, no overlay;
    // `pack_vertex`/`pack_vertex2` own the bit layouts.
    for plane in planes {
        let start = opaque.len() as u32;
        for (corner, p) in plane.iter().enumerate() {
            opaque.push(Vertex {
                pos: *p,
                tint: pack_tint(tint),
                packed: pack_vertex(tile.index() as u32, corner as u32, 0, 0, false, 3, sky6),
                packed2: pack_vertex2(block6),
            });
        }
        opaque_idx.extend_from_slice(&[start, start + 1, start + 2, start, start + 2, start + 3]);
        opaque_idx.extend_from_slice(&[start, start + 2, start + 1, start, start + 3, start + 2]);
    }
}
