//! The mesher's ONE ambient-occlusion + smooth-light gather: every lit face —
//! cube faces, box-set planes, posed planes, fluid faces — takes its
//! per-corner `(ao, sky light, block light)` from [`face_lighting`].

use glam::IVec3;
use petramond_world::block::{Block, CellView};
use petramond_world::block_state::SlabState;
use petramond_world::chunk::SKY_FULL;
use petramond_world::light::{BlockLight6, LightRgb};

use super::super::face::{quad_ao, Face, AO_OPEN};
use super::super::face_emit::{fold_light, fold_light_smooth, slab_corner_open};
use super::neighbourhood::Neighbourhood;
use super::pad::SECTION_PAD;

/// Per-corner `(ao, sky6, block light)` of one face, in quad corner order.
pub(crate) type CornerLight = ([u32; 4], [u32; 4], [BlockLight6; 4]);

/// Whether a NON-occluding ring cell still deserves a sub-cell AO cast probe:
/// a box-shaped cell occupies only part of itself, so a corner pocket inside
/// it can be solid even though the whole cell is not. Reads the loader-derived
/// dense flag rather than listing families, so a new box family — engine or
/// mod — casts sub-cell AO the moment it resolves to boxes.
#[inline]
fn probe_worthy(block: Block) -> bool {
    block.has_box_shape()
}

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
    let (dx, dy, dz) = face.dir();
    let d = [dx, dy, dz];
    let (ux, uy, uz) = face.ao_u();
    let u = [ux, uy, uz];

    // The corner's position in the front cell: on the face plane, at the
    // corner the (su, sv) signs pick.
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
    // Per pocket: the normal axis spans (lift, lift + reach) into the front
    // region; a tangent spans REACH beyond the corner when the pocket lies
    // on that side (`beyond`), else REACH back toward the face interior.
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
    let (dx, dy, dz) = face.dir();
    (dx + dy + dz < 0) as u32 as f32
}

/// The flat-array step of one pad cell along `(dx, dy, dz)`.
#[inline]
fn pad_stride(dx: i32, dy: i32, dz: i32) -> isize {
    let pad = SECTION_PAD as isize;
    dx as isize + dz as isize * pad + dy as isize * pad * pad
}

