//! The ONE box-set emitter every axis-aligned block shape meshes through.
//!
//! Every block shape in the game whose geometry is a set of axis-aligned
//! cuboids — the full cube is the degenerate one-box case, stairs and slabs
//! are half-cell box sets, fences/panes/ladders are thin box sets, Layer-3
//! custom shapes are baked box sets — shares one characteristic: every face
//! is a rectangle on an axis-aligned plane. This module meshes that
//! characteristic once, so per-family emitters carry no culling or lighting
//! logic of their own:
//!
//! - **Hidden-surface removal is geometric, not per-family policy.** A face
//!   is emitted only where no matter is SEALED FLUSH against it: sibling
//!   boxes of the same cell (butted contact faces vanish; coincident
//!   same-direction faces keep exactly one winner by box order; merely
//!   interpenetrating volumes deliberately do NOT hide — see
//!   [`push_occluder`]), the neighbour cell's own box set for faces flush on
//!   the cell boundary (a fence post cap on a slab, a chain continuing into
//!   the chain above), and the classic whole-face cull against a full opaque
//!   neighbour. The old per-family rules (fence post-cap enums, pane
//!   per-segment caps, stair/slab half-cell adjacency) are subsumed by this
//!   subtraction.
//! - **Lighting is the cube's, per plane.** The emitter gathers
//!   [`cube_face_lighting`] once per (face direction, boundary/interior
//!   plane) — the front voxel is the neighbour for a flush face, the cell
//!   itself for an interior one, exactly the stair/slab convention — and
//!   bilinearly samples it at every emitted corner, so a box face shades
//!   identically to a full cube face wherever their corners coincide.
//!   `NegY` planes stay flat-lit (the stair's closed-underside rule). Every
//!   box family is smooth-lit — the old flat-lit thin-shape policy predated
//!   plane-field sampling (its "AO smears on thin geometry" concern was an
//!   artifact of per-quad corner lighting) and is gone.
//! - **Self-AO and neighbour casting come from corner probes.** Each emitted
//!   corner probes its three front-side pockets (the sub-cell analogue of
//!   the grid ring's side/side/diagonal cells): a probe inside the cell
//!   tests the box set itself; a probe outside resolves through the shared
//!   `matter` query (opaque whole-cell, a neighbour's stair/slab occupancy,
//!   its box set...) — so for a full-cell box the probes reduce EXACTLY to
//!   grid vertex AO, and for sub-cell geometry the inner corners (a stair's
//!   riser/tread crease, a cauldron's cavity floor against its wall) darken
//!   with the same 0..3 AO vocabulary two abutting cubes would produce.
//!   Probes are lifted off the face plane, so two flush boxes forming one
//!   continuous surface never darken their shared seam (the model-AO rule).
//!   The cube gathers run the same probes against box-family ring cells
//!   ([`super::builder::corner_cast_probes`]), which is what makes a stair
//!   or cauldron CAST onto the terrain beside it.
//!
//! Emitted quads carry cell-local UVs (carved from the tile like a stair),
//! directional shade, and the standard packed vertex layout. Coplanar quads
//! from one subtraction share whole band edges; quads of one plane sample
//! one shared light field, so seams are invisible.

use crate::atlas::Tile;
use crate::block::Block;
use crate::block_state::SlabState;
use crate::torch::warm_tint;

use super::builder::{cube_face_lighting, face_axes};
use super::face::{quad_ao, should_flip, Face, FACES};
use super::plane::{cell_uv, PlaneLight};
use super::vertex::{
    pack_cell_uv, pack_normal_code, pack_tint, pack_vertex, pack_vertex2, Vertex,
    UV_MODE_CELL_LOCAL,
};
use super::UV_MODE_SHIFT;

/// How one face of a [`MeshBox`] is textured.
#[derive(Copy, Clone)]
pub(super) struct FaceStyle {
    pub tile: Tile,
    /// Swap the cell-local u/v (the pane edge strip laid along a W/E arm).
    pub swap_uv: bool,
    pub tint: [f32; 3],
}

/// One cell-local cuboid of a block's shape. `faces` is indexed by
/// `Face as usize` ([`Face::ALL`] order); `None` = the family never emits
/// that face (a fence rail's end caps), regardless of coverage.
#[derive(Copy, Clone)]
pub(super) struct MeshBox {
    pub min: [f32; 3],
    pub max: [f32; 3],
    pub faces: [Option<FaceStyle>; 6],
}

