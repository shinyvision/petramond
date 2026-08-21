//! Per-face emission for the chunk mesher: folding sky/block light into the
//! packed vertex channels, one cube face's per-corner AO/smooth-light gather
//! over the section pad, and the packed-vertex face pushes.

use crate::vertex::BlockLightVertexExt;
use petramond_world::block::CellView;
use petramond_world::block_state::SlabState;
use petramond_world::chunk::SKY_FULL;
use petramond_world::light::{BlockLight6, LightRgb};
use petramond_world::tile::Tile;

use petramond_world::block::Block;

use super::builder::{mesh_pad_idx, SectionMeshPad};
use super::face::{quad_ao, should_flip, Face};
use super::vertex::{
    pack_cell_uv, pack_normal_code, pack_overlay, pack_vertex, Vertex, UV_MODE_CELL_LOCAL,
    UV_MODE_SHIFT,
};

/// Fold a cell's (or neighbourhood-summed) skylight + block-light into the
/// packed vertex channels: a 6-bit `sky6` and a three-channel [`BlockLight6`].
/// `sum_sky`/`sum_block` are x2-scale sums over `denom = cnt * SKY_FULL` cells
/// (`cnt = 1`, `denom = SKY_FULL` for a single cell); the block sums are PER
/// CHANNEL, because a mean of two differently-coloured lights is only
/// meaningful in a linear space — the hue is derived after the average, never
/// averaged itself.
///
/// The channels stay SEPARATE in the vertex (`packed` bits 23..29 = sky, block
/// light split across `packed2` + the chroma lanes) so the shader can dim the
/// sky term without dimming torch light; the shader recombines with a
/// per-channel `max(sky_term, block_term)`. Because the per-channel quantizer
/// is monotone non-decreasing, `max(sky6, block6.luminance())` equals
/// `quantize(max(sum_sky, sum_block))` for COLOURLESS light — the value the
/// single channel used to hold — so white light is bit-identical to the
/// pre-colour output.
#[inline]
pub(super) fn fold_light(sum_sky: u32, sum_block: [u32; 3], denom: u32) -> (u32, BlockLight6) {
    let sky6 = ((sum_sky * 63 + denom / 2) / denom).min(63);
    // Nearly all daylit terrain reaches no emitter at all: one OR-and-test
    // skips all three channel divides (and yields the canonical dark cell,
    // which is what those divides would have produced).
    let block = if (sum_block[0] | sum_block[1] | sum_block[2]) == 0 {
        BlockLight6::DARK
    } else {
        let q = |sum: u32| ((sum * 63 + denom / 2) / denom).min(63);
        BlockLight6::new(q(sum_block[0]), q(sum_block[1]), q(sum_block[2]))
    };
    (sky6, block)
}

/// Like [`fold_light`] but for the per-corner smooth-light mean over `cnt` cells
/// (`1..=4`). The divisor `cnt * SKY_FULL` is one of four constants, so matching on
/// `cnt` lets the compiler lower each arm's integer division to a multiply-shift —
/// removing the last per-corner division from the emit hot loop. Byte-identical to
/// `fold_light(sum_sky, sum_block, cnt * SKY_FULL)`.
#[inline]
pub(super) fn fold_light_smooth(sum_sky: u32, sum_block: [u32; 3], cnt: u32) -> (u32, BlockLight6) {
    #[inline(always)]
    fn quant(sum: u32, cnt: u32) -> u32 {
        let v = sum * 63;
        match cnt {
            1 => (v + 15) / 30,
            2 => (v + 30) / 60,
            3 => (v + 45) / 90,
            _ => (v + 60) / 120,
        }
        .min(63)
    }
    let sky6 = quant(sum_sky, cnt);
    // The torch-free common case (nearly all terrain) skips the block-channel
    // divides entirely: a zero sum quantizes to exactly 0.
    let block = if (sum_block[0] | sum_block[1] | sum_block[2]) == 0 {
        BlockLight6::DARK
    } else {
        BlockLight6::new(
            quant(sum_block[0], cnt),
            quant(sum_block[1], cnt),
            quant(sum_block[2], cnt),
        )
    };
    (sky6, block)
}

