//! Per-face emission for the chunk mesher: folding sky/block light into the
//! packed vertex channels, the partial-slab light gate the face-lighting
//! gather applies per corner, and the packed-vertex face pushes.

use crate::vertex::BlockLightVertexExt;
use petramond_world::block_state::SlabState;
use petramond_world::light::BlockLight6;
use petramond_world::tile::Tile;

use super::builder::CornerLight;
use super::face::{should_flip, Face, FaceShading};
use super::vertex::{
    pack_cell_uv, pack_normal_code, pack_overlay, pack_uv_turn, pack_uv_turn2, pack_vertex, Vertex,
    UV_MODE_CELL_LOCAL, UV_MODE_NONE, UV_MODE_SHIFT,
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
    let (u, v) = (face.ao_u(), face.ao_v());
    let pick = |uc: i32, vc: i32| -> usize {
        if uc != 0 {
            hu
        } else if vc != 0 {
            hv
        } else {
            front_half
        }
    };
    !petramond_world::slab::half_cell_occupied(state, pick(u.x, v.x), pick(u.y, v.y), pick(u.z, v.z))
}

/// The light and tint of flat-lit geometry (plant planes, the torch pole):
/// one value for every corner, no AO, no directional shade.
#[derive(Copy, Clone)]
pub(super) struct FlatLit {
    pub(super) tint: [f32; 3],
    /// The cell's skylight, 0..=63.
    pub(super) sky6: u32,
    pub(super) block: BlockLight6,
}

impl FlatLit {
    /// A flat-lit vertex at `pos` for `corner` of a quad textured with `tile`:
    /// shade index 0 (top, no directional darkening), AO 3, no overlay.
    pub(super) fn vertex(self, pos: [f32; 3], tile: Tile, corner: u32) -> Vertex {
        Vertex {
            pos,
            tint: self.block.tint_word(self.tint),
            packed: pack_vertex(tile.index() as u32, corner, 0, false, 3, self.sky6)
                | self.block.packed_bits(),
            packed2: self.block.packed2_bits(),
        }
    }
}

/// One cube-face quad to push: its geometry, its art, and its per-corner
/// light.
pub(super) struct FaceSpec {
    pub(super) face: Face,
    /// The four corners, in [`Face::quad_box`] order.
    pub(super) corners: [[f32; 3]; 4],
    pub(super) base_tile: Tile,
    /// The overlay lane's payload: an overlay tile index, or a fluid
    /// surface's flow angle.
    pub(super) overlay: u32,
    /// Whether the shader draws `overlay` as an overlay tile.
    pub(super) has_overlay: bool,
    /// Explicit cell-local UVs per corner (the log remap); `None` maps the
    /// tile by corner id.
    pub(super) cell_uvs: Option<[(u32, u32); 4]>,
    pub(super) uv_turn: u32,
    pub(super) tint: [f32; 3],
    /// Per-corner `(ao, sky light, block light)`.
    pub(super) light: CornerLight,
    pub(super) dyed: bool,
}

/// Push one cube face and return the index of its first vertex.
pub(super) fn push_cube_face(vbuf: &mut Vec<Vertex>, spec: &FaceSpec) -> u32 {
    let FaceSpec {
        face,
        corners,
        base_tile,
        overlay,
        has_overlay,
        cell_uvs,
        uv_turn,
        tint,
        light: (ao, light6, block6),
        dyed,
    } = *spec;
    let shade_idx = face.shade_idx();
    let dyed = if dyed { super::vertex::DYED_FLAG2 } else { 0 };
    let packed_uv_mode = if cell_uvs.is_some() {
        UV_MODE_CELL_LOCAL
    } else {
        UV_MODE_NONE
    };
    // A CELL_LOCAL face carries its full mapping in the explicit UV it packs
    // (box sets, the log remap), so its turn bits stay zero — the shaders
    // apply the packed turn to plain cube faces only.
    let packed_turn = if cell_uvs.is_some() { 0 } else { uv_turn };
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
                | (packed_uv_mode << UV_MODE_SHIFT)
                | pack_uv_turn(packed_turn),
            packed2: light.packed2_bits()
                | pack_overlay(overlay)
                | explicit_uv
                | pack_normal_code(face.normal_code())
                | dyed
                | pack_uv_turn2(packed_turn),
        });
    }
    start
}

#[cfg(test)]
mod fold_light_tests {
    use super::*;
    use petramond_world::chunk::SKY_FULL;

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