impl MeshBox {
    /// A box textured like a cube: `[top, bottom, side]` tiles, one tint
    /// per tile, plain UVs on every face.
    pub(super) fn uniform(
        min: [f32; 3],
        max: [f32; 3],
        tiles: [Tile; 3],
        tint_for: impl Fn(Tile) -> [f32; 3],
    ) -> Self {
        let style = |tile: Tile| {
            Some(FaceStyle {
                tile,
                swap_uv: false,
                tint: tint_for(tile),
            })
        };
        let mut faces = [style(tiles[2]); 6];
        faces[Face::PosY as usize] = style(tiles[0]);
        faces[Face::NegY as usize] = style(tiles[1]);
        MeshBox { min, max, faces }
    }
}

/// An axis-aligned rectangle in a face's (u, v) plane.
#[derive(Copy, Clone, Debug, PartialEq)]
struct Rect {
    u0: f32,
    v0: f32,
    u1: f32,
    v1: f32,
}

impl Rect {
    #[inline]
    fn is_empty(&self) -> bool {
        self.u1 - self.u0 <= AREA_EPS || self.v1 - self.v0 <= AREA_EPS
    }

    #[inline]
    fn clipped_to(&self, r: &Rect) -> Rect {
        Rect {
            u0: self.u0.max(r.u0),
            v0: self.v0.max(r.v0),
            u1: self.u1.min(r.u1),
            v1: self.v1.min(r.v1),
        }
    }
}

/// Coordinate tolerance for coverage/coincidence tests: bake and family
/// geometry is authored on the 1/16 grid (finer subdivisions stay far above
/// this), so 1e-4 cleanly separates "same plane" from "different plane".
const T: f32 = 1e-4;
/// A subtraction remainder thinner than this is dropped (degenerate sliver).
const AREA_EPS: f32 = 1e-4;
/// Probe lift off the face plane: keeps a coplanar continuation (two flush
/// boxes forming one surface) from shadowing its own seam, the model-AO rule.
/// Shared with the cube gathers' cast probes (`builder::corner_cast_probes`)
/// so casting and self-AO speak one geometry.
pub(super) const PROBE_LIFT: f32 = 0.02;
/// Probe tangential reach (1.5 texels): how close sub-cell geometry must be
/// to a corner to occlude it. Shared with the cube gathers' cast probes.
pub(super) const PROBE_REACH: f32 = 1.5 / 16.0;

/// The shared sub-cell AO occupancy oracle: does the world cell's matter
/// overlap the cell-local pocket AABB `(lo, hi)`?
pub(super) type MatterFn<'a> = dyn Fn((i32, i32, i32), [f32; 3], [f32; 3]) -> bool + 'a;

/// Fills `out` with a neighbour cell's occupancy boxes (neighbour-local).
pub(super) type NeighborBoxesFn<'a> = dyn Fn(Face, &mut Vec<([f32; 3], [f32; 3])>) + 'a;

/// Reusable scratch for [`emit_box_set`] — one per mesh build, so the hot
/// loop allocates nothing after warm-up.
#[derive(Default)]
pub(super) struct BoxSetScratch {
    occ: Vec<Rect>,
    cuts: Vec<f32>,
    runs: Vec<(f32, f32)>,
    rects: Vec<Rect>,
    nb: Vec<([f32; 3], [f32; 3])>,
}