/// Whether ring cell `(a, b)` (tangent offsets from the front voxel) lends its
/// light to face corner `(su, sv)`. A partial slab's single light value
/// describes its OPEN half, so it only feeds a corner whose touching half-cell
/// octant is open: a wall base resting on a top-slab floor must not blend in
/// the under-floor darkness sealed away behind the slab's solid top half.
/// `SlabState::EMPTY` means "not a partial slab" — always open. `front_half`
/// is the ring-cell half along the normal on the face plane's FRONT side
/// (the gather computes it from the plane height — an interior plane's front
/// half differs from a boundary plane's).
#[inline]
pub(super) fn slab_corner_open(
    state: SlabState,
    face: Face,
    a: i32,
    b: i32,
    su: i32,
    sv: i32,
    front_half: usize,
) -> bool {
    if state == SlabState::EMPTY {
        return true;
    }
    // The touching octant: along the normal, the half in front of the face
    // plane; along a tangent axis, the half toward the front voxel when the
    // cell is offset there (a/b != 0), else the half on the corner's side.
    let hu = ((su > 0) != (a != 0)) as usize;
    let hv = ((sv > 0) != (b != 0)) as usize;
    let (ux, uy, uz) = face.ao_u();
    let (vx, vy, vz) = face.ao_v();
    let pick = |uc: i32, vc: i32| -> usize {
        if uc != 0 {
            hu
        } else if vc != 0 {
            hv
        } else {
            front_half
        }
    };
    !petramond_world::slab::half_cell_occupied(state, pick(ux, vx), pick(uy, vy), pick(uz, vz))
}

/// The flat-array step of one pad cell along `(dx, dy, dz)`.
#[inline]
fn pad_stride(dx: i32, dy: i32, dz: i32) -> isize {
    let pad = super::builder::MESH_PAD_SIDE as isize;
    dx as isize + dz as isize * pad + dy as isize * pad * pad
}

