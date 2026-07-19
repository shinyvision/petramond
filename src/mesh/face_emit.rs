//! Per-face emission for the chunk mesher: folding sky/block light into the two
//! packed 6-bit vertex channels (+ warm tint amount), one cube face's per-corner
//! AO/smooth-light gather over the section pad, and the packed-vertex face pushes.

use crate::atlas::Tile;
use crate::block_state::SlabState;
use crate::chunk::SKY_FULL;
use crate::torch::{warm_amount, warm_tint};

use super::builder::{mesh_pad_idx, SectionMeshPad};
use super::face::{should_flip, vertex_ao, Face};
use super::vertex::{
    pack_cell_uv, pack_normal_code, pack_tint, pack_vertex, pack_vertex2, Vertex,
    UV_MODE_CELL_LOCAL, UV_MODE_SHIFT,
};

/// Fold a cell's (or neighbourhood-summed) skylight + block-light into TWO packed
/// 6-bit brightness channels `(sky6, block6)` plus a 0..1 warm amount.
/// `sum_sky`/`sum_block` are x2-scale sums over `denom = cnt * SKY_FULL` cells
/// (`cnt = 1`, `denom = SKY_FULL` for a single cell). The channels stay SEPARATE
/// in the vertex (`packed` bits 23..29 = sky, `packed2` bits 0..6 = block) so the
/// shader can dim the sky term without dimming torch light; the shader recombines
/// with `max(sky_term, block_term)`. Because the per-channel quantizer is monotone
/// non-decreasing, `max(sky6, block6) == quantize(max(sum_sky, sum_block))` — the
/// value the single channel used to hold — so at identity scale the final light is
/// bit-identical to the pre-split output. Warm comes from the shared
/// [`warm_amount`](crate::torch::warm_amount) so static blocks and dynamic
/// geometry warm identically.
#[inline]
pub(super) fn fold_light(sum_sky: u32, sum_block: u32, denom: u32) -> (u32, u32, f32) {
    let sky6 = ((sum_sky * 63 + denom / 2) / denom).min(63);
    let block6 = ((sum_block * 63 + denom / 2) / denom).min(63);
    // `warm_amount` is `block01 * (1 - sky01) * strength`, so no block-light means
    // exactly 0.0 warmth — skip its two f32 divisions in the torch-free common case
    // (nearly all daylit terrain). Byte-identical: the multiply by a zero block term
    // yields +0.0, the same bits this returns.
    let warm = if sum_block == 0 {
        0.0
    } else {
        warm_amount(
            sum_sky as f32 / denom as f32,
            sum_block as f32 / denom as f32,
        )
    };
    (sky6, block6, warm)
}

/// Like [`fold_light`] but for the per-corner smooth-light mean over `cnt` cells
/// (`1..=4`). The divisor `cnt * SKY_FULL` is one of four constants, so matching on
/// `cnt` lets the compiler lower each arm's integer division to a multiply-shift —
/// removing the last per-corner division from the emit hot loop. Byte-identical to
/// `fold_light(sum_sky, sum_block, cnt * SKY_FULL)`.
#[inline]
pub(super) fn fold_light_smooth(sum_sky: u32, sum_block: u32, cnt: u32) -> (u32, u32, f32) {
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
    // divide entirely: a zero sum quantizes to exactly 0.
    let block6 = if sum_block == 0 {
        0
    } else {
        quant(sum_block, cnt)
    };
    let warm = if sum_block == 0 {
        0.0
    } else {
        let denom = cnt * SKY_FULL as u32;
        warm_amount(
            sum_sky as f32 / denom as f32,
            sum_block as f32 / denom as f32,
        )
    };
    (sky6, block6, warm)
}

/// Whether ring cell `(a, b)` (tangent offsets from the front voxel) lends its
/// light to face corner `(su, sv)`. A partial slab's single light value
/// describes its OPEN half, so it only feeds a corner whose touching half-cell
/// octant is open: a wall base resting on a top-slab floor must not blend in
/// the under-floor darkness sealed away behind the slab's solid top half.
/// `SlabState::EMPTY` means "not a partial slab" — always open.
#[inline]
pub(super) fn slab_corner_open(
    state: SlabState,
    face: Face,
    a: i32,
    b: i32,
    su: i32,
    sv: i32,
) -> bool {
    if state == SlabState::EMPTY {
        return true;
    }
    // The touching octant: along the normal, the half against the face plane;
    // along a tangent axis, the half toward the front voxel when the cell is
    // offset there (a/b != 0), else the half on the corner's side.
    let hu = ((su > 0) != (a != 0)) as usize;
    let hv = ((sv > 0) != (b != 0)) as usize;
    let (dx, dy, dz) = face.dir();
    let (ux, uy, uz) = face.ao_u();
    let (vx, vy, vz) = face.ao_v();
    let pick = |d: i32, uc: i32, vc: i32| -> usize {
        if uc != 0 {
            hu
        } else if vc != 0 {
            hv
        } else {
            (d < 0) as usize
        }
    };
    !crate::slab::half_cell_occupied(state, pick(dx, ux, vx), pick(dy, uy, vy), pick(dz, uz, vz))
}