/// Mesh one cell's box set. See the module doc for the model.
///
/// - `neighbor_solid(face)`: full opaque occupier across that boundary (the
///   classic whole-face cull).
/// - `neighbor_boxes(face, out)`: the neighbour cell's own occupancy boxes in
///   NEIGHBOUR-local coordinates, for sub-cell boundary culling. May push
///   nothing (unknown/none).
/// - `matter(cell, lo, hi)`: the shared sub-cell AO occupancy query (world
///   cell + cell-local pocket AABB) — the out-of-cell probe resolution AND
///   the plane gather's cast probe, so box shapes receive neighbour casting
///   with exactly the cube faces' semantics.
#[allow(clippy::too_many_arguments)]
pub(super) fn emit_box_set<B, S, L, K>(
    vbuf: &mut Vec<Vertex>,
    ibuf: &mut Vec<u32>,
    wx: i32,
    wy: i32,
    wz: i32,
    boxes: &[MeshBox],
    scratch: &mut BoxSetScratch,
    neighbor_solid: &dyn Fn(Face) -> bool,
    neighbor_boxes: &NeighborBoxesFn,
    matter: &MatterFn,
    block_at: &B,
    slab_at: &S,
    neighbour_light: &L,
    neighbour_blocklight: &K,
) where
    B: Fn(i32, i32, i32) -> Block,
    S: Fn(i32, i32, i32) -> Option<SlabState>,
    L: Fn(i32, i32, i32) -> u8,
    K: Fn(i32, i32, i32) -> u8,
{
    for face in FACES {
        let fi = face as usize;
        let (axis, ua, va) = face_axes(face);
        let positive = matches!(face, Face::PosX | Face::PosY | Face::PosZ);

        // Per-face lazies: the whole-face solid cull, the neighbour box
        // fetch, and the two plane light gathers (boundary / interior).
        let mut solid: Option<bool> = None;
        let mut nb_fetched = false;
        let mut plane_light: [Option<PlaneLight>; 2] = [None, None];

        for (i, b) in boxes.iter().enumerate() {
            let Some(style) = b.faces[fi] else { continue };
            let d = if positive { b.max[axis] } else { b.min[axis] };
            let flush = if positive { d >= 1.0 - T } else { d <= T };

            if flush {
                let s = match solid {
                    Some(s) => s,
                    None => *solid.insert(neighbor_solid(face)),
                };
                if s {
                    continue;
                }
                if !nb_fetched {
                    nb_fetched = true;
                    scratch.nb.clear();
                    neighbor_boxes(face, &mut scratch.nb);
                }
            }

            let rect = Rect {
                u0: b.min[ua],
                v0: b.min[va],
                u1: b.max[ua],
                v1: b.max[va],
            };

            // Everything covering the space just in front of this face.
            scratch.occ.clear();
            for (j, o) in boxes.iter().enumerate() {
                if j != i {
                    push_occluder(
                        &mut scratch.occ,
                        o.min,
                        o.max,
                        (axis, ua, va),
                        positive,
                        d,
                        j < i,
                        &rect,
                    );
                }
            }
            if flush {
                let shift = if positive { 1.0 } else { -1.0 };
                for &(nmin, nmax) in &scratch.nb {
                    let mut smin = nmin;
                    let mut smax = nmax;
                    smin[axis] += shift;
                    smax[axis] += shift;
                    push_occluder(
                        &mut scratch.occ,
                        smin,
                        smax,
                        (axis, ua, va),
                        positive,
                        d,
                        false,
                        &rect,
                    );
                }
            }

            subtract(
                rect,
                &scratch.occ,
                &mut scratch.cuts,
                &mut scratch.runs,
                &mut scratch.rects,
            );
            if scratch.rects.is_empty() {
                continue;
            }

            let pl_idx = flush as usize;
            if plane_light[pl_idx].is_none() {
                let (dx, dy, dz) = face.dir();
                let (fx, fy, fz) = if flush {
                    (wx + dx, wy + dy, wz + dz)
                } else {
                    (wx, wy, wz)
                };
                let (ao, sky, block, warm) = cube_face_lighting(
                    face,
                    fx,
                    fy,
                    fz,
                    neighbour_light(fx, fy, fz) as u32,
                    neighbour_blocklight(fx, fy, fz) as u32,
                    // The closed-underside rule (stairs, slabs): a NegY
                    // plane must not smooth sky from cells beside a dark
                    // cell below.
                    face != Face::NegY,
                    block_at,
                    slab_at,
                    neighbour_light,
                    neighbour_blocklight,
                    &matter,
                );
                plane_light[pl_idx] = Some(PlaneLight {
                    ao,
                    sky,
                    block,
                    warm,
                });
            }
            let pl = plane_light[pl_idx].as_ref().expect("just filled");

            for r_idx in 0..scratch.rects.len() {
                let r = scratch.rects[r_idx];
                let mut min3 = [0.0f32; 3];
                let mut max3 = [0.0f32; 3];
                min3[axis] = d;
                max3[axis] = d;
                min3[ua] = r.u0;
                max3[ua] = r.u1;
                min3[va] = r.v0;
                max3[va] = r.v1;
                let local = face.quad_box(min3, max3);

                let start = vbuf.len() as u32;
                let mut quad_ao = [3u32; 4];
                for (ci, lp) in local.into_iter().enumerate() {
                    let [u, v] = cell_uv(face, lp);
                    let (mut ao, sky6, block6, warm) = pl.sample(u, v);
                    ao = ao.min(probe_ao(
                        boxes,
                        lp,
                        (axis, ua, va),
                        positive,
                        &r,
                        (wx, wy, wz),
                        matter,
                    ));
                    quad_ao[ci] = ao;
                    let (mut uu, mut vv) = (u, v);
                    if style.swap_uv {
                        std::mem::swap(&mut uu, &mut vv);
                    }
                    let tint = if warm == 0.0 {
                        style.tint
                    } else {
                        warm_tint(style.tint, warm)
                    };
                    let quant =
                        |x: f32| ((x * 16.0).round() as i32).clamp(0, 16) as u32;
                    vbuf.push(Vertex {
                        pos: [
                            wx as f32 + lp[0],
                            wy as f32 + lp[1],
                            wz as f32 + lp[2],
                        ],
                        tint: pack_tint(tint),
                        packed: pack_vertex(
                            style.tile.index() as u32,
                            ci as u32,
                            face.shade_idx(),
                            0,
                            false,
                            ao,
                            sky6,
                        ) | (UV_MODE_CELL_LOCAL << UV_MODE_SHIFT),
                        packed2: pack_vertex2(block6)
                            | pack_cell_uv(quant(uu), quant(vv))
                            | pack_normal_code(face.normal_code()),
                    });
                }
                let tris: [u32; 6] = if should_flip(quad_ao) {
                    [0, 1, 3, 1, 2, 3]
                } else {
                    [0, 1, 2, 0, 2, 3]
                };
                ibuf.extend(tris.map(|t| start + t));
            }
        }
    }
}