#[allow(clippy::too_many_arguments)]
pub(super) fn cube_face_lighting_pad<P>(
    pad: &SectionMeshPad<'_>,
    face: Face,
    fx: usize,
    fy: usize,
    fz: usize,
    // The front voxel's WORLD coords, for the sub-cell AO cast probes (the
    // probe closure speaks world cells like the closure-path gather's).
    wf: (i32, i32, i32),
    f_l: u32,
    f_bl: LightRgb,
    smooth_light: bool,
    probe: &P,
) -> ([u32; 4], [u32; 4], [BlockLight6; 4])
where
    P: Fn((i32, i32, i32), [f32; 3], [f32; 3]) -> bool,
{
    let (ux, uy, uz) = face.ao_u();
    let (vx, vy, vz) = face.ao_v();
    // The ring's eight pad indices are the front voxel's plus a constant
    // stride per tangent step — the pad is a flat array and the face's tangent
    // axes are fixed, so the coordinate arithmetic and its two multiplies per
    // ring cell collapse to one add. The ring never leaves the pad: the axis
    // that can sit on the pad's outer plane is the face NORMAL, and the ring
    // only steps along the two tangents.
    let (fi, ustride, vstride) = (
        mesh_pad_idx(fx, fy, fz) as isize,
        pad_stride(ux, uy, uz),
        pad_stride(vx, vy, vz),
    );
    // The pad path meshes cube faces only, always on the voxel boundary.
    let plane = super::builder::boundary_plane(face, wf);
    let front_half = {
        let (dx, dy, dz) = face.dir();
        (dx + dy + dz < 0) as usize
    };

    // Front cell's own sub-cell matter joins the interior quadrant — the
    // closure gather's `front_probe`, mirrored for byte parity.
    let front_probe = super::builder::probe_worthy(pad.block_at_pad(fx, fy, fz));

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
            let i = (fi + a as isize * ustride + b as isize * vstride) as usize;
            let cell = Block::from_id(pad.blocks[i]);
            // ONE dense flag word per ring cell: the four shape questions
            // below become bit tests instead of four table lookups.
            let cf = cell.flags();
            let (ia, ib) = ((a + 1) as usize, (b + 1) as usize);
            // Full slab stacks occlude AO/light like opaque cubes; partial slab
            // states are kept for the per-corner octant gate below — mirrors the
            // closure-path gather in `cube_face_lighting` (byte parity).
            let slab_state = cf.is_slab().then(|| {
                petramond_world::slab::normalize_state(
                    cell,
                    petramond_world::block_state::SlabState::from_cell(pad.cell_states[i]),
                )
            });
            let full_stack = slab_state.is_some_and(|s| s.is_full());
            occ[ia][ib] = cf.occludes_ao() || full_stack;
            probe_cell[ia][ib] = !occ[ia][ib] && cf.has_box_shape();
            if smooth_light {
                opq[ia][ib] = cf.is_opaque() || full_stack;
                if !opq[ia][ib] {
                    sky[ia][ib] = pad.skylight[i] as u32;
                    blk[ia][ib] = pad.blocklight[i];
                    if let Some(state) = slab_state {
                        slab[ia][ib] = state;
                    }
                }
            }
        }
    }

    let signs = face.ao_signs();
    let mut ao = [3u32; 4];
    let mut light6 = [0u32; 4];
    let mut block6 = [BlockLight6::DARK; 4];
    let flat = fold_light(f_l, f_bl.channels().map(u32::from), SKY_FULL as u32);
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
            let pk = super::builder::corner_cast_probes(face, wf, su, sv, plane);
            let cell_of = |s_u: i32, s_v: i32| {
                (
                    wf.0 + s_u * ux + s_v * vx,
                    wf.1 + s_u * uy + s_v * vy,
                    wf.2 + s_u * uz + s_v * vz,
                )
            };
            let local = |p: [f32; 3], cl: (i32, i32, i32)| {
                [p[0] - cl.0 as f32, p[1] - cl.1 as f32, p[2] - cl.2 as f32]
            };
            if probe_cell[iu][1] && !s1 {
                let cl = cell_of(su, 0);
                s1 = probe(cl, local(pk[0].0, cl), local(pk[0].1, cl));
            }
            if probe_cell[1][iv] && !s2 {
                let cl = cell_of(0, sv);
                s2 = probe(cl, local(pk[1].0, cl), local(pk[1].1, cl));
            }
            if probe_cell[iu][iv] && !c {
                let cl = cell_of(su, sv);
                c = probe(cl, local(pk[2].0, cl), local(pk[2].1, cl));
            }
            if front_probe {
                let cl = wf;
                q_int = probe(cl, local(pk[3].0, cl), local(pk[3].1, cl));
            }
        }
        ao[corner] = quad_ao(q_int, s1, s2, c);
        if !smooth_light {
            (light6[corner], block6[corner]) = flat;
            continue;
        }
        let mut sum = f_l;
        // The block mean is taken PER CHANNEL, in the linear light space: an
        // average of two hues is only meaningful there.
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

