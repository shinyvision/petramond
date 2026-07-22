use crate::atlas::Tile;
use crate::block::{Aabb, ShapeFamily};

use super::super::face::{crop_quads, cross_quads, Face};
use super::super::plane::{push_plane_quad, PlaneLight};
use super::super::vertex::{pack_tint, pack_vertex, pack_vertex2, Vertex};

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
    opaque_idx: &mut Vec<u32>,
    shape: ShapeFamily,
    bx: f32,
    y: f32,
    bz: f32,
    tile: Tile,
    tint: [f32; 3],
    sky6: u32,
    block6: u32,
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

/// Emit a Layer-3 custom shape's baked RENDER boxes (from the section's render
/// bake cache) into the opaque buffer — each box drawn face-by-face through the
/// shared [`push_plane_quad`] (cell-local UV, directional face shade, sun
/// normal, single-winding back-face cull), so a baked furniture piece textures
/// and lights exactly like a stair carved from its own block tiles. A box's
/// top/bottom faces sample the block's `[top, bottom]` tiles, the four sides the
/// `side` tile — the ordinary block convention. Lighting is flat across the cell
/// (a one-cell shape has no meaningful per-vertex gradient).
#[allow(clippy::too_many_arguments)]
pub(super) fn emit_baked_boxes(
    opaque: &mut Vec<Vertex>,
    opaque_idx: &mut Vec<u32>,
    wx: i32,
    wy: i32,
    wz: i32,
    boxes: &[Aabb],
    tiles: [Tile; 3],
    tint: [f32; 3],
    sky6: u32,
    block6: u32,
    warm: f32,
    // Whether the neighbour across a given face is a full opaque occluder — a
    // box face flush to that cell boundary is buried and gets culled.
    neighbor_opaque: impl Fn(Face) -> bool,
) {
    let light = PlaneLight {
        ao: [3; 4],
        sky: [sky6; 4],
        block: [block6; 4],
        warm: [warm; 4],
    };
    for b in boxes {
        for face in Face::ALL {
            if box_face_flush(b, face) && neighbor_opaque(face) {
                continue;
            }
            let slot = match face {
                Face::PosY => 0,
                Face::NegY => 1,
                _ => 2,
            };
            push_plane_quad(
                opaque, opaque_idx, wx, wy, wz, b.min, b.max, face, tiles[slot], tint, &light,
            );
        }
    }
}

/// Whether a box's `face` sits on (or past) the cell boundary — the only faces
/// eligible for neighbour culling (an interior face is always drawn).
#[inline]
fn box_face_flush(b: &Aabb, face: Face) -> bool {
    match face {
        Face::PosX => b.max[0] >= 1.0,
        Face::NegX => b.min[0] <= 0.0,
        Face::PosY => b.max[1] >= 1.0,
        Face::NegY => b.min[1] <= 0.0,
        Face::PosZ => b.max[2] >= 1.0,
        Face::NegZ => b.min[2] <= 0.0,
    }
}