/// If the box `(omin, omax)` hides part of a face at plane `d` (normal along
/// `axis`, facing `positive`), push its (u, v) footprint clipped to `rect`.
///
/// Hidden means SEALED CONTACT, one of:
/// - the box BUTTS flush against the face (its near bound lies on the face
///   plane and its volume extends in front) — the two surfaces coincide, so
///   nothing can ever show between them;
/// - `earlier` and the box's own same-direction face is COINCIDENT with this
///   plane while its volume lies behind — two overlapping boxes ending on
///   one plane emit that plane once (the earlier box wins; ties are the only
///   case subtraction alone cannot order).
///
/// A box merely STRADDLING the plane (interpenetration, like the chain's two
/// crossing plates) deliberately does NOT hide: box faces draw alpha-cutout
/// tiles, so a face inside another box's volume can still show through that
/// box's transparent texels — culling it would punch visible holes. Where the
/// straddling box is fully opaque the retained face is invisible overdraw,
/// which is harmless.
#[allow(clippy::too_many_arguments)]
fn push_occluder(
    occ: &mut Vec<Rect>,
    omin: [f32; 3],
    omax: [f32; 3],
    (axis, ua, va): (usize, usize, usize),
    positive: bool,
    d: f32,
    earlier: bool,
    rect: &Rect,
) {
    let (lo, hi) = (omin[axis], omax[axis]);
    let hides = if positive {
        ((lo - d).abs() <= T && hi > d + T) || (earlier && (hi - d).abs() <= T && lo < d - T)
    } else {
        ((hi - d).abs() <= T && lo < d - T) || (earlier && (lo - d).abs() <= T && hi > d + T)
    };
    if !hides {
        return;
    }
    let r = Rect {
        u0: omin[ua],
        v0: omin[va],
        u1: omax[ua],
        v1: omax[va],
    }
    .clipped_to(rect);
    if !r.is_empty() {
        occ.push(r);
    }
}

