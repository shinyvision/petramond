//! The ONE box-set emitter every axis-aligned block shape meshes through.
//!
//! Every block shape in the game whose geometry is a set of axis-aligned
//! cuboids — the full cube is the degenerate one-box case, stairs and slabs
//! are half-cell box sets, fences/panes/ladders are thin box sets, custom
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
//!   neighbour. The same subtraction handles fence post caps, pane segment
//!   caps, and stair/slab half-cell adjacency.
//! - **Lighting is the cube's, per plane.** The emitter gathers the
//!   mesher's face lighting (`face_light`) once per (face direction, boundary/interior
//!   plane) — the front voxel is the neighbour for a flush face, the cell
//!   itself for an interior one, exactly the stair/slab convention — and
//!   bilinearly samples it at every emitted corner, so a box face shades
//!   identically to a full cube face wherever their corners coincide.
//!   `NegY` planes stay flat-lit (the stair's closed-underside rule). Every
//!   box family is smooth-lit because plane-field sampling keeps lighting
//!   continuous across thin geometry.
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
//!   (the face-lighting gather's corner cast probes), which is what makes a stair
//!   or cauldron CAST onto the terrain beside it.
//!
//! Emitted quads carry cell-local UVs (carved from the tile like a stair),
//! directional shade, and the standard packed vertex layout. Coplanar quads
//! from one subtraction share whole band edges; quads of one plane sample
//! one shared light field, so seams are invisible.

use crate::vertex::BlockLightVertexExt;
use glam::IVec3;
use petramond_world::block::Block;

use super::builder::{boundary_plane, face_axes, CornerLight};
use super::face::{quad_ao, should_flip, Face, FaceShading, FACES};
use super::plane::{cell_uv, FaceUvSpan, PlaneLight};
use super::vertex::{pack_cell_uv, pack_normal_code, pack_vertex, Vertex, UV_MODE_CELL_LOCAL};
use super::UV_MODE_SHIFT;

pub(super) use petramond_world::block::{ShapeBox, ShapeFace};

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

const T: f32 = 1e-4;
const AREA_EPS: f32 = 1e-4;
pub(super) const PROBE_LIFT: f32 = 0.02;
pub(super) const PROBE_REACH: f32 = 1.5 / 16.0;

pub(super) type MatterFn<'a> = dyn Fn(IVec3, [f32; 3], [f32; 3]) -> bool + 'a;

pub(super) trait BoxWorld {
    fn neighbour_solid(&self, face: Face) -> bool;

    fn neighbour_boxes(&self, face: Face, out: &mut Vec<([f32; 3], [f32; 3])>);

    fn matter(&self, cell: IVec3, lo: [f32; 3], hi: [f32; 3]) -> bool;

    fn face_light(&self, face: Face, front: IVec3, plane: f32, smooth: bool) -> CornerLight;
}

#[derive(Copy, Clone)]
pub(super) struct BoxCell {
    pub(super) cell: IVec3,
    pub(super) anchor: IVec3,
}

impl BoxCell {
    #[inline]
    fn mesh_pos(self, local: [f32; 3]) -> [f32; 3] {
        let base = (self.cell - self.anchor).as_vec3();
        [base.x + local[0], base.y + local[1], base.z + local[2]]
    }
}

#[derive(Copy, Clone)]
struct FacePlane {
    axes: (usize, usize, usize),
    positive: bool,
    d: f32,
}

#[derive(Default)]
pub(super) struct BoxSetScratch {
    occ: Vec<Rect>,
    cuts: Vec<f32>,
    runs: Vec<(f32, f32)>,
    rects: Vec<Rect>,
    nb: Vec<([f32; 3], [f32; 3])>,
    planes: Vec<(f32, PlaneLight)>,
}

