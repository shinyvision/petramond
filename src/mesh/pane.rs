//! In-world geometry for a glass pane: a thin full-height post growing arms
//! toward its connected sides (see `crate::pane` for the connection rules).
//!
//! Arms on one axis merge into a single run so a straight wall of panes is two
//! long broad faces per cell, not per-arm strips. Faces at a connected cell edge
//! are never emitted — the neighbour pane's run continues flush, or the
//! neighbouring full face hides them — so glass reads as one continuous sheet.
//! Unconnected ends, and the sides of a bare post, show the pane edge tile (the
//! 2px strip), which also caps every segment top/bottom. Caps against a
//! same-block pane above/below are culled per segment: only where the vertical
//! neighbour actually continues that arm, so a longer arm below a shorter one
//! keeps its exposed cap. Flat-lit at the cell's own light with directional face
//! shade, like a thin object should be (per-corner AO would smear).

use crate::atlas::Tile;
use crate::pane::{EAST, HI, LO, NORTH, SOUTH, WEST};
use crate::torch::warm_tint;

use super::face::Face;
use super::plane::cell_uv;
use super::vertex::{
    pack_cell_uv, pack_normal_code, pack_tint, pack_vertex, pack_vertex2, Vertex,
    UV_MODE_CELL_LOCAL,
};
use super::UV_MODE_SHIFT;

/// What sits directly above/below a pane cell, for per-segment cap culling.
#[derive(Copy, Clone)]
pub(super) enum PaneVertical {
    /// An opaque cube or full slab stack: every cap is hidden.
    Solid,
    /// The same pane block, with ITS resolved connection mask: the post and any
    /// continued arm hide their caps; an arm the neighbour lacks keeps its cap.
    Pane(u8),
    /// Anything else: all caps are exposed.
    Open,
}

impl PaneVertical {
    /// Whether this vertical neighbour buries the cap of segment `seg`
    /// (`0` = the centre post, else an arm's mask bit).
    fn hides_cap(self, seg: u8) -> bool {
        match self {
            PaneVertical::Solid => true,
            PaneVertical::Pane(mask) => seg == 0 || mask & seg != 0,
            PaneVertical::Open => false,
        }
    }
}

/// Which tile a pane face samples: the glass sheet, or the 2px edge strip.
#[derive(Copy, Clone, PartialEq, Eq)]
pub(crate) enum PaneTile {
    Glass,
    Edge,
}

