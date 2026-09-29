//! The mesher's ONE ambient-occlusion + smooth-light gather: every lit face —
//! cube faces, box-set planes, posed planes, fluid faces — takes its
//! per-corner `(ao, sky light, block light)` from [`face_lighting`].

use glam::IVec3;
use petramond_world::block::CellView;
use petramond_world::block_state::SlabState;
use petramond_world::chunk::SKY_FULL;
use petramond_world::light::{BlockLight6, LightRgb};

use super::super::face::{quad_ao, Face, AO_OPEN};
use super::super::face_emit::{fold_light, fold_light_smooth, slab_corner_open};
use super::cell_class::{PAD_OPAQUE, PAD_SLAB, RING_BOX_SHAPE, RING_OCCLUDES_AO};
use super::cube_face::face_index;
use super::neighbourhood::{
    Neighbourhood, RING_BLOCK_MASK, RING_BLOCK_SHIFT, RING_CLASS_SHIFT, RING_SKY_MASK,
};
use super::pad::SECTION_PAD;

/// Per-corner `(ao, sky6, block light)` of one face, in quad corner order.
pub(crate) type CornerLight = ([u32; 4], [u32; 4], [BlockLight6; 4]);

/// The four sub-cell AO cast probe POCKETS of one face corner — the
/// side-u / side-v / diagonal / interior quadrants of a
/// [`PROBE_REACH`]-sized volume around the corner `(su, sv)` on the face
/// fronted by voxel `f`, lifted [`PROBE_LIFT`] off the face plane into the
/// front region. `plane` is the face plane's coordinate along the face
/// normal, measured from `f`'s minimum corner: the voxel boundary for cube faces
/// ([`boundary_plane`]), but an INTERIOR height for a box family's inner
/// planes (a slab's top at 0.5) — pockets must sit off the actual plane, or
/// they probe matter BELOW it (the slab's own bottom half, the neighbouring
/// slab it forms a continuous floor with) and shadow a face that nothing
/// overhangs. Each pocket is an AABB `(lo, hi)` in `f`'s local frame,
/// overlap-tested against its ring cell's occupancy — the grid-AO
/// generalization: for an opaque ring cell the whole-cell bit answers; for a
/// box-family cell the pocket must overlap its actual matter. Pockets are
/// VOLUMES, not points: an inset base (the cauldron's) still overlaps the
/// edge-adjacent pockets, so casting is uniform along an edge instead of
/// sparking only at diagonal corners. A fence's centred post overlaps no
/// corner pocket and correctly casts nothing.
///
/// [`PROBE_LIFT`]: super::super::boxset::PROBE_LIFT
/// [`PROBE_REACH`]: super::super::boxset::PROBE_REACH
#[inline]
pub(crate) fn corner_cast_probes(
    face: Face,
    su: i32,
    sv: i32,
    plane: f32,
) -> [([f32; 3], [f32; 3]); 4] {
    use super::super::boxset::{PROBE_LIFT, PROBE_REACH};
    let d = face.dir().to_array();
    let u = face.ao_u().to_array();

    let mut corner = [0.0f32; 3];
    for a in 0..3 {
        if d[a] != 0 {
            corner[a] = plane;
        } else if u[a] != 0 {
            corner[a] += (su > 0) as u32 as f32;
        } else {
            corner[a] += (sv > 0) as u32 as f32;
        }
    }
    let pocket = |u_beyond: bool, v_beyond: bool| -> ([f32; 3], [f32; 3]) {
        let mut lo = [0.0f32; 3];
        let mut hi = [0.0f32; 3];
        for a in 0..3 {
            let (l, h) = if d[a] != 0 {
                if d[a] > 0 {
                    (corner[a] + PROBE_LIFT, corner[a] + PROBE_LIFT + PROBE_REACH)
                } else {
                    (corner[a] - PROBE_LIFT - PROBE_REACH, corner[a] - PROBE_LIFT)
                }
            } else {
                let (sign, beyond) = if u[a] != 0 {
                    (su, u_beyond)
                } else {
                    (sv, v_beyond)
                };
                let dir = if beyond { sign } else { -sign } as f32;
                let end = corner[a] + dir * PROBE_REACH;
                (corner[a].min(end), corner[a].max(end))
            };
            lo[a] = l;
            hi[a] = h;
        }
        (lo, hi)
    };
    [
        pocket(true, false),
        pocket(false, true),
        pocket(true, true),
        // The INTERIOR quadrant — inside the front cell itself. Grid AO
        // assumes it empty (matter in front of a cube face culls the face),
        // but sub-cell matter STANDS on faces it doesn't cull: the exposed
        // ring of the cell under a cauldron must darken toward the base
        // rising from it, or it stays bright beside darkened neighbours (a
        // hard edge at the cell boundary).
        pocket(false, false),
    ]
}