/// One face's per-corner AO + smooth light (skylight + coloured block light),
/// gathered from the shared 3×3 tangent-plane ring around the front voxel
/// `front` ONCE. The four corners share these eight ring cells (each edge cell
/// feeds two corners, each diagonal one), so a single gather replaces
/// per-corner re-reads. `occ` = AO occluders (opaque cubes AND leaves, for
/// canopy self-occlusion); `opq` = full-opaque, which carry no light and so
/// are excluded from the smooth-light mean (leaves differ between the two,
/// hence both bits). The centre cell is the front voxel itself and is never
/// sampled.
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
    let pad = nb.pad();
    let fi = nb
        .pad_index(front)
        .expect("a lit face's front voxel lies inside the mesh pad");
    let (ux, uy, uz) = face.ao_u();
    let (vx, vy, vz) = face.ao_v();
    let (ustride, vstride) = (pad_stride(ux, uy, uz), pad_stride(vx, vy, vz));
    let f_l = u32::from(pad.skylight[fi]);
    let f_bl = pad.blocklight[fi];

    // The ring-cell half along the normal that lies on the plane's FRONT
    // side — what a partial slab's single light value must describe to feed
    // a corner (see `slab_corner_open`). Boundary planes give the fixed halves
    // (front-half 0 for positive faces, 1 for negative); an interior plane at
    // 0.5 flips them, so a neighbouring bottom slab's open-top light DOES feed
    // the slab-top plane beside it.
    let front_half = {
        let (dx, dy, dz) = face.dir();
        if dx + dy + dz > 0 {
            (plane >= 0.25) as usize
        } else {
            (plane > 0.75) as usize
        }
    };

    // Whether the FRONT cell itself holds sub-cell matter: its interior
    // quadrant then joins the corner occlusion (the exposed ring of a face
    // something box-shaped stands on).
    let front_probe = probe_worthy(Block::from_id(pad.blocks[fi]));

    let mut occ = [[false; 3]; 3];
    let mut probe_cell = [[false; 3]; 3];
    let mut opq = [[false; 3]; 3];
    let mut sky = [[0u32; 3]; 3];
    let mut blk = [[LightRgb::ZERO; 3]; 3];
    let mut slab = [[SlabState::EMPTY; 3]; 3];
    for a in -1i32..=1 {
        for b in -1i32..=1 {
            if a == 0 && b == 0 {
                continue;
            }
            let i = (fi as isize + a as isize * ustride + b as isize * vstride) as usize;
            let cell = Block::from_id(pad.blocks[i]);
            // ONE dense flag word per ring cell: the shape questions below are
            // bit tests off it, not separate table lookups.
            let cf = cell.flags();
            let (ia, ib) = ((a + 1) as usize, (b + 1) as usize);
            // A full slab stack occludes AO and carries no light, exactly like
            // an opaque cube — without this it darkens corners twice (it blocks
            // the light flood, then still enters the smooth-light mean as a
            // dark open cell). Partial slab states are kept for the per-corner
            // octant gate below. The dense `is_slab` flag gates the state read.
            let slab_state = cf.is_slab().then(|| {
                let stored = SlabState::from_cell(pad.cell_states[i]);
                petramond_world::slab::normalize_state(cell, stored)
            });
            let full_stack = slab_state.is_some_and(|s| s.is_full());
            occ[ia][ib] = cf.occludes_ao() || full_stack;
            // A non-occluding cell that still holds sub-cell matter (a box
            // shape, a partial slab) gets corner-probe casting below.
            probe_cell[ia][ib] = !occ[ia][ib] && cf.has_box_shape();
            if smooth_light {
                opq[ia][ib] = cf.is_opaque() || full_stack;
                if !opq[ia][ib] {
                    sky[ia][ib] = u32::from(pad.skylight[i]);
                    blk[ia][ib] = pad.blocklight[i];
                    if let Some(state) = slab_state {
                        slab[ia][ib] = state;
                    }
                }
            }
        }
    }

    // Per corner, resolve AO + light from the gathered ring: its two edge cells
    // (`[iu][1]` along u, `[1][iv]` along v) and its diagonal (`[iu][iv]`).
    let signs = face.ao_signs();
    let mut ao = [3u32; 4];
    let mut light6 = [0u32; 4];
    let mut block6 = [BlockLight6::DARK; 4];
    let flat = fold_light(f_l, f_bl.channels().map(u32::from), SKY_FULL as u32);
    let (fx, fy, fz) = (front.x, front.y, front.z);
    for corner in 0..4 {
        let (su, sv) = signs[corner];
        let (iu, iv) = ((su + 1) as usize, (sv + 1) as usize);
        let (mut s1, mut s2, mut c) = (occ[iu][1], occ[1][iv], occ[iu][iv]);
        let mut q_int = false;
        if front_probe
            || (probe_cell[iu][1] && !s1)
            || (probe_cell[1][iv] && !s2)
            || (probe_cell[iu][iv] && !c)
        {
            let pk = corner_cast_probes(face, su, sv, plane);
            let cell_of = |s_u: i32, s_v: i32| {
                (
                    fx + s_u * ux + s_v * vx,
                    fy + s_u * uy + s_v * vy,
                    fz + s_u * uz + s_v * vz,
                )
            };
            let local = |p: [f32; 3], cl: (i32, i32, i32)| {
                [
                    p[0] - (cl.0 - fx) as f32,
                    p[1] - (cl.1 - fy) as f32,
                    p[2] - (cl.2 - fz) as f32,
                ]
            };
            let probe = |cl: (i32, i32, i32), (lo, hi): ([f32; 3], [f32; 3])| {
                nb.matter(cl, local(lo, cl), local(hi, cl))
            };
            if probe_cell[iu][1] && !s1 {
                s1 = probe(cell_of(su, 0), pk[0]);
            }
            if probe_cell[1][iv] && !s2 {
                s2 = probe(cell_of(0, sv), pk[1]);
            }
            if probe_cell[iu][iv] && !c {
                c = probe(cell_of(su, sv), pk[2]);
            }
            if front_probe {
                q_int = probe((fx, fy, fz), pk[3]);
            }
        }
        ao[corner] = quad_ao(q_int, s1, s2, c);
        if !smooth_light {
            (light6[corner], block6[corner]) = flat;
            continue;
        }
        let mut sum = f_l;
        // Per-channel mean: hues average in the linear light space only.
        let mut sum_block = f_bl.channels().map(u32::from);
        let mut cnt = 1u32;
        for (ia, ib, a, b) in [(iu, 1, su, 0), (1, iv, 0, sv), (iu, iv, su, sv)] {
            if opq[ia][ib] || !slab_corner_open(slab[ia][ib], face, a, b, su, sv, front_half) {
                continue;
            }
            sum += sky[ia][ib];
            let c = blk[ia][ib];
            sum_block[0] += c.r() as u32;
            sum_block[1] += c.g() as u32;
            sum_block[2] += c.b() as u32;
            cnt += 1;
        }
        (light6[corner], block6[corner]) = fold_light_smooth(sum, sum_block, cnt);
    }
    (ao, light6, block6)
}

/// A cell's own flat light — what flat-lit emitters (plants, models) carry:
/// its skylight and block light folded into the packed channels.
pub(super) fn cell_light(nb: &Neighbourhood<'_>, p: IVec3) -> (u32, BlockLight6) {
    let l = u32::from(nb.skylight(p));
    let bl = nb.blocklight(p).channels().map(u32::from);
    fold_light(l, bl, SKY_FULL as u32)
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

    /// The fraction scales the lift: none leaves the sampled face untouched, all
    /// raises every corner to the emission with open AO, and a part lands in
    /// between — never below the sample, never past the target.
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
