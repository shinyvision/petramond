//! In-world geometry for a fence: a centre post growing a pair of horizontal
//! rails toward each connected side (see `crate::fence` for the rules).
//!
//! Rails run from the cell edge to the post face and emit only their four long
//! faces — never end caps: the cell-edge end continues into the neighbour
//! fence's rail (or hides against the connected block's face), the post end is
//! covered by the post's own side face. Stacked fences hide the shared post
//! cap; rail faces are never cap-culled (they never touch a cell boundary).
//! Flat-lit at the cell's own light with directional face shade, like the pane
//! (per-corner AO would smear on thin geometry).

use crate::atlas::Tile;
use crate::fence::{rail_cross, RAIL_BOT_HI, RAIL_BOT_LO, RAIL_TOP_HI, RAIL_TOP_LO};
use crate::pane::{EAST, NORTH, SOUTH, WEST};
use crate::torch::warm_tint;

use super::face::Face;
use super::vertex::Vertex;

/// What sits directly above/below a fence cell, for post-cap culling.
#[derive(Copy, Clone)]
pub(super) enum FenceVertical {
    /// An opaque cube or full slab stack: the post cap is hidden.
    Solid,
    /// Another fence: its post continues through the boundary.
    Fence,
    /// Anything else: the cap is exposed.
    Open,
}

impl FenceVertical {
    #[inline]
    fn hides_post_cap(self) -> bool {
        !matches!(self, FenceVertical::Open)
    }
}

/// Visit every face of the connected fence shape for `mask`, in the mesher's
/// emission order — the SINGLE face list shared by the chunk mesher and the
/// break-crack overlay, so the crack is coincident with the mesh.
/// `visit(min, max, face, post_cap)`: `post_cap` marks the post's PosY/NegY
/// caps — the only faces the mesher culls against the vertical neighbours
/// (the overlay draws them all).
pub(crate) fn shape_faces(
    post_lo: f32,
    post_hi: f32,
    mask: u8,
    mut visit: impl FnMut([f32; 3], [f32; 3], Face, bool),
) {
    let post = ([post_lo, 0.0, post_lo], [post_hi, 1.0, post_hi]);
    for face in [Face::NegX, Face::PosX, Face::NegZ, Face::PosZ] {
        visit(post.0, post.1, face, false);
    }
    visit(post.0, post.1, Face::PosY, true);
    visit(post.0, post.1, Face::NegY, true);

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
            for face in [
                Face::NegX,
                Face::PosX,
                Face::NegY,
                Face::PosY,
                Face::NegZ,
                Face::PosZ,
            ] {
                // Skip the two end faces perpendicular to the arm: the edge end
                // continues into the neighbour, the post end is buried.
                let end_face = match face {
                    Face::NegX | Face::PosX => along_x,
                    Face::NegZ | Face::PosZ => !along_x,
                    _ => false,
                };
                if !end_face {
                    visit(min, max, face, false);
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn emit_fence_block(
    opaque: &mut Vec<Vertex>,
    opaque_idx: &mut Vec<u32>,
    wx: i32,
    wy: i32,
    wz: i32,
    post_lo: f32,
    post_hi: f32,
    mask: u8,
    above: FenceVertical,
    below: FenceVertical,
    tiles: [Tile; 3],
    tint: [f32; 3],
    sky6: u32,
    block6: u32,
    warm: f32,
) {
    let origin = [wx as f32, wy as f32, wz as f32];
    let tint = if warm == 0.0 {
        tint
    } else {
        warm_tint(tint, warm)
    };
    shape_faces(post_lo, post_hi, mask, |min, max, face, post_cap| {
        let hidden = match face {
            Face::PosY => post_cap && above.hides_post_cap(),
            Face::NegY => post_cap && below.hides_post_cap(),
            _ => false,
        };
        if hidden {
            return;
        }
        let tile = match face {
            Face::PosY => tiles[0],
            Face::NegY => tiles[1],
            _ => tiles[2],
        };
        super::pane::push_face(
            opaque, opaque_idx, origin, min, max, face, tile, false, tint, sky6, block6,
        );
    });
}