/// A cube face's plane along the face normal, measured from its front voxel's
/// minimum corner: the front voxel's boundary toward the cell it fronts.
#[inline]
pub(crate) fn boundary_plane(face: Face) -> f32 {
    (face.dir().element_sum() < 0) as u32 as f32
}

#[inline]
pub(super) fn pad_stride(d: IVec3) -> isize {
    let pad = SECTION_PAD as isize;
    d.x as isize + d.z as isize * pad + d.y as isize * pad * pad
}

/// The smooth-light + AO value of one face corner, shared between the faces that meet at that
/// vertex. Sharing is exact only for a SIMPLE square — the four front-layer cells around the
/// vertex hold no box-shape family and no slab, and the asking face's front cell neither
/// occludes AO nor is opaque. For such a square the corner value is symmetric in which of its
/// cells is the front: the AO cast is `3 - occluders` over the three non-front cells, whose
/// `(side && side) -> 0` exception can only fire on the two cells edge-adjacent to a non-
/// occluding front, and the smooth mean runs over every non-opaque cell of the square, the
/// front being one of them. Every other corner (probes, slabs, a leaf or opaque front) is
/// computed from its own three ring cells exactly as before and never stored.
///
/// One entry per (face direction, vertex of the pad's 18x19x19 vertex lattice), stamped with
/// the build generation so a build costs no clear.
#[derive(Default)]
pub(super) struct VertexLightCache {
    gen: u32,
    entries: Vec<(u32, u32)>,
    #[cfg(test)]
    disabled: bool,
}

/// Vertices per face direction: 18 front layers along the normal, 19 x 19 vertices across.
const VERTEX_LAYER: usize = 19 * 19;
const VERTEX_GRID: usize = SECTION_PAD * VERTEX_LAYER;

impl VertexLightCache {
    pub(super) fn begin(&mut self) {
        self.gen = self.gen.wrapping_add(1);
        if self.gen == 0 || self.entries.len() != 6 * VERTEX_GRID {
            self.entries.clear();
            self.entries.resize(6 * VERTEX_GRID, (0, 0));
            self.gen = 1;
        }
    }

    /// Turns sharing off for the rest of the build (the equivalence test's control).
    #[cfg(test)]
    pub(super) fn set_disabled(&mut self, disabled: bool) {
        self.disabled = disabled;
    }

    /// Vertices the last build stored.
    #[cfg(test)]
    pub(super) fn stored(&self) -> usize {
        self.entries
            .iter()
            .filter(|(gen, _)| *gen == self.gen)
            .count()
    }

    #[inline]
    fn enabled(&self) -> bool {
        #[cfg(test)]
        {
            !self.disabled
        }
        #[cfg(not(test))]
        {
            true
        }
    }

    #[inline]
    fn get(&self, key: usize) -> Option<(u32, u32, BlockLight6)> {
        let (gen, packed) = self.entries[key];
        (gen == self.gen).then(|| {
            (
                packed & 3,
                (packed >> 2) & 63,
                BlockLight6::from_bits(packed >> 8),
            )
        })
    }

    #[inline]
    fn set(&mut self, key: usize, ao: u32, sky6: u32, block: BlockLight6) {
        self.entries[key] = (self.gen, ao | (sky6 << 2) | (block.bits() << 8));
    }
}

const SIMPLE_CELL: u8 = RING_BOX_SHAPE | PAD_SLAB;
const SIMPLE_FRONT: u8 = SIMPLE_CELL | RING_OCCLUDES_AO | PAD_OPAQUE;