/// Visit every face of the connected pane shape for `mask`, in the mesher's
/// emission order — the SINGLE face list shared by the chunk mesher and the
/// break-crack overlay, so the crack is coincident with the mesh.
/// `visit(min, max, face, tile, swap_uv, seg)`: `swap_uv` lays the edge strip
/// along west/east arm caps; `seg` is the segment bit a PosY/NegY cap belongs
/// to (`0` = the centre post; unused for side faces) — the mesher culls caps
/// against the vertical neighbours per segment, the overlay draws them all.
pub(crate) fn shape_faces(
    mask: u8,
    mut visit: impl FnMut([f32; 3], [f32; 3], Face, PaneTile, bool, u8),
) {
    let (w, e) = (mask & WEST != 0, mask & EAST != 0);
    let (n, s) = (mask & NORTH != 0, mask & SOUTH != 0);
    let z_run = n || s;
    let x_run = w || e;
    let z0 = if n { 0.0 } else { LO };
    let z1 = if s { 1.0 } else { HI };
    let x0 = if w { 0.0 } else { LO };
    let x1 = if e { 1.0 } else { HI };

    if mask == 0 {
        // A bare post: four thin edge-strip sides.
        let post = ([LO, 0.0, LO], [HI, 1.0, HI]);
        for face in [Face::NegX, Face::PosX, Face::NegZ, Face::PosZ] {
            visit(post.0, post.1, face, PaneTile::Edge, false, 0);
        }
    }
    if z_run {
        // The north-south run's broad glass faces, post included.
        let (min, max) = ([LO, 0.0, z0], [HI, 1.0, z1]);
        visit(min, max, Face::NegX, PaneTile::Glass, false, 0);
        visit(min, max, Face::PosX, PaneTile::Glass, false, 0);
        // Unconnected ends show the edge strip — unless a crossing east-west run
        // exists, whose broad faces already cover those planes.
        if !x_run {
            if !n {
                visit(min, max, Face::NegZ, PaneTile::Edge, false, 0);
            }
            if !s {
                visit(min, max, Face::PosZ, PaneTile::Edge, false, 0);
            }
        }
    }
    if x_run {
        let (min, max) = ([x0, 0.0, LO], [x1, 1.0, HI]);
        visit(min, max, Face::NegZ, PaneTile::Glass, false, 0);
        visit(min, max, Face::PosZ, PaneTile::Glass, false, 0);
        if !z_run {
            if !w {
                visit(min, max, Face::NegX, PaneTile::Edge, false, 0);
            }
            if !e {
                visit(min, max, Face::PosX, PaneTile::Edge, false, 0);
            }
        }
    }

    // Top/bottom caps, one per occupied segment so crossing runs never overlap.
    // The edge tile's 2px strip runs vertically (u = the thin 7..9/16 span), so
    // north/south arm caps map with plain cell UVs while west/east arm caps swap
    // u/v to lay the strip along the arm.
    let post = ([LO, 0.0, LO], [HI, 1.0, HI]);
    // (present, cap box min/max, arm segment bit, swap edge-strip u/v).
    type CapSegment = (bool, ([f32; 3], [f32; 3]), u8, bool);
    let segments: [CapSegment; 5] = [
        (true, post, 0, false),
        (n, ([LO, 0.0, 0.0], [HI, 1.0, LO]), NORTH, false),
        (s, ([LO, 0.0, HI], [HI, 1.0, 1.0]), SOUTH, false),
        (w, ([0.0, 0.0, LO], [LO, 1.0, HI]), WEST, true),
        (e, ([HI, 0.0, LO], [1.0, 1.0, HI]), EAST, true),
    ];
    for (present, (min, max), seg, swap_uv) in segments {
        if !present {
            continue;
        }
        visit(min, max, Face::PosY, PaneTile::Edge, swap_uv, seg);
        visit(min, max, Face::NegY, PaneTile::Edge, swap_uv, seg);
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn emit_pane_block(
    opaque: &mut Vec<Vertex>,
    opaque_idx: &mut Vec<u32>,
    wx: i32,
    wy: i32,
    wz: i32,
    mask: u8,
    above: PaneVertical,
    below: PaneVertical,
    glass_tile: Tile,
    edge_tile: Tile,
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
    shape_faces(mask, |min, max, face, pane_tile, swap_uv, seg| {
        let hidden = match face {
            Face::PosY => above.hides_cap(seg),
            Face::NegY => below.hides_cap(seg),
            _ => false,
        };
        if hidden {
            return;
        }
        let tile = match pane_tile {
            PaneTile::Glass => glass_tile,
            PaneTile::Edge => edge_tile,
        };
        push_face(
            opaque, opaque_idx, origin, min, max, face, tile, swap_uv, tint, sky6, block6,
        );
    });
}

/// One flat quad from a cell-local `(min, max)` box + a [`Face`], with
/// cell-local UVs and flat lighting — shared with the ladder mesher, which
/// emits the same kind of thin cutout panel faces.
#[allow(clippy::too_many_arguments)]
pub(super) fn push_face(
    vbuf: &mut Vec<Vertex>,
    ibuf: &mut Vec<u32>,
    origin: [f32; 3],
    min: [f32; 3],
    max: [f32; 3],
    face: Face,
    tile: Tile,
    swap_uv: bool,
    tint: [f32; 3],
    sky6: u32,
    block6: u32,
) {
    let world_min = [origin[0] + min[0], origin[1] + min[1], origin[2] + min[2]];
    let world_max = [origin[0] + max[0], origin[1] + max[1], origin[2] + max[2]];
    let corners = face.quad_box(world_min, world_max);
    let local = face.quad_box(min, max);
    let start = vbuf.len() as u32;
    for (corner, pos) in corners.into_iter().enumerate() {
        let [mut u, mut v] = cell_uv(face, local[corner]);
        if swap_uv {
            std::mem::swap(&mut u, &mut v);
        }
        vbuf.push(Vertex {
            pos,
            tint: pack_tint(tint),
            // Flat AO (3 = unoccluded) with the face's directional shade: thin
            // glass reads best evenly lit, but still distinguishes its sides.
            packed: pack_vertex(
                tile.index() as u32,
                corner as u32,
                face.shade_idx(),
                0,
                false,
                3,
                sky6,
            ) | (UV_MODE_CELL_LOCAL << UV_MODE_SHIFT),
            packed2: pack_vertex2(block6)
                | pack_cell_uv((u * 16.0).round() as u32, (v * 16.0).round() as u32)
                | pack_normal_code(face.normal_code()),
        });
    }
    ibuf.extend_from_slice(&[start, start + 1, start + 2, start, start + 2, start + 3]);
}