pub(super) fn emit_box_set(
    vbuf: &mut Vec<Vertex>,
    at: BoxCell,
    boxes: &[ShapeBox],
    scratch: &mut BoxSetScratch,
    world: &dyn BoxWorld,
) {
    let matter = |cell: IVec3, lo: [f32; 3], hi: [f32; 3]| world.matter(cell, lo, hi);
    for face in FACES {
        let fi = face as usize;
        let (axis, ua, va) = face_axes(face);
        let positive = matches!(face, Face::PosX | Face::PosY | Face::PosZ);

        let mut solid: Option<bool> = None;
        let mut nb_fetched = false;
        scratch.planes.clear();

        for (i, b) in boxes.iter().enumerate() {
            let Some(style) = b.faces[fi] else { continue };
            if let Some(pose) = b.pose {
                emit_posed_face(vbuf, at, b, &pose, face, &style, world);
                continue;
            }
            let d = if positive {
                b.aabb.max[axis]
            } else {
                b.aabb.min[axis]
            };
            let flush = if positive { d >= 1.0 - T } else { d <= T };

            if flush {
                let s = match solid {
                    Some(s) => s,
                    None => *solid.insert(world.neighbour_solid(face)),
                };
                if s {
                    continue;
                }
                if !nb_fetched {
                    nb_fetched = true;
                    scratch.nb.clear();
                    world.neighbour_boxes(face, &mut scratch.nb);
                }
            }

            let rect = Rect {
                u0: b.aabb.min[ua],
                v0: b.aabb.min[va],
                u1: b.aabb.max[ua],
                v1: b.aabb.max[va],
            };

            let face_plane = FacePlane {
                axes: (axis, ua, va),
                positive,
                d,
            };
            scratch.occ.clear();
            for (j, o) in boxes.iter().enumerate() {
                if j != i && o.pose.is_none() {
                    push_occluder(
                        &mut scratch.occ,
                        (o.aabb.min, o.aabb.max),
                        face_plane,
                        // The coincidence tie-break only settles WHICH of two
                        // boxes draws a shared plane. A box that never emits
                        // this face has no claim on it and must not suppress
                        // the box that does — otherwise a cap plate flush with
                        // the body it caps loses its only face.
                        j < i && o.faces[fi].is_some(),
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
                    push_occluder(&mut scratch.occ, (smin, smax), face_plane, false, &rect);
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

            if !scratch.planes.iter().any(|(pd, _)| (pd - d).abs() <= T) {
                let front = if flush { at.cell + face.dir() } else { at.cell };
                // The gather's probe pockets sit ON the face plane, measured
                // from the front cell — the voxel boundary when flush, the
                // box's own plane height when interior (a slab top's pockets
                // at 0.5, not the cell floor).
                let plane = if flush { boundary_plane(face) } else { d };
                let (ao, sky, block) = world.face_light(
                    face,
                    front,
                    plane,
                    // The closed-underside rule (stairs, slabs): a NegY
                    // plane must not smooth sky from cells beside a dark
                    // cell below.
                    face != Face::NegY,
                );
                scratch.planes.push((d, PlaneLight { ao, sky, block }));
            }
            let pl = &scratch
                .planes
                .iter()
                .find(|(pd, _)| (pd - d).abs() <= T)
                .expect("just filled")
                .1;
            let span = FaceUvSpan::of(face, b.aabb.min, b.aabb.max);

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

                let mut quad_ao = [3u32; 4];
                let mut sky = [0u32; 4];
                let mut light = [petramond_world::light::BlockLight6::DARK; 4];
                let mut uvs = [(0u32, 0u32); 4];
                for (ci, lp) in local.into_iter().enumerate() {
                    let [u, v] = cell_uv(face, lp);
                    let (mut ao, sky6, block) = pl.sample(u, v);
                    ao = ao.min(probe_ao(boxes, lp, face_plane, &r, at.cell, &matter));
                    if b.ao_strength < 1.0 {
                        // Scale the DARKENING, not the value: 3 stays 3, and
                        // strength 0 lifts every corner to full brightness.
                        let dark = (3 - ao) as f32 * b.ao_strength.max(0.0);
                        ao = 3 - (dark.round() as u32).min(3);
                    }
                    quad_ao[ci] = ao;
                    sky[ci] = sky6;
                    light[ci] = block;
                    let (uu, vv) = style.texel_uv((u, v), span.fraction((u, v)));
                    uvs[ci] = (quant_uv(uu), quant_uv(vv));
                }
                let start = vbuf.len() as u32;
                let rot = usize::from(should_flip(quad_ao));
                for k in 0..4usize {
                    let ci = (k + rot) & 3;
                    vbuf.push(Vertex {
                        pos: at.mesh_pos(local[ci]),
                        tint: light[ci].tint_word(style.tint),
                        packed: pack_vertex(
                            style.tile.index() as u32,
                            ci as u32,
                            face.shade_idx(),
                            false,
                            quad_ao[ci],
                            sky[ci],
                        ) | light[ci].packed_bits()
                            | (UV_MODE_CELL_LOCAL << UV_MODE_SHIFT),
                        packed2: light[ci].packed2_bits()
                            | pack_cell_uv(uvs[ci].0, uvs[ci].1)
                            | pack_normal_code(face.normal_code())
                            | if b.dyed { super::vertex::DYED_FLAG2 } else { 0 },
                    });
                }
                if b.double_sided {
                    super::vertex::push_back_face(vbuf, start);
                }
            }
        }
    }
}

#[inline]
fn quant_uv(x: f32) -> u32 {
    super::vertex::round_i32(x * 16.0).clamp(0, 16) as u32
}

/// One face of a POSED box: its four authored corners carried through the
/// pose, drawn whole. No subtraction (it lies on no axis plane, so nothing
/// can be flush against it), and its art is carved in the box's OWN frame so
/// it turns with the box. A flat plane's two faces are coplanar with opposite
/// windings, so back-face culling shows exactly one from any side.
///
/// Lit as an interior plane of the cell in the direction its posed normal
/// leans most: the plane light is gathered at the face centre's height along
/// that axis and sampled bilinearly at each corner's projection, so a tilted
/// plate shades like the axis face it most resembles and blends toward the
/// cell's light at its edges. The sub-cell corner probes are axis-aligned by
/// construction and are skipped; the plane's own ring AO still applies.
fn emit_posed_face(
    vbuf: &mut Vec<Vertex>,
    at: BoxCell,
    b: &ShapeBox,
    pose: &petramond_world::block::BoxPose,
    face: Face,
    style: &ShapeFace,
    world: &dyn BoxWorld,
) {
    let (_, ua, va) = face_axes(face);
    if b.aabb.max[ua] - b.aabb.min[ua] <= 0.0 || b.aabb.max[va] - b.aabb.min[va] <= 0.0 {
        return;
    }
    let local = face.quad_box(b.aabb.min, b.aabb.max);
    let posed: [[f32; 3]; 4] = local.map(|p| pose.apply(glam::Vec3::from(p)).to_array());
    let lit = dominant_face(pose.rotate(face.dir().as_vec3()));
    let (laxis, _, _) = face_axes(lit);
    let centre = posed
        .iter()
        .fold(0.0f32, |acc, p| acc + p[laxis] * 0.25)
        .clamp(0.0, 1.0);
    let (ao, sky6, block6) = world.face_light(lit, at.cell, centre, lit != Face::NegY);
    let pl = PlaneLight {
        ao,
        sky: sky6,
        block: block6,
    };

    let mut quad_ao = [3u32; 4];
    let mut sky = [0u32; 4];
    let mut light = [petramond_world::light::BlockLight6::DARK; 4];
    let mut uvs = [(0u32, 0u32); 4];
    let span = FaceUvSpan::of(face, b.aabb.min, b.aabb.max);
    for ci in 0..4 {
        let proj = posed[ci].map(|c| c.clamp(0.0, 1.0));
        let [pu, pv] = cell_uv(lit, proj);
        let (mut ao, sky6, block) = pl.sample(pu, pv);
        if b.ao_strength < 1.0 {
            let dark = (3 - ao) as f32 * b.ao_strength.max(0.0);
            ao = 3 - (dark.round() as u32).min(3);
        }
        quad_ao[ci] = ao;
        sky[ci] = sky6;
        light[ci] = block;
        let [u, v] = cell_uv(face, local[ci]);
        let (uu, vv) = style.texel_uv((u, v), span.fraction((u, v)));
        uvs[ci] = (quant_uv(uu), quant_uv(vv));
    }
    let start = vbuf.len() as u32;
    let rot = usize::from(should_flip(quad_ao));
    for k in 0..4usize {
        let ci = (k + rot) & 3;
        vbuf.push(Vertex {
            pos: at.mesh_pos(posed[ci]),
            tint: light[ci].tint_word(style.tint),
            packed: pack_vertex(
                style.tile.index() as u32,
                ci as u32,
                lit.shade_idx(),
                false,
                quad_ao[ci],
                sky[ci],
            ) | light[ci].packed_bits()
                | (UV_MODE_CELL_LOCAL << UV_MODE_SHIFT),
            packed2: light[ci].packed2_bits()
                | pack_cell_uv(uvs[ci].0, uvs[ci].1)
                | pack_normal_code(lit.normal_code())
                | if b.dyed { super::vertex::DYED_FLAG2 } else { 0 },
        });
    }
    if b.double_sided {
        super::vertex::push_back_face(vbuf, start);
    }
}

fn dominant_face(n: glam::Vec3) -> Face {
    let a = n.abs();
    if a.y >= a.x && a.y >= a.z {
        if n.y >= 0.0 {
            Face::PosY
        } else {
            Face::NegY
        }
    } else if a.x >= a.z {
        if n.x >= 0.0 {
            Face::PosX
        } else {
            Face::NegX
        }
    } else if n.z >= 0.0 {
        Face::PosZ
    } else {
        Face::NegZ
    }
}

/// Whether the cell at `pos` SEALS the whole 1×1 cell boundary its `face`
/// lies on — i.e. its own geometry completely covers that boundary rectangle
/// with view-blocking material.
///
/// This is the CUBE path's counterpart of the emitter's flush subtraction:
/// both ask the ONE box producer, so a cube face is culled exactly where a
/// neighbour's resolved geometry would have covered it, with no family named
/// anywhere. A see-through neighbour never seals (its texels cannot hide
/// another block's face — the glass convention [`occupancy_boxes`] uses), and
/// a family with no box form answers `false`, which is overdraw and never a
/// hole.
pub(crate) fn cell_seals_face(
    nb: &dyn petramond_world::block::ShapeNeighborhood,
    pos: glam::IVec3,
    face: Face,
    boxes: &mut Vec<ShapeBox>,
    scratch: &mut BoxSetScratch,
) -> bool {
    let block = nb.block(pos);
    boxes.clear();
    let tint_for = |_: petramond_world::tile::Tile| [1.0f32; 3];
    snow_bed_boxes(nb, pos, block, &tint_for, boxes);
    if block.has_box_shape() && !block.is_transparent() && !block.is_translucent() {
        let k = block.shape_kind_def();
        k.render.boxes(
            &petramond_world::block::ShapeCtx {
                nb,
                pos,
                block,
                params: &k.params,
                tint_for: &tint_for,
                part_tint: petramond_world::block::NO_PART_TINT,
            },
            boxes,
        );
    }
    !boxes.is_empty() && covers_boundary(boxes, face, scratch)
}

/// The `snow_cover` block bedding this cell, if any: a [`snow_bedded`] row with
/// a snow-cover block on one of its four horizontal neighbours.
///
/// Worldgen places ONE thing per column, so ground decoration and a snow
/// blanket compete for the same cell and the decoration always wins it. That
/// left every tuft, fern and pebble punching a bare hole in a snowfield. The
/// blanket is given back at MESH time from the neighbours, never stored per
/// cell, exactly like the snowy-grass side swap — so it appears and vanishes
/// the moment the snow beside it is placed or dug.
///
/// [`snow_bedded`]: petramond_world::block::BlockTag::SNOW_BEDDED
fn snow_bed(
    nb: &dyn petramond_world::block::ShapeNeighborhood,
    pos: glam::IVec3,
    block: Block,
) -> Option<Block> {
    if !block.is_snow_bedded() {
        return None;
    }
    petramond_world::block::snow_cover_at(pos, |p| nb.block(p))
}

pub(crate) fn cell_wears_snow(
    nb: &dyn petramond_world::block::ShapeNeighborhood,
    pos: glam::IVec3,
) -> bool {
    petramond_world::block::snow_cover_at(pos, |p| nb.block(p)).is_some()
}

pub(crate) fn snow_bed_boxes(
    nb: &dyn petramond_world::block::ShapeNeighborhood,
    pos: glam::IVec3,
    block: Block,
    tint_for: &dyn Fn(petramond_world::tile::Tile) -> [f32; 3],
    out: &mut Vec<ShapeBox>,
) {
    let Some(bed) = snow_bed(nb, pos, block) else {
        return;
    };
    let k = bed.shape_kind_def();
    k.render.boxes(
        &petramond_world::block::ShapeCtx {
            nb,
            pos,
            block: bed,
            params: &k.params,
            tint_for,
            part_tint: petramond_world::block::NO_PART_TINT,
        },
        out,
    );
}

fn covers_boundary(boxes: &[ShapeBox], face: Face, scratch: &mut BoxSetScratch) -> bool {
    let (axis, ua, va) = face_axes(face);
    let positive = matches!(face, Face::PosX | Face::PosY | Face::PosZ);
    scratch.occ.clear();
    for b in boxes.iter().filter(|b| b.occludes && b.pose.is_none()) {
        let d = if positive {
            b.aabb.max[axis]
        } else {
            b.aabb.min[axis]
        };
        if if positive { d < 1.0 - T } else { d > T } {
            continue;
        }
        scratch.occ.push(Rect {
            u0: b.aabb.min[ua],
            v0: b.aabb.min[va],
            u1: b.aabb.max[ua],
            v1: b.aabb.max[va],
        });
    }
    if scratch.occ.is_empty() {
        return false;
    }
    let unit = Rect {
        u0: 0.0,
        v0: 0.0,
        u1: 1.0,
        v1: 1.0,
    };
    subtract(
        unit,
        &scratch.occ,
        &mut scratch.cuts,
        &mut scratch.runs,
        &mut scratch.rects,
    );
    scratch.rects.is_empty()
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
/// A box merely STRADDLING the plane (interpenetration — the cactus's inset
/// trunk behind its full-cell face carriers) deliberately does NOT hide: box
/// faces draw alpha-cutout tiles, so a face inside another box's volume can
/// still show through that box's transparent texels — culling it would punch
/// visible holes. Where the straddling box is fully opaque the retained face
/// is overdraw — invisible if the surface in front is a texel or more away,
/// but a Z-FIGHT once the two are a fraction of a texel apart and the depth
/// buffer stops separating them, which only shows at distance. A pack shape
/// should butt adjacent boxes to let the emitter cull their shared faces.
fn push_occluder(
    occ: &mut Vec<Rect>,
    (omin, omax): ([f32; 3], [f32; 3]),
    plane: FacePlane,
    earlier: bool,
    rect: &Rect,
) {
    let FacePlane {
        axes: (axis, ua, va),
        positive,
        d,
    } = plane;
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
        runs.clear();
        for o in occ {
            if o.v0 <= va + T && o.v1 >= vb - T {
                runs.push((o.u0, o.u1));
            }
        }
        runs.sort_by(|a, b| a.0.partial_cmp(&b.0).expect("finite runs"));
        let mut cursor = rect.u0;
        let push_run = |u0: f32, u1: f32, out: &mut Vec<Rect>| {
            let r = Rect {
                u0,
                v0: va,
                u1,
                v1: vb,
            };
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
                j = i + 1;
            } else {
                j += 1;
            }
        }
        i += 1;
    }
}

/// Corner-probe self-AO + received casting: sub-cell version of grid vertex AO. Three probes are
/// pocket volumes (side-u / side-v / diagonal quadrants, [`PROBE_REACH`] sized, lifted
/// [`PROBE_LIFT`] off the face plane), overlap-tested against matter: cell's own box set directly,
/// plus out-of-cell part via `matter` (opaque whole-cell, neighbour stair/slab occupancy, box
/// set...).
/// Volumes not points: an inset neighbour base still overlaps an edge pocket, so received casting
/// stays uniform along an edge. Full-cell box face reproduces classic grid vertex AO exactly. The
/// lift stops two flush boxes from shadowing their shared seam.
fn probe_ao(
    boxes: &[ShapeBox],
    corner: [f32; 3],
    plane: FacePlane,
    rect: &Rect,
    wcell: IVec3,
    matter: &MatterFn,
) -> u32 {
    let FacePlane {
        axes: (axis, ua, va),
        positive,
        ..
    } = plane;
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
        if boxes
            .iter()
            .filter(|b| b.occludes && b.casts_ao)
            .any(|b| b.overlaps_pocket(plo, phi))
        {
            return true;
        }
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
                    let cl = wcell + IVec3::new(ox, oy, oz);
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
mod tests;