/// `rect` minus the union of `occ`, as maximal-ish rectangles: band the v
/// axis at every occluder edge, emit the uncovered u-runs per band, then
/// re-merge vertically adjacent runs with identical u-extents. Coplanar
/// output rects share whole band edges by construction.
fn subtract(
    rect: Rect,
    occ: &[Rect],
    cuts: &mut Vec<f32>,
    runs: &mut Vec<(f32, f32)>,
    out: &mut Vec<Rect>,
) {
    out.clear();
    if occ.is_empty() {
        out.push(rect);
        return;
    }

    cuts.clear();
    cuts.push(rect.v0);
    cuts.push(rect.v1);
    for o in occ {
        if o.v0 > rect.v0 + T {
            cuts.push(o.v0);
        }
        if o.v1 < rect.v1 - T {
            cuts.push(o.v1);
        }
    }
    cuts.sort_by(|a, b| a.partial_cmp(b).expect("finite cuts"));
    cuts.dedup_by(|a, b| (*a - *b).abs() <= T);

    for band in cuts.windows(2) {
        let (va, vb) = (band[0], band[1]);
        if vb - va <= AREA_EPS {
            continue;
        }
        // Occluder u-intervals overlapping this band, merged.
        runs.clear();
        for o in occ {
            if o.v0 <= va + T && o.v1 >= vb - T {
                runs.push((o.u0, o.u1));
            }
        }
        runs.sort_by(|a, b| a.0.partial_cmp(&b.0).expect("finite runs"));
        let mut cursor = rect.u0;
        let push_run = |u0: f32, u1: f32, out: &mut Vec<Rect>| {
            let r = Rect { u0, v0: va, u1, v1: vb };
            if !r.is_empty() {
                out.push(r);
            }
        };
        for &(o0, o1) in runs.iter() {
            if o0 > cursor + T {
                push_run(cursor, o0.min(rect.u1), out);
            }
            cursor = cursor.max(o1);
            if cursor >= rect.u1 - T {
                break;
            }
        }
        if cursor < rect.u1 - T {
            push_run(cursor, rect.u1, out);
        }
    }

    // Vertical re-merge: adjacent bands producing the same u-extent fuse
    // back into one rect (restores e.g. a pane's full broad face).
    let mut i = 0;
    while i < out.len() {
        let mut j = i + 1;
        while j < out.len() {
            let (a, b) = (out[i], out[j]);
            if (a.u0 - b.u0).abs() <= T
                && (a.u1 - b.u1).abs() <= T
                && ((a.v1 - b.v0).abs() <= T || (b.v1 - a.v0).abs() <= T)
            {
                out[i].v0 = a.v0.min(b.v0);
                out[i].v1 = a.v1.max(b.v1);
                out.swap_remove(j);
                j = i + 1; // rescan: the grown rect may fuse further
            } else {
                j += 1;
            }
        }
        i += 1;
    }
}

