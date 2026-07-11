//! Stair meshing: a stair cell renders as a full block with a chunk cut out.
//!
//! A stair's faces lie on at most twelve planes: per face direction, the cell
//! boundary plane (outer half-cell layer) and the cell's mid plane (inner
//! layer). Each plane is a 2x2 grid of half-cell quads. Per plane we:
//!
//! - gather the SAME neighbourhood lighting a full cube face on that plane
//!   would get (once, via the builder's `cube_face_lighting`), and bilinearly
//!   interpolate it at every emitted corner, so shading is continuous across
//!   the plane and identical to a full block wherever corners coincide;
//! - merge the exposed grid cells into one quad when they cover an exact
//!   rectangle (a lone stair's underside is ONE full-cell quad), falling back
//!   to per-half-cell quads for L-shaped exposures so coplanar quads always
//!   meet vertex-to-vertex (no T-junction cracks) and never overlap;
//! - texture every corner with the cell-local UV ([`super::plane::cell_uv`])
//!   carried explicitly in the vertex, so each face samples the sub-rectangle
//!   of its tile matching its position in the cell — the underside of a stair
//!   shows one continuous tile, not four restarts.
//!
//! The lighting gather + quad push shared with slabs lives in [`super::plane`].

use crate::atlas::Tile;
use crate::block::Block;
use crate::block_state::SlabState;
use crate::stair::StairShape;

use super::builder::cube_face_lighting;
use super::face::{Face, FACES};
use super::plane::{push_plane_quad, PlaneLight, PlaneQuads};
use super::vertex::Vertex;

/// The exterior quads of one (face direction, plane) of a stair shape, as
/// cell-local boxes whose `face` side is the quad. `outer` selects the cell
/// boundary plane (faces that front the neighbour cell), `!outer` the cell's
/// mid plane. Exposed half-cells covering an exact rectangle merge into ONE
/// box; L-shaped exposures stay per half-cell so coplanar quads share whole
/// edges (no T-junction cracks) and never overlap.
///
/// Pure geometry, shared by the chunk mesher and the break-crack overlay so
/// the crack decal is built from exactly the quads the mesh renders.
pub(crate) fn plane_quads(shape: StairShape, face: Face, outer: bool) -> PlaneQuads {
    let (dx, dy, dz) = face.dir();
    let dir = [dx, dy, dz];
    let axis = dir.iter().position(|&d| d != 0).expect("face dir");
    // The half-cell layer whose faces lie on this plane: the outer layer's
    // faces touch the cell boundary, the inner layer's sit on the mid plane.
    let layer = usize::from((dir[axis] > 0) == outer);

    let mut cells: [([f32; 3], [f32; 3]); 4] = Default::default();
    let mut n = 0;
    let (mut lo, mut hi) = ([usize::MAX; 3], [usize::MIN; 3]);
    for iy in 0..2 {
        for iz in 0..2 {
            for ix in 0..2 {
                let idx = [ix, iy, iz];
                if idx[axis] != layer
                    || !crate::stair::shape_half_cell_occupied(shape, ix, iy, iz)
                    || crate::stair::adjacent_shape_half_cell_occupied(
                        shape,
                        ix,
                        iy,
                        iz,
                        face.dir(),
                    )
                {
                    continue;
                }
                cells[n] = crate::stair::half_cell_bounds(ix, iy, iz);
                n += 1;
                for a in 0..3 {
                    lo[a] = lo[a].min(idx[a]);
                    hi[a] = hi[a].max(idx[a]);
                }
            }
        }
    }
    if n == 0 {
        return (cells, 0);
    }
    let bbox_cells: usize = (0..3).map(|a| hi[a] - lo[a] + 1).product();
    if n == bbox_cells {
        let min = [0, 1, 2].map(|a| lo[a] as f32 * 0.5);
        let max = [0, 1, 2].map(|a| (hi[a] as f32 + 1.0) * 0.5);
        cells[0] = (min, max);
        n = 1;
    }
    (cells, n)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn emit_stair_block<B, S, L, K, T>(
    opaque: &mut Vec<Vertex>,
    opaque_idx: &mut Vec<u32>,
    wx: i32,
    wy: i32,
    wz: i32,
    shape: StairShape,
    tiles: [Tile; 3],
    tint_for: &T,
    block_at: &B,
    slab_at: &S,
    neighbour_light: &L,
    neighbour_blocklight: &K,
) where
    B: Fn(i32, i32, i32) -> Block,
    S: Fn(i32, i32, i32) -> Option<SlabState>,
    L: Fn(i32, i32, i32) -> u8,
    K: Fn(i32, i32, i32) -> u8,
    T: Fn(Tile) -> [f32; 3],
{
    for face in FACES {
        for outer in [true, false] {
            emit_face_plane(
                opaque,
                opaque_idx,
                wx,
                wy,
                wz,
                shape,
                face,
                outer,
                tiles,
                tint_for,
                block_at,
                slab_at,
                neighbour_light,
                neighbour_blocklight,
            );
        }
    }
}

/// Emit one (face direction, plane) of a stair: the boundary plane fronts the
/// neighbour cell (and is culled whole against an opaque neighbour, exactly
/// like a full cube face), the mid plane fronts the stair's own cell.
#[allow(clippy::too_many_arguments)]
fn emit_face_plane<B, S, L, K, T>(
    opaque: &mut Vec<Vertex>,
    opaque_idx: &mut Vec<u32>,
    wx: i32,
    wy: i32,
    wz: i32,
    shape: StairShape,
    face: Face,
    outer: bool,
    tiles: [Tile; 3],
    tint_for: &T,
    block_at: &B,
    slab_at: &S,
    neighbour_light: &L,
    neighbour_blocklight: &K,
) where
    B: Fn(i32, i32, i32) -> Block,
    S: Fn(i32, i32, i32) -> Option<SlabState>,
    L: Fn(i32, i32, i32) -> u8,
    K: Fn(i32, i32, i32) -> u8,
    T: Fn(Tile) -> [f32; 3],
{
    let (quads, n) = plane_quads(shape, face, outer);
    if n == 0 {
        return;
    }

    let (dx, dy, dz) = face.dir();
    let (fx, fy, fz) = if outer {
        (wx + dx, wy + dy, wz + dz)
    } else {
        (wx, wy, wz)
    };
    if outer {
        let nb = block_at(fx, fy, fz);
        // A full slab stack in the neighbour cell hides this boundary plane
        // exactly like an opaque cube would.
        if nb.is_opaque()
            || (nb.is_slab() && slab_at(fx, fy, fz).is_some_and(|s| s.is_full()))
        {
            return;
        }
    }

    // The underside is a closed face: if the cell below the stair is dark,
    // adjacent sky-lit cells must not smooth light onto it.
    let smooth_light = face != Face::NegY;
    let (ao, sky, block, warm) = cube_face_lighting(
        face,
        fx,
        fy,
        fz,
        neighbour_light(fx, fy, fz) as u32,
        neighbour_blocklight(fx, fy, fz) as u32,
        smooth_light,
        block_at,
        slab_at,
        neighbour_light,
        neighbour_blocklight,
    );
    let light = PlaneLight {
        ao,
        sky,
        block,
        warm,
    };

    let tile = match face {
        Face::PosY => tiles[0],
        Face::NegY => tiles[1],
        _ => tiles[2],
    };
    let tint = tint_for(tile);

    for &(min, max) in quads.iter().take(n) {
        push_plane_quad(
            opaque, opaque_idx, wx, wy, wz, min, max, face, tile, tint, &light,
        );
    }
}
