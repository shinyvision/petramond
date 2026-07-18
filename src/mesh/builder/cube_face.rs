use crate::atlas::Tile;
use crate::block::Block;
use crate::block_state::{LogAxis, SlabState};
use crate::chunk::SKY_FULL;
use crate::facing::Facing;

use super::super::face::{vertex_ao, Face};
use super::super::face_emit::{fold_light, fold_light_smooth, slab_corner_open};

/// The horizontal cube face a directional block's front points to, for its
/// stored entity [`Facing`] (furnace/chest fronts).
#[inline]
pub(super) fn facing_face(facing: Facing) -> Face {
    match facing {
        Facing::North => Face::NegZ,
        Facing::South => Face::PosZ,
        Facing::West => Face::NegX,
        Facing::East => Face::PosX,
    }
}

#[inline]
pub(super) fn cube_face_tile(
    block: Block,
    face: Face,
    tiles: [Tile; 3],
    front: Option<(Face, Tile)>,
    log_axis: LogAxis,
) -> Tile {
    let [tile_top, tile_bot, tile_side] = tiles;
    if block.is_log() {
        return match (log_axis, face) {
            (LogAxis::X, Face::PosX) | (LogAxis::Y, Face::PosY) | (LogAxis::Z, Face::PosZ) => {
                tile_top
            }
            (LogAxis::X, Face::NegX) | (LogAxis::Y, Face::NegY) | (LogAxis::Z, Face::NegZ) => {
                tile_bot
            }
            _ => tile_side,
        };
    }
    match face {
        Face::PosY => tile_top,
        Face::NegY => tile_bot,
        // A row-declared `front` tile replaces the side tile on the one face
        // the block's stored entity facing points to (furnace fronts).
        _ => match front {
            Some((front_face, front_tile)) if face == front_face => front_tile,
            _ => tile_side,
        },
    }
}

#[inline]
fn uv_16ths(value: f32) -> u32 {
    (value.clamp(0.0, 1.0) * 16.0).round() as u32
}

#[inline]
pub(super) fn log_side_cell_uvs(
    axis: LogAxis,
    face: Face,
    corners: [[f32; 3]; 4],
    base: [f32; 3],
) -> Option<[(u32, u32); 4]> {
    let mut uvs = [(0, 0); 4];
    for (i, corner) in corners.into_iter().enumerate() {
        let local = [
            corner[0] - base[0],
            corner[1] - base[1],
            corner[2] - base[2],
        ];
        let [u, v] = face.log_side_cell_uv(axis, local)?;
        uvs[i] = (uv_16ths(u), uv_16ths(v));
    }
    Some(uvs)
}

/// One cube face's per-corner AO + smooth light (skylight/block-light + warm amount),
/// gathered from the shared 3×3 tangent-plane ring around the front voxel F ONCE. The
/// four corners share these eight ring cells (each edge cell feeds two corners, each
/// diagonal one), so a single gather replaces per-corner re-reads. `occ` = AO occluders
/// (opaque cubes AND leaves, for canopy self-occlusion); `opq` = full-opaque, which carry
/// no light and so are excluded from the smooth-light mean (leaves differ between the two,
/// hence both bits). The centre cell (a=b=0) is F itself and is never sampled, so skipped.
///
/// Split from the vertex push so the greedy mesher can test a face for flatness (all four
/// corners equal — the merge condition) before deciding to emit it per-cell or merge it.
#[allow(clippy::too_many_arguments)]
pub(in crate::mesh) fn cube_face_lighting<B, S, L, K>(
    face: Face,
    fx: i32,
    fy: i32,
    fz: i32,
    f_l: u32,
    f_bl: u32,
    smooth_light: bool,
    block_at: &B,
    slab_at: &S,
    neighbour_light: &L,
    neighbour_blocklight: &K,
) -> ([u32; 4], [u32; 4], [u32; 4], [f32; 4])
where
    B: Fn(i32, i32, i32) -> Block,
    S: Fn(i32, i32, i32) -> Option<SlabState>,
    L: Fn(i32, i32, i32) -> u8,
    K: Fn(i32, i32, i32) -> u8,
{
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
                fx + a * ux + b * vx,
                fy + a * uy + b * vy,
                fz + a * uz + b * vz,
            );
            let cell = block_at(cx, cy, cz);
            let (ia, ib) = ((a + 1) as usize, (b + 1) as usize);
            // A full slab stack occludes AO and carries no light, exactly like an
            // opaque cube — without this it darkens corners twice (it blocks the
            // light flood, then still enters the smooth-light mean as a dark open
            // cell). Partial slab states are kept for the per-corner octant gate
            // below. The dense `is_slab` flag gates the state lookup.
            let slab_state = if cell.is_slab() {
                slab_at(cx, cy, cz)
            } else {
                None
            };
            let full_stack = slab_state.is_some_and(|s| s.is_full());
            occ[ia][ib] = cell.occludes_ao() || full_stack;
            if smooth_light {
                opq[ia][ib] = cell.is_opaque() || full_stack;
                if !opq[ia][ib] {
                    sky[ia][ib] = neighbour_light(cx, cy, cz) as u32;
                    blk[ia][ib] = neighbour_blocklight(cx, cy, cz) as u32;
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

/// A cube face's `(normal, U, V)` local axes (0=X, 1=Y, 2=Z), derived from `Face::quad_box`
/// so the greedy slice's `(u,v)` grid and a merged quad's tiled UV (W tiles along U, H along
/// V) align with `corner_local`: normal-X → U=Z,V=Y; normal-Y → U=X,V=Z; normal-Z → U=X,V=Y.
#[inline]
pub(in crate::mesh) fn face_axes(face: Face) -> (usize, usize, usize) {
    match face {
        Face::PosX | Face::NegX => (0, 2, 1),
        Face::PosY | Face::NegY => (1, 0, 2),
        Face::PosZ | Face::NegZ => (2, 0, 1),
    }
}

/// Index of a face in [`FACES`] — the per-direction plane in [`GreedyScratch::faces`]. Must
/// match `FACES.into_iter().enumerate()` in [`emit_greedy_quads`].
#[inline]
pub(super) fn face_index(face: Face) -> usize {
    match face {
        Face::PosX => 0,
        Face::NegX => 1,
        Face::PosY => 2,
        Face::NegY => 3,
        Face::PosZ => 4,
        Face::NegZ => 5,
    }
}