/// One ring cell as the general corner path reads it: its AO/light roles resolved through the
/// slab state when the cell is a partial slab.
struct RingCell {
    occ: bool,
    probe: bool,
    opq: bool,
    sky: u32,
    blk: LightRgb,
    slab: SlabState,
}

#[inline]
fn read_ring_cell(nb: &Neighbourhood<'_>, i: usize, smooth_light: bool) -> RingCell {
    let pad = nb.pad();
    let word = nb.ring()[i];
    let cls = (word >> RING_CLASS_SHIFT) as u8;
    let slab_state = (cls & PAD_SLAB != 0).then(|| {
        let stored = SlabState::from_cell(pad.cell_states[i]);
        petramond_world::slab::normalize_state(pad.table.block(pad.blocks[i]), stored)
    });
    let full_stack = slab_state.is_some_and(|s| s.is_full());
    let occ = cls & RING_OCCLUDES_AO != 0 || full_stack;
    let mut cell = RingCell {
        occ,
        probe: !occ && cls & RING_BOX_SHAPE != 0,
        opq: false,
        sky: 0,
        blk: LightRgb::ZERO,
        slab: SlabState::EMPTY,
    };
    if smooth_light {
        cell.opq = cls & PAD_OPAQUE != 0 || full_stack;
        if !cell.opq {
            cell.sky = word & RING_SKY_MASK;
            cell.blk = LightRgb::from_bits(((word >> RING_BLOCK_SHIFT) & RING_BLOCK_MASK) as u16);
            if let Some(state) = slab_state {
                cell.slab = state;
            }
        }
    }
    cell
}

