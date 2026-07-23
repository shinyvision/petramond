//! In-world geometry for a fence: a centre post growing a pair of horizontal
//! rails toward each connected side (see `crate::fence` for the rules).
//!
//! [`shape_boxes`] is the ONE geometry source: the chunk mesher wraps its
//! boxes into the unified [`super::boxset`] emitter (which culls buried
//! faces — a rail end inside the post, a post cap under a solid or a stacked
//! fence — geometrically), and the break-crack overlay walks the same boxes
//! through [`shape_faces`]. Rail end caps (the two faces perpendicular to
//! the arm) are declared never-emitted: the cell-edge end continues into the
//! neighbour fence's rail or hides against the connected block's face, the
//! post end is buried — the connection RULE guarantees this, not local
//! geometry, so it stays an explicit omission.
//! Flat-lit at the cell's own light with directional face shade, like the
//! pane (per-corner AO would smear on thin geometry).

use crate::atlas::Tile;
use crate::fence::{rail_cross, RAIL_BOT_HI, RAIL_BOT_LO, RAIL_TOP_HI, RAIL_TOP_LO};
use crate::pane::{EAST, NORTH, SOUTH, WEST};

use super::boxset::{FaceStyle, MeshBox};
use super::face::Face;

/// One box of the connected fence shape.
#[derive(Copy, Clone, PartialEq, Eq)]
pub(crate) enum FenceBox {
    Post,
    /// A rail, running along X (`true`) or Z (`false`).
    Rail(bool),
}

/// Visit every box of the connected fence shape for `mask`: the post, then
/// two rails per connected side. The SINGLE geometry source for the chunk
/// mesher, the break-crack overlay, and neighbour-occupancy queries.
pub(crate) fn shape_boxes(
    post_lo: f32,
    post_hi: f32,
    mask: u8,
    mut visit: impl FnMut([f32; 3], [f32; 3], FenceBox),
) {
    visit([post_lo, 0.0, post_lo], [post_hi, 1.0, post_hi], FenceBox::Post);

    // The rail cross-section tracks the post (a modded wall keeps rails on its
    // own post), not fixed engine constants.
    let (rail_lo, rail_hi) = rail_cross(post_lo, post_hi);
    // (side bit, arm runs along X?, arm span from cell edge to post face).
    for (bit, along_x, from, to) in [
        (NORTH, false, 0.0, post_lo),
        (SOUTH, false, post_hi, 1.0),
        (WEST, true, 0.0, post_lo),
        (EAST, true, post_hi, 1.0),
    ] {
        if mask & bit == 0 {
            continue;
        }
        for (y_lo, y_hi) in [(RAIL_BOT_LO, RAIL_BOT_HI), (RAIL_TOP_LO, RAIL_TOP_HI)] {
            let (min, max) = if along_x {
                ([from, y_lo, rail_lo], [to, y_hi, rail_hi])
            } else {
                ([rail_lo, y_lo, from], [rail_hi, y_hi, to])
            };
            visit(min, max, FenceBox::Rail(along_x));
        }
    }
}

/// Whether `face` is a rail's never-emitted end cap (perpendicular to the
/// arm — see the module doc).
#[inline]
fn rail_end(kind: FenceBox, face: Face) -> bool {
    match kind {
        FenceBox::Post => false,
        FenceBox::Rail(along_x) => match face {
            Face::NegX | Face::PosX => along_x,
            Face::NegZ | Face::PosZ => !along_x,
            _ => false,
        },
    }
}

/// Visit every drawable face of the connected fence shape, in emission order
/// — consumed by the break-crack overlay so the crack traces the mesh's
/// faces. `post_cap` marks the post's PosY/NegY caps (the overlay draws them
/// all; the mesher's generic burial cull handles their visibility).
pub(crate) fn shape_faces(
    post_lo: f32,
    post_hi: f32,
    mask: u8,
    mut visit: impl FnMut([f32; 3], [f32; 3], Face, bool),
) {
    shape_boxes(post_lo, post_hi, mask, |min, max, kind| {
        for face in Face::ALL {
            if rail_end(kind, face) {
                continue;
            }
            let post_cap = kind == FenceBox::Post && matches!(face, Face::PosY | Face::NegY);
            visit(min, max, face, post_cap);
        }
    });
}

/// The connected fence shape as [`MeshBox`]es for the unified emitter:
/// `[top, bottom, side]` tiles, rail end caps omitted.
pub(super) fn push_mesh_boxes(
    out: &mut Vec<MeshBox>,
    post_lo: f32,
    post_hi: f32,
    mask: u8,
    tiles: [Tile; 3],
    tint: [f32; 3],
) {
    shape_boxes(post_lo, post_hi, mask, |min, max, kind| {
        let style = |tile: Tile| {
            Some(FaceStyle {
                tile,
                swap_uv: false,
                tint,
            })
        };
        let mut faces = [style(tiles[2]); 6];
        faces[Face::PosY as usize] = style(tiles[0]);
        faces[Face::NegY as usize] = style(tiles[1]);
        for face in Face::ALL {
            if rail_end(kind, face) {
                faces[face as usize] = None;
            }
        }
        out.push(MeshBox { min, max, faces });
    });
}