/// Corner-probe self-AO + received casting: the sub-cell generalization of
/// grid vertex AO. The three probes are POCKET VOLUMES (side-u / side-v /
/// diagonal quadrants of a [`PROBE_REACH`]-sized region around the corner,
/// lifted [`PROBE_LIFT`] off the face plane) overlap-tested against matter —
/// the cell's own box set directly, and any out-of-cell portion through the
/// shared `matter` query (opaque whole-cell, a neighbour's stair/slab
/// occupancy, its box set...). Volumes, not points: an inset neighbour base
/// still overlaps an edge pocket, so received casting is uniform along an
/// edge. A full-cell box face reproduces classic grid vertex AO exactly, and
/// the lift keeps two flush boxes forming one continuous surface from
/// shadowing their shared seam.
fn probe_ao(
    boxes: &[MeshBox],
    corner: [f32; 3],
    (axis, ua, va): (usize, usize, usize),
    positive: bool,
    rect: &Rect,
    wcell: (i32, i32, i32),
    matter: &MatterFn,
) -> u32 {
    let su = if corner[ua] - rect.u0 < rect.u1 - corner[ua] {
        -PROBE_REACH
    } else {
        PROBE_REACH
    };
    let sv = if corner[va] - rect.v0 < rect.v1 - corner[va] {
        -PROBE_REACH
    } else {
        PROBE_REACH
    };

    let pocket = |u_beyond: bool, v_beyond: bool| -> ([f32; 3], [f32; 3]) {
        let mut lo = corner;
        let mut hi = corner;
        if positive {
            lo[axis] += PROBE_LIFT;
            hi[axis] += PROBE_LIFT + PROBE_REACH;
        } else {
            hi[axis] -= PROBE_LIFT;
            lo[axis] -= PROBE_LIFT + PROBE_REACH;
        }
        for (a, sign, beyond) in [(ua, su, u_beyond), (va, sv, v_beyond)] {
            let end = corner[a] + if beyond { sign } else { -sign };
            lo[a] = corner[a].min(end);
            hi[a] = corner[a].max(end);
        }
        (lo, hi)
    };

    let occupied = |(plo, phi): ([f32; 3], [f32; 3])| -> bool {
        // The cell's own boxes: strict positive overlap (a graze is not
        // matter; the lift owns continuation immunity).
        if boxes
            .iter()
            .any(|b| (0..3).all(|a| plo[a] < b.max[a] && phi[a] > b.min[a]))
        {
            return true;
        }
        // Out-of-cell portions: split the pocket per axis at the cell
        // bounds and resolve each non-empty foreign part through `matter`.
        let segs = |a: usize| -> [(i32, f32, f32); 3] {
            [
                (-1, plo[a], phi[a].min(0.0)),
                (0, plo[a].max(0.0), phi[a].min(1.0)),
                (1, plo[a].max(1.0), phi[a]),
            ]
        };
        for (ox, xl, xh) in segs(0) {
            if xh - xl <= T {
                continue;
            }
            for (oy, yl, yh) in segs(1) {
                if yh - yl <= T {
                    continue;
                }
                for (oz, zl, zh) in segs(2) {
                    if zh - zl <= T || (ox, oy, oz) == (0, 0, 0) {
                        continue;
                    }
                    let cl = (wcell.0 + ox, wcell.1 + oy, wcell.2 + oz);
                    let off = [ox as f32, oy as f32, oz as f32];
                    if matter(
                        cl,
                        [xl - off[0], yl - off[1], zl - off[2]],
                        [xh - off[0], yh - off[1], zh - off[2]],
                    ) {
                        return true;
                    }
                }
            }
        }
        false
    };

    quad_ao(
        occupied(pocket(false, false)),
        occupied(pocket(true, false)),
        occupied(pocket(false, true)),
        occupied(pocket(true, true)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(u0: f32, v0: f32, u1: f32, v1: f32) -> Rect {
        Rect { u0, v0, u1, v1 }
    }

    fn area(rects: &[Rect]) -> f32 {
        rects.iter().map(|r| (r.u1 - r.u0) * (r.v1 - r.v0)).sum()
    }

    fn run_subtract(face: Rect, occ: &[Rect]) -> Vec<Rect> {
        let mut cuts = vec![];
        let mut runs = vec![];
        let mut out = vec![];
        subtract(face, occ, &mut cuts, &mut runs, &mut out);
        out
    }

    /// Butted contact faces vanish entirely; a partial cover leaves exactly
    /// the uncovered area, with no overlapping output rects.
    #[test]
    fn subtraction_conserves_uncovered_area() {
        let face = rect(0.0, 0.0, 1.0, 1.0);
        assert!(run_subtract(face, &[face]).is_empty(), "full burial");

        // A centred notch: 4 surrounding rects totalling 1 - 1/4.
        let out = run_subtract(face, &[rect(0.25, 0.25, 0.75, 0.75)]);
        assert!((area(&out) - 0.75).abs() < 1e-5, "area {:?}", out);
        // No two output rects overlap.
        for (i, a) in out.iter().enumerate() {
            for b in out.iter().skip(i + 1) {
                let w = (a.u1.min(b.u1) - a.u0.max(b.u0)).max(0.0);
                let h = (a.v1.min(b.v1) - a.v0.max(b.v0)).max(0.0);
                assert!(w * h < 1e-6, "overlap {a:?} {b:?}");
            }
        }
    }

    /// An uncovered face returns itself; disjoint covers split into bands
    /// that re-merge where extents match.
    #[test]
    fn subtraction_remerges_bands() {
        let face = rect(0.0, 0.0, 1.0, 1.0);
        assert_eq!(run_subtract(face, &[]), vec![face]);

        // A strip across the middle: two rects remain (above and below),
        // NOT four (band split must re-merge horizontally-equal bands).
        let out = run_subtract(face, &[rect(0.0, 0.4, 1.0, 0.6)]);
        assert_eq!(out.len(), 2, "{out:?}");
        assert!((area(&out) - 0.8).abs() < 1e-5);
    }

    fn plain(min: [f32; 3], max: [f32; 3]) -> MeshBox {
        // The tile is irrelevant to these geometry tests; any atlas row works.
        let t = crate::atlas::Tile::named("stone");
        MeshBox::uniform(min, max, [t, t, t], |_| [1.0, 1.0, 1.0])
    }

    /// The occluder rule: butting-in-front hides, butting-behind does not,
    /// straddling (interpenetration) hides, and coincident same-plane faces
    /// of overlapping boxes keep exactly one winner.
    #[test]
    fn occluder_rule_covers_butting_and_coincidence() {
        let axes = (0usize, 2usize, 1usize); // +X face: u=Z, v=Y
        let rect_full = rect(0.0, 0.0, 1.0, 1.0);
        let mut occ = vec![];

        // In front (butted at d): hides.
        push_occluder(&mut occ, [0.5, 0.0, 0.0], [1.0, 1.0, 1.0], axes, true, 0.5, false, &rect_full);
        assert_eq!(occ.len(), 1);

        // Behind (ends at d): does not hide without the coincidence tie.
        occ.clear();
        push_occluder(&mut occ, [0.0, 0.0, 0.0], [0.5, 1.0, 1.0], axes, true, 0.5, false, &rect_full);
        assert!(occ.is_empty());

        // Behind + coincident + earlier: hides (the tie-break winner).
        push_occluder(&mut occ, [0.0, 0.0, 0.0], [0.5, 1.0, 1.0], axes, true, 0.5, true, &rect_full);
        assert_eq!(occ.len(), 1);

        // Straddling (interpenetration): does NOT hide — cutout tiles may
        // show a face through the straddling box (the chain's crossing
        // plates), so only sealed flush contact culls.
        occ.clear();
        push_occluder(&mut occ, [0.25, 0.0, 0.0], [0.75, 1.0, 1.0], axes, true, 0.5, false, &rect_full);
        assert!(occ.is_empty());
    }

    /// For a full-cell box the corner probes leave the cell on every axis,
    /// so self-AO reduces exactly to the grid ring: an empty ring keeps AO
    /// at 3, a diagonal-only occluder gives 2, two sides give the buried 0.
    #[test]
    fn full_cell_probe_reduces_to_grid_vertex_ao() {
        let boxes = [plain([0.0; 3], [1.0; 3])];
        let axes = face_axes(Face::PosY);
        let r = rect(0.0, 0.0, 1.0, 1.0);
        // Corner (0, 1, 0): side cells (-1,1,0) and (0,1,-1), diag (-1,1,-1).
        let corner = [0.0, 1.0, 0.0];
        let at = (0, 0, 0);
        let ao_none = probe_ao(&boxes, corner, axes, true, &r, at, &|_, _, _| false);
        assert_eq!(ao_none, 3);
        let ao_diag = probe_ao(&boxes, corner, axes, true, &r, at, &|cl, _, _| {
            cl == (-1, 1, -1)
        });
        assert_eq!(ao_diag, 2);
        let ao_sides = probe_ao(&boxes, corner, axes, true, &r, at, &|cl, _, _| {
            cl.1 == 1 && (cl.0 == -1) != (cl.2 == -1)
        });
        assert_eq!(ao_sides, 0, "two solid sides bury the corner");
    }

    /// An inner crease darkens: the corner of a floor face that meets a wall
    /// box probes into the wall. And a coplanar continuation does NOT: two
    /// flush boxes forming one surface leave the shared seam at AO 3 (the
    /// lifted probe passes above both).
    #[test]
    fn probes_darken_creases_but_not_continuations() {
        // Floor + wall along the u-min edge (a stair silhouette).
        let stair = [
            plain([0.0, 0.0, 0.0], [1.0, 0.5, 1.0]),
            plain([0.0, 0.5, 0.0], [0.5, 1.0, 1.0]),
        ];
        let axes = face_axes(Face::PosY);
        // The tread: the floor box's +Y face right of the riser.
        let tread = rect(0.5, 0.0, 1.0, 1.0); // u = X in [0.5, 1], v = Z
        let crease = probe_ao(&stair, [0.5, 0.5, 0.5], axes, true, &tread, (0, 0, 0), &|_, _, _| false);
        assert!(crease < 3, "tread corner against the riser must darken");

        // Two half boxes forming one flat top: the shared seam stays open.
        let flat = [
            plain([0.0, 0.0, 0.0], [0.5, 1.0, 1.0]),
            plain([0.5, 0.0, 0.0], [1.0, 1.0, 1.0]),
        ];
        let left_top = rect(0.0, 0.0, 0.5, 1.0);
        let seam = probe_ao(&flat, [0.5, 1.0, 0.5], axes, true, &left_top, (0, 0, 0), &|_, _, _| false);
        assert_eq!(seam, 3, "coplanar continuation must not self-shadow");
    }
}