/// One face's per-corner AO + smooth light (skylight + coloured block light).
/// Each corner reads its three ring cells around the front voxel `front`
/// (the edge cell along u, along v, and the diagonal) from the packed ring
/// words. `occ` = AO occluders (opaque cubes AND leaves, for canopy
/// self-occlusion); `opq` = full-opaque, which carry no light and so are
/// excluded from the smooth-light mean (leaves differ between the two, hence
/// both bits). A corner whose square is simple (see [`VertexLightCache`]) is
/// looked up in the build's vertex cache first and stored there after, so the
/// faces meeting at a vertex fold it once.
///
/// `plane` is the face plane along the normal, measured from the front
/// voxel's minimum corner — [`boundary_plane`] for cube faces, the actual
/// plane height for a box family's interior planes (see
/// `corner_cast_probes`). `smooth_light = false` lights every corner flat
/// from the front voxel (the closed-underside rule of box sets' `NegY`
/// planes). Ring cells that are box-shape families or partial slabs are
/// resolved through the sub-cell cast probes ([`Neighbourhood::matter`]), so
/// pure cube/air neighbourhoods pay nothing for them.
///
/// Split from the vertex push so the greedy mesher can test a face for
/// flatness (all four corners equal — the merge condition) before deciding to
/// emit it per-cell or merge it.
///
/// `front` must be an in-section cell or one of its face neighbours, so the
/// ring stays inside the pad: the pad is a flat array and the face's tangent
/// axes are fixed, so each ring cell is the front's index plus a constant
/// stride — the axis that can sit on the pad's outer plane is the face
/// NORMAL, and the ring only steps along the two tangents.
pub(super) fn face_lighting(
    nb: &Neighbourhood<'_>,
    face: Face,
    front: IVec3,
    plane: f32,
    smooth_light: bool,
) -> CornerLight {
    let ring = nb.ring();
    let q = front - nb.origin() + IVec3::ONE;
    let fi = nb
        .pad_index(front)
        .expect("a lit face's front voxel lies inside the mesh pad");
    let (u, v) = (face.ao_u(), face.ao_v());
    let (ustride, vstride) = (pad_stride(u), pad_stride(v));
    let f_word = ring[fi];
    let f_cls = (f_word >> RING_CLASS_SHIFT) as u8;
    let f_l = f_word & RING_SKY_MASK;
    let f_bl = LightRgb::from_bits(((f_word >> RING_BLOCK_SHIFT) & RING_BLOCK_MASK) as u16);
    let f_ch = [
        u32::from(f_bl.r()),
        u32::from(f_bl.g()),
        u32::from(f_bl.b()),
    ];
    let flat = fold_light(f_l, f_ch, SKY_FULL as u32);

    // The ring-cell half along the normal that lies on the plane's FRONT
    // side — what a partial slab's single light value must describe to feed
    // a corner (see `slab_corner_open`). Boundary planes give the fixed halves
    // (front-half 0 for positive faces, 1 for negative); an interior plane at
    // 0.5 flips them, so a neighbouring bottom slab's open-top light DOES feed
    // the slab-top plane beside it.
    let front_half = if face.dir().element_sum() > 0 {
        (plane >= 0.25) as usize
    } else {
        (plane > 0.75) as usize
    };
    let front_probe = f_cls & RING_BOX_SHAPE != 0;

    let mut cache = nb.vertex_light().borrow_mut();
    let share = smooth_light && f_cls & SIMPLE_FRONT == 0 && cache.enabled();
    let (qu, qv, qn) = (q.dot(u), q.dot(v), q.dot(face.dir().abs()));
    let vertex_base = face_index(face) * VERTEX_GRID + qn as usize * VERTEX_LAYER;

    let signs = face.ao_signs();
    let mut ao = [3u32; 4];
    let mut light6 = [0u32; 4];
    let mut block6 = [BlockLight6::DARK; 4];
    for corner in 0..4 {
        let (su, sv) = signs[corner];
        let key = vertex_base
            + (qv + 1 - (sv < 0) as i32) as usize * 19
            + (qu + 1 - (su < 0) as i32) as usize;
        if share {
            if let Some((a, l, b)) = cache.get(key) {
                (ao[corner], light6[corner], block6[corner]) = (a, l, b);
                continue;
            }
        }
        let i1 = (fi as isize + su as isize * ustride) as usize;
        let i2 = (fi as isize + sv as isize * vstride) as usize;
        let i3 = (i1 as isize + sv as isize * vstride) as usize;
        let (w1, w2, w3) = (ring[i1], ring[i2], ring[i3]);
        let cls = [
            (w1 >> RING_CLASS_SHIFT) as u8,
            (w2 >> RING_CLASS_SHIFT) as u8,
            (w3 >> RING_CLASS_SHIFT) as u8,
        ];
        if !front_probe && (cls[0] | cls[1] | cls[2]) & SIMPLE_CELL == 0 {
            let occ = |c: u8| c & RING_OCCLUDES_AO != 0;
            ao[corner] = quad_ao(false, occ(cls[0]), occ(cls[1]), occ(cls[2]));
            if !smooth_light {
                (light6[corner], block6[corner]) = flat;
                continue;
            }
            let mut sum = f_l;
            let mut sum_block = f_ch;
            let mut cnt = 1u32;
            for (c, w) in [(cls[0], w1), (cls[1], w2), (cls[2], w3)] {
                if c & PAD_OPAQUE != 0 {
                    continue;
                }
                sum += w & RING_SKY_MASK;
                let bl = LightRgb::from_bits(((w >> RING_BLOCK_SHIFT) & RING_BLOCK_MASK) as u16);
                sum_block[0] += bl.r() as u32;
                sum_block[1] += bl.g() as u32;
                sum_block[2] += bl.b() as u32;
                cnt += 1;
            }
            (light6[corner], block6[corner]) = fold_light_smooth(sum, sum_block, cnt);
            if share {
                cache.set(key, ao[corner], light6[corner], block6[corner]);
            }
            continue;
        }

        // The general corner: sub-cell probes for box-shape cells, half-cell
        // openness for partial slabs.
        let cells = [
            read_ring_cell(nb, i1, smooth_light),
            read_ring_cell(nb, i2, smooth_light),
            read_ring_cell(nb, i3, smooth_light),
        ];
        let (mut s1, mut s2, mut c) = (cells[0].occ, cells[1].occ, cells[2].occ);
        let mut q_int = false;
        if front_probe
            || (cells[0].probe && !s1)
            || (cells[1].probe && !s2)
            || (cells[2].probe && !c)
        {
            let pk = corner_cast_probes(face, su, sv, plane);
            let cell_of = |s_u: i32, s_v: i32| front + u * s_u + v * s_v;
            let local = |p: [f32; 3], cl: IVec3| {
                let o = (cl - front).as_vec3();
                [p[0] - o.x, p[1] - o.y, p[2] - o.z]
            };
            let probe = |cl: IVec3, (lo, hi): ([f32; 3], [f32; 3])| {
                nb.matter(cl, local(lo, cl), local(hi, cl))
            };
            if cells[0].probe && !s1 {
                s1 = probe(cell_of(su, 0), pk[0]);
            }
            if cells[1].probe && !s2 {
                s2 = probe(cell_of(0, sv), pk[1]);
            }
            if cells[2].probe && !c {
                c = probe(cell_of(su, sv), pk[2]);
            }
            if front_probe {
                q_int = probe(front, pk[3]);
            }
        }
        ao[corner] = quad_ao(q_int, s1, s2, c);
        if !smooth_light {
            (light6[corner], block6[corner]) = flat;
            continue;
        }
        let mut sum = f_l;
        let mut sum_block = f_ch;
        let mut cnt = 1u32;
        for (cell, a, b) in [(&cells[0], su, 0), (&cells[1], 0, sv), (&cells[2], su, sv)] {
            if cell.opq || !slab_corner_open(cell.slab, face, a, b, su, sv, front_half) {
                continue;
            }
            sum += cell.sky;
            sum_block[0] += cell.blk.r() as u32;
            sum_block[1] += cell.blk.g() as u32;
            sum_block[2] += cell.blk.b() as u32;
            cnt += 1;
        }
        (light6[corner], block6[corner]) = fold_light_smooth(sum, sum_block, cnt);
    }
    (ao, light6, block6)
}

