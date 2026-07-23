use crate::atlas::Tile;
use crate::block::Block;
use crate::block_state::{LogAxis, SlabState};
use crate::chunk::SKY_FULL;
use crate::facing::Facing;

use super::super::face::{quad_ao, Face};
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

/// Whether a NON-occluding ring cell still deserves a sub-cell AO cast probe:
/// the box-shape families (and partial slabs, handled by the caller) occupy
/// part of their cell, so a corner pocket inside them can be solid even
/// though the whole cell is not.
#[inline]
pub(in crate::mesh) fn probe_worthy(block: Block) -> bool {
    matches!(
        block.shape_family(),
        crate::block::ShapeFamily::Stair
            | crate::block::ShapeFamily::Fence
            | crate::block::ShapeFamily::Pane
            | crate::block::ShapeFamily::Ladder
            | crate::block::ShapeFamily::Custom
    )
}

/// The four sub-cell AO cast probe POCKETS of one face corner — the
/// side-u / side-v / diagonal / interior quadrants of a
/// [`PROBE_REACH`]-sized volume around the corner `(su, sv)` on the face
/// fronted by world voxel `f`, lifted [`PROBE_LIFT`] off the face plane
/// into the front region. Each
/// pocket is an AABB `(lo, hi)` in WORLD space, overlap-tested against its
/// ring cell's occupancy — the grid-AO generalization: for an opaque ring
/// cell the whole-cell bit answers; for a box-family cell the pocket must
/// overlap its actual matter. Pockets are VOLUMES, not points: an inset base
/// (the cauldron's) still overlaps the edge-adjacent pockets, so casting is
/// uniform along an edge instead of sparking only at diagonal corners. A
/// fence's centred post overlaps no corner pocket and correctly casts
/// nothing.
///
/// [`PROBE_LIFT`]: super::super::boxset::PROBE_LIFT
/// [`PROBE_REACH`]: super::super::boxset::PROBE_REACH
#[inline]
pub(in crate::mesh) fn corner_cast_probes(
    face: Face,
    f: (i32, i32, i32),
    su: i32,
    sv: i32,
) -> [([f32; 3], [f32; 3]); 4] {
    use super::super::boxset::{PROBE_LIFT, PROBE_REACH};
    let (dx, dy, dz) = face.dir();
    let d = [dx, dy, dz];
    let (ux, uy, uz) = face.ao_u();
    let u = [ux, uy, uz];

    // The corner's world position: on the face plane (the front voxel's
    // boundary toward the cell), at the corner the (su, sv) signs pick.
    let mut corner = [f.0 as f32, f.1 as f32, f.2 as f32];
    for a in 0..3 {
        if d[a] != 0 {
            corner[a] += (d[a] < 0) as u32 as f32;
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
///
/// `probe(cell, lo, hi)` is the sub-cell AO cast query — does the cell's
/// occupancy overlap the cell-local pocket AABB (see [`corner_cast_probes`])?
/// Consulted only for ring cells that are box-shape families or partial
/// slabs, so pure cube/air neighbourhoods pay nothing.
#[allow(clippy::too_many_arguments)]
pub(in crate::mesh) fn cube_face_lighting<B, S, L, K, P>(
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
    probe: &P,
) -> ([u32; 4], [u32; 4], [u32; 4], [f32; 4])
where
    B: Fn(i32, i32, i32) -> Block,
    S: Fn(i32, i32, i32) -> Option<SlabState>,
    L: Fn(i32, i32, i32) -> u8,
    K: Fn(i32, i32, i32) -> u8,
    P: Fn((i32, i32, i32), [f32; 3], [f32; 3]) -> bool,
{
    let (ux, uy, uz) = face.ao_u();
    let (vx, vy, vz) = face.ao_v();

    // Whether the FRONT cell itself holds sub-cell matter: its interior
    // quadrant then joins the corner occlusion (the exposed ring of a face
    // something box-shaped stands on).
    let front_probe = {
        let fb = block_at(fx, fy, fz);
        probe_worthy(fb) || (fb.is_slab() && slab_at(fx, fy, fz).is_some_and(|st| !st.is_full()))
    };

    let mut occ = [[false; 3]; 3];
    let mut probe_cell = [[false; 3]; 3];
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
            // A non-occluding cell that still holds sub-cell matter (a box
            // shape, a partial slab) gets corner-probe casting below.
            probe_cell[ia][ib] = !occ[ia][ib] && (probe_worthy(cell) || slab_state.is_some());
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
        let (mut s1, mut s2, mut c) = (occ[iu][1], occ[1][iv], occ[iu][iv]);
        let mut q_int = false;
        if front_probe
            || (probe_cell[iu][1] && !s1)
            || (probe_cell[1][iv] && !s2)
            || (probe_cell[iu][iv] && !c)
        {
            let pk = corner_cast_probes(face, (fx, fy, fz), su, sv);
            let cell_of = |s_u: i32, s_v: i32| {
                (
                    fx + s_u * ux + s_v * vx,
                    fy + s_u * uy + s_v * vy,
                    fz + s_u * uz + s_v * vz,
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
                let cl = (fx, fy, fz);
                q_int = probe(cl, local(pk[3].0, cl), local(pk[3].1, cl));
            }
        }
        ao[corner] = quad_ao(q_int, s1, s2, c);
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