#[allow(clippy::too_many_arguments)]
pub(super) fn push_cube_face_with_cell_uvs(
    vbuf: &mut Vec<Vertex>,
    corners: [[f32; 3]; 4],
    base_tile: Tile,
    overlay: u32,
    has_overlay: bool,
    uv_mode: u32,
    cell_uvs: Option<[(u32, u32); 4]>,
    tint: [f32; 3],
    face: Face,
    ao: [u32; 4],
    light6: [u32; 4],
    block6: [BlockLight6; 4],
    dyed: bool,
) -> u32 {
    let shade_idx = face.shade_idx();
    let dyed = if dyed { super::vertex::DYED_FLAG2 } else { 0 };
    let packed_uv_mode = if cell_uvs.is_some() {
        UV_MODE_CELL_LOCAL
    } else {
        uv_mode
    };
    let start = vbuf.len() as u32;
    // The AO split must run along the darker diagonal. With an implied
    // triangulation there is no second index pattern to switch to, so the
    // corners are ROTATED by one instead — the same two triangles, the same
    // winding, and every vertex keeps its own corner id (and therefore its UV).
    let rot = usize::from(should_flip(ao));
    for k in 0..4usize {
        let corner = (k + rot) & 3;
        let p = corners[corner];
        let explicit_uv = cell_uvs
            .map(|uvs| {
                let (u, v) = uvs[corner];
                pack_cell_uv(u, v)
            })
            .unwrap_or(0);
        let light = block6[corner];
        vbuf.push(Vertex {
            pos: p,
            tint: light.tint_word(tint),
            packed: pack_vertex(
                base_tile.index() as u32,
                corner as u32,
                shade_idx,
                has_overlay,
                ao[corner],
                light6[corner],
            ) | light.packed_bits()
                | (packed_uv_mode << UV_MODE_SHIFT),
            packed2: light.packed2_bits()
                | pack_overlay(overlay)
                | explicit_uv
                | pack_normal_code(face.normal_code())
                | dyed,
        });
    }
    start
}

#[cfg(test)]
mod fold_light_tests {
    use super::*;

    /// The light-channel split's terrain identity: per-channel quantization is
    /// monotone, so for COLOURLESS light `max(sky6, block6)` reproduces the
    /// pre-split single channel (`quantize(max(sums))`) exactly — the shader's
    /// `max(sky_term, block_term)` at identity scale therefore matches the old
    /// fold bit-for-bit. Also pins `fold_light_smooth`'s constant-divisor arms
    /// to `fold_light` byte parity.
    #[test]
    fn split_channels_reproduce_the_max_folded_single_channel() {
        for cnt in 1u32..=4 {
            let denom = cnt * SKY_FULL as u32;
            for sky in 0..=denom {
                for blk in 0..=denom {
                    let (s6, b6) = fold_light(sky, [blk; 3], denom);
                    let old = ((sky.max(blk) * 63 + denom / 2) / denom).min(63);
                    assert_eq!(s6.max(b6.luminance()), old, "sky={sky} blk={blk}");
                    assert_eq!(b6, BlockLight6::grey(b6.r()), "white light stays white");
                    let smooth = fold_light_smooth(sky, [blk; 3], cnt);
                    assert_eq!(
                        smooth,
                        (s6, b6),
                        "smooth arm must stay byte-identical at cnt={cnt}"
                    );
                }
            }
        }
    }

    /// A coloured mean must be taken channel by channel: the fold may never
    /// collapse the hue into a brightness and re-expand it. Two cells lit by
    /// different colours average to the mean of each channel, and the canonical
    /// dark cell survives the fast path.
    #[test]
    fn the_block_mean_is_taken_per_channel() {
        let denom = 2 * SKY_FULL as u32;
        let purple = [22u32, 6, 30];
        let green = [4u32, 28, 8];
        let sum = [
            purple[0] + green[0],
            purple[1] + green[1],
            purple[2] + green[2],
        ];
        let (_, mixed) = fold_light(0, sum, denom);
        let q = |v: u32| ((v * 63 + denom / 2) / denom).min(63);
        assert_eq!(mixed.channels(), [q(sum[0]), q(sum[1]), q(sum[2])]);
        // ... and that is NOT the same as averaging luminance and re-tinting.
        assert_ne!(mixed, BlockLight6::grey(mixed.luminance()));
        assert!(fold_light(0, [0; 3], denom).1.is_dark());
        assert!(fold_light_smooth(0, [0; 3], 3).1.is_dark());
    }
}