pub(super) fn cell_light(nb: &Neighbourhood<'_>, p: IVec3) -> (u32, BlockLight6) {
    let l = u32::from(nb.skylight(p));
    let c = nb.blocklight(p);
    fold_light(
        l,
        [u32::from(c.r()), u32::from(c.g()), u32::from(c.b())],
        SKY_FULL as u32,
    )
}

/// Fold a fluid's own emission into a face it provides `fraction` (`0..=1`) of
/// its light for: each corner's block channel moves from the sampled value
/// toward `max(sampled, emission)` and its AO toward open by that fraction, so
/// `0` is the sampled face and `1` a face lit wholly by itself. The sky channel
/// is left as sampled.
///
/// The cube path lights a face from the cell IN FRONT of it, while every
/// other emitter path (plant, torch, model, box plane) reads the block's own
/// cell — the one the light flood seeds with the emission. A glowing fluid's
/// recessed surface under a lid fronts the lid, an opaque cell the flood never
/// enters, so it drew black under a cave ceiling; and grid AO read the rock
/// around a trench as shadow on the very surface that is the light source.
#[inline]
pub(crate) fn self_lit_face(
    emission: BlockLight6,
    fraction: f32,
    ao: &mut [u32; 4],
    block6: &mut [BlockLight6; 4],
) {
    let lift =
        |from: u32, to: u32| from + (to.saturating_sub(from) as f32 * fraction).round() as u32;
    for a in ao.iter_mut() {
        *a = lift(*a, AO_OPEN);
    }
    let [er, eg, eb] = emission.channels();
    for c in block6.iter_mut() {
        let [r, g, b] = c.channels();
        *c = BlockLight6::new(lift(r, er), lift(g, eg), lift(b, eb));
    }
}

#[cfg(test)]
mod self_lit_tests {
    use super::*;

    #[test]
    fn self_lit_fraction_scales_the_lift_toward_the_emission() {
        let emission = BlockLight6::new(60, 30, 12);
        let sampled = [BlockLight6::new(4, 40, 0); 4];
        let lit = |fraction: f32| {
            let (mut ao, mut block6) = ([0; 4], sampled);
            self_lit_face(emission, fraction, &mut ao, &mut block6);
            (ao, block6)
        };
        assert_eq!(lit(0.0), ([0; 4], sampled));
        assert_eq!(lit(1.0), ([AO_OPEN; 4], [BlockLight6::new(60, 40, 12); 4]));
        let (ao, block6) = lit(0.5);
        assert!(ao.iter().all(|&a| 0 < a && a < AO_OPEN));
        let [r, g, b] = block6[0].channels();
        assert!(
            4 < r && r < 60 && g == 40 && 0 < b && b < 12,
            "{:?}",
            block6[0]
        );
    }
}
