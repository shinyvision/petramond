//! In-world geometry for a glass pane: a thin full-height post growing arms
//! toward its connected sides (see `crate::pane` for the connection rules).
//!
//! [`shape_boxes`] is the ONE geometry source, a NON-overlapping box
//! decomposition: arms on one axis merge into a single run (a straight wall
//! of panes is two long broad faces per cell, not per-arm strips); when runs
//! cross, the north-south run keeps the post and the east-west arms butt
//! against it. The chunk mesher wraps the boxes into the unified
//! [`super::boxset`] emitter — cap culling against the pane above/below, the
//! junction's interior faces, everything buried is removed geometrically —
//! and the break-crack overlay walks the same boxes through [`shape_faces`].
//! Faces at a CONNECTED cell edge are declared never-emitted (the neighbour
//! pane's run continues flush, or the connected block's complete face hides
//! them — the connection RULE guarantees it, not local geometry).
//!
//! Unconnected ends, and the sides of a bare post, show the pane edge tile
//! (the 2px strip), which also caps each box top/bottom. Smooth-lit like
//! every box family (2026-07-23), so a glass wall shades continuously with
//! the terrain it meets.

use crate::atlas::Tile;
use crate::pane::{EAST, NORTH, SOUTH, WEST};

use super::boxset::{FaceStyle, MeshBox};
use super::face::Face;

/// Which tile a pane face samples: the glass sheet, or the 2px edge strip.
#[derive(Copy, Clone, PartialEq, Eq)]
pub(crate) enum PaneTile {
    Glass,
    Edge,
}

/// One box of the connected pane shape.
#[derive(Copy, Clone, PartialEq, Eq)]
pub(crate) enum PaneBox {
    /// The bare centre post (no connections).
    Post,
    /// The north-south run, post included.
    ZRun,
    /// The east-west run, post included (no crossing north-south run).
    XRun,
    /// A west/east arm butting against a crossing north-south run.
    ArmW,
    ArmE,
}

/// Visit every box of the connected pane shape for `mask` — non-overlapping
/// by construction. The SINGLE geometry source for the chunk mesher, the
/// break-crack overlay, and neighbour-occupancy queries.
pub(crate) fn shape_boxes(
    post_lo: f32,
    post_hi: f32,
    mask: u8,
    mut visit: impl FnMut([f32; 3], [f32; 3], PaneBox),
) {
    let (w, e) = (mask & WEST != 0, mask & EAST != 0);
    let (n, s) = (mask & NORTH != 0, mask & SOUTH != 0);
    let z_run = n || s;
    let x_run = w || e;
    let z0 = if n { 0.0 } else { post_lo };
    let z1 = if s { 1.0 } else { post_hi };
    let x0 = if w { 0.0 } else { post_lo };
    let x1 = if e { 1.0 } else { post_hi };

    if !z_run && !x_run {
        visit([post_lo, 0.0, post_lo], [post_hi, 1.0, post_hi], PaneBox::Post);
        return;
    }
    if z_run {
        visit([post_lo, 0.0, z0], [post_hi, 1.0, z1], PaneBox::ZRun);
        if w {
            visit([0.0, 0.0, post_lo], [post_lo, 1.0, post_hi], PaneBox::ArmW);
        }
        if e {
            visit([post_hi, 0.0, post_lo], [1.0, 1.0, post_hi], PaneBox::ArmE);
        }
    } else {
        visit([x0, 0.0, post_lo], [x1, 1.0, post_hi], PaneBox::XRun);
    }
}

/// The per-face styling of one pane box: which tile it samples, whether the
/// edge strip's u/v swap to lie along the run, or `None` for a face the pane
/// never emits (a connected end). Shared by the mesher wrap and the overlay.
fn face_styles(kind: PaneBox, mask: u8) -> [Option<(PaneTile, bool)>; 6] {
    let (w, e) = (mask & WEST != 0, mask & EAST != 0);
    let (n, s) = (mask & NORTH != 0, mask & SOUTH != 0);
    let edge = Some((PaneTile::Edge, false));
    let glass = Some((PaneTile::Glass, false));
    let end = |connected: bool| if connected { None } else { edge };
    let mut f = [None; 6];
    match kind {
        PaneBox::Post => {
            f[Face::NegX as usize] = edge;
            f[Face::PosX as usize] = edge;
            f[Face::NegZ as usize] = edge;
            f[Face::PosZ as usize] = edge;
        }
        PaneBox::ZRun => {
            f[Face::NegX as usize] = glass;
            f[Face::PosX as usize] = glass;
            f[Face::NegZ as usize] = end(n);
            f[Face::PosZ as usize] = end(s);
        }
        PaneBox::XRun => {
            f[Face::NegZ as usize] = glass;
            f[Face::PosZ as usize] = glass;
            f[Face::NegX as usize] = end(w);
            f[Face::PosX as usize] = end(e);
        }
        // Arms exist only when their side is connected: the outer end is a
        // connected cell edge (never emitted), the inner end butts into the
        // crossing run (buried; kept omitted so the overlay matches).
        PaneBox::ArmW | PaneBox::ArmE => {
            f[Face::NegZ as usize] = glass;
            f[Face::PosZ as usize] = glass;
        }
    }
    // Caps: the edge strip runs vertically (u = the thin post span), so
    // boxes running along X swap u/v to lay the strip along the arm.
    let cap_swap = matches!(kind, PaneBox::XRun | PaneBox::ArmW | PaneBox::ArmE);
    f[Face::PosY as usize] = Some((PaneTile::Edge, cap_swap));
    f[Face::NegY as usize] = Some((PaneTile::Edge, cap_swap));
    f
}

/// Visit every drawable face of the connected pane shape — consumed by the
/// break-crack overlay so the crack traces the mesh's faces (the overlay
/// draws them all; the mesher's generic burial cull decides visibility).
pub(crate) fn shape_faces(
    post_lo: f32,
    post_hi: f32,
    mask: u8,
    mut visit: impl FnMut([f32; 3], [f32; 3], Face, PaneTile, bool),
) {
    shape_boxes(post_lo, post_hi, mask, |min, max, kind| {
        let styles = face_styles(kind, mask);
        for face in Face::ALL {
            if let Some((tile, swap_uv)) = styles[face as usize] {
                visit(min, max, face, tile, swap_uv);
            }
        }
    });
}

/// The connected pane shape as [`MeshBox`]es for the unified emitter.
#[allow(clippy::too_many_arguments)]
pub(super) fn push_mesh_boxes(
    out: &mut Vec<MeshBox>,
    post_lo: f32,
    post_hi: f32,
    mask: u8,
    glass_tile: Tile,
    edge_tile: Tile,
    tint: [f32; 3],
) {
    shape_boxes(post_lo, post_hi, mask, |min, max, kind| {
        let styles = face_styles(kind, mask);
        let faces = styles.map(|s| {
            s.map(|(tile, swap_uv)| FaceStyle {
                tile: match tile {
                    PaneTile::Glass => glass_tile,
                    PaneTile::Edge => edge_tile,
                },
                swap_uv,
                tint,
            })
        });
        out.push(MeshBox { min, max, faces });
    });
}