pub(super) fn cube_face_lighting_pad(
    pad: &SectionMeshPad<'_>,
    face: Face,
    fx: usize,
    fy: usize,
    fz: usize,
    f_l: u32,
    f_bl: u32,
    smooth_light: bool,
) -> ([u32; 4], [u32; 4], [u32; 4], [f32; 4]) {
    let (ux, uy, uz) = face.ao_u();
    let (vx, vy, vz) = face.ao_v();

    let mut occ = [[false; 3]; 3];
    let mut opq = [[false; 3]; 3];
    let mut sky = [[0u32; 3]; 3];
    let mut blk = [[0u32; 3]; 3];
    let mut slab = [[SlabState::EMPTY; 3]; 3];
    for a in -1i32..=1 {
        for b in -1i32..=1 {
            if a == 0 && b == 0 {
                continue;
            }
            let (cx, cy, cz) = (
                (fx as i32 + a * ux + b * vx) as usize,
                (fy as i32 + a * uy + b * vy) as usize,
                (fz as i32 + a * uz + b * vz) as usize,
            );
            let cell = pad.block_at_pad(cx, cy, cz);
            let i = mesh_pad_idx(cx, cy, cz);
            let (ia, ib) = ((a + 1) as usize, (b + 1) as usize);
            // Full slab stacks occlude AO/light like opaque cubes; partial slab
            // states are kept for the per-corner octant gate below — mirrors the
            // closure-path gather in `cube_face_lighting` (byte parity).
            let slab_state = cell
                .is_slab()
                .then(|| crate::slab::normalize_state(cell, pad.slab_states[i]));
            let full_stack = slab_state.is_some_and(|s| s.is_full());
            occ[ia][ib] = cell.occludes_ao() || full_stack;
            if smooth_light {
                opq[ia][ib] = cell.is_opaque() || full_stack;
                if !opq[ia][ib] {
                    sky[ia][ib] = pad.skylight[i] as u32;
                    blk[ia][ib] = pad.blocklight[i] as u32;
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
    let mut block6 = [0u32; 4];
    let mut warm = [0f32; 4];
    let flat = fold_light(f_l, f_bl, SKY_FULL as u32);
    for corner in 0..4 {
        let (su, sv) = signs[corner];
        let (iu, iv) = ((su + 1) as usize, (sv + 1) as usize);
        ao[corner] = vertex_ao(occ[iu][1], occ[1][iv], occ[iu][iv]);
        if !smooth_light {
            (light6[corner], block6[corner], warm[corner]) = flat;
            continue;
        }
        let mut sum = f_l;
        let mut sum_block = f_bl;
        let mut cnt = 1u32;
        for (ia, ib, a, b) in [(iu, 1, su, 0), (1, iv, 0, sv), (iu, iv, su, sv)] {
            if opq[ia][ib] || !slab_corner_open(slab[ia][ib], face, a, b, su, sv) {
                continue;
            }
            sum += sky[ia][ib];
            sum_block += blk[ia][ib];
            cnt += 1;
        }
        (light6[corner], block6[corner], warm[corner]) = fold_light_smooth(sum, sum_block, cnt);
    }
    (ao, light6, block6, warm)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn push_cube_face_with_cell_uvs(
    vbuf: &mut Vec<Vertex>,
    ibuf: &mut Vec<u32>,
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
    block6: [u32; 4],
    warm: [f32; 4],
) -> [u32; 6] {
    let shade_idx = face.shade_idx();
    let packed_uv_mode = if cell_uvs.is_some() {
        UV_MODE_CELL_LOCAL
    } else {
        uv_mode
    };
    let start = vbuf.len() as u32;
    for (corner, p) in corners.into_iter().enumerate() {
        let explicit_uv = cell_uvs
            .map(|uvs| {
                let (u, v) = uvs[corner];
                pack_cell_uv(u, v)
            })
            .unwrap_or(0);
        vbuf.push(Vertex {
            pos: p,
            // Warm the face tint per corner by however much torch light reaches it,
            // so the glow fades smoothly across the surface (0 warm = unchanged, so
            // skip the multiply entirely — the torch-free common case).
            tint: pack_tint(if warm[corner] == 0.0 {
                tint
            } else {
                warm_tint(tint, warm[corner])
            }),
            packed: pack_vertex(
                base_tile.index() as u32,
                corner as u32,
                shade_idx,
                overlay,
                has_overlay,
                ao[corner],
                light6[corner],
            ) | (packed_uv_mode << UV_MODE_SHIFT),
            packed2: pack_vertex2(block6[corner])
                | explicit_uv
                | pack_normal_code(face.normal_code()),
        });
    }
    // Flip the triangulation so the split runs along the darker diagonal -- keeps
    // the AO gradient symmetric (no bright bleed).
    let tris: [u32; 6] = if should_flip(ao) {
        [start, start + 1, start + 3, start + 1, start + 2, start + 3]
    } else {
        [start, start + 1, start + 2, start, start + 2, start + 3]
    };
    ibuf.extend_from_slice(&tris);
    tris
}

#[cfg(test)]
mod fold_light_tests {
    use super::*;

    /// The light-channel split's terrain identity: per-channel quantization is
    /// monotone, so `max(sky6, block6)` reproduces the pre-split single channel
    /// (`quantize(max(sums))`) exactly — the shader's `max(sky_term, block_term)`
    /// at identity scale therefore matches the old fold bit-for-bit. Also pins
    /// `fold_light_smooth`'s constant-divisor arms to `fold_light` byte parity.
    #[test]
    fn split_channels_reproduce_the_max_folded_single_channel() {
        for cnt in 1u32..=4 {
            let denom = cnt * SKY_FULL as u32;
            for sky in 0..=denom {
                for blk in 0..=denom {
                    let (s6, b6, warm) = fold_light(sky, blk, denom);
                    let old = ((sky.max(blk) * 63 + denom / 2) / denom).min(63);
                    assert_eq!(s6.max(b6), old, "sky={sky} blk={blk} denom={denom}");
                    let smooth = fold_light_smooth(sky, blk, cnt);
                    assert_eq!(
                        smooth,
                        (s6, b6, warm),
                        "smooth arm must stay byte-identical at cnt={cnt}"
                    );
                }
            }
        }
    }
}
