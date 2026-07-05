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
//! - texture every corner with the cell-local UV ([`cell_uv`]) carried
//!   explicitly in the vertex ([`UV_MODE_CELL_LOCAL`]), so each face samples
//!   the sub-rectangle of its tile matching its position in the cell — the
//!   underside of a stair shows one continuous tile, not four restarts.

use crate::atlas::Tile;
use crate::block::Block;
use crate::stair::StairShape;
use crate::torch::warm_tint;

use super::builder::cube_face_lighting;
use super::face::{should_flip, Face, FACES};
use super::vertex::{pack_cell_uv, pack_vertex, pack_vertex2, Vertex, UV_MODE_CELL_LOCAL};
use super::UV_MODE_SHIFT;

/// The tile-local UV of a point inside a block cell, per face, matching the
/// orientation a full cube face gets from the shader's `corner_local` (corner
/// 0 -> (0,1), 1 -> (1,1), 2 -> (1,0), 3 -> (0,0) in `Face::quad_box` corner
/// order). Shared by the chunk mesher and the item cube so a stair textures
/// identically to the full block it is cut from, everywhere it is drawn.
#[inline]
pub(crate) fn cell_uv(face: Face, p: [f32; 3]) -> [f32; 2] {
    match face {
        Face::PosX => [1.0 - p[2], 1.0 - p[1]],
        Face::NegX => [p[2], 1.0 - p[1]],
        Face::PosY => [p[0], p[2]],
        Face::NegY => [p[0], 1.0 - p[2]],
        Face::PosZ => [p[0], 1.0 - p[1]],
        Face::NegZ => [1.0 - p[0], 1.0 - p[1]],
    }
}

/// Quantize a cell-local UV pair to the 1/16th grid `pack_cell_uv` carries.
#[inline]
fn cell_uv16(face: Face, p: [f32; 3]) -> u32 {
    let [u, v] = cell_uv(face, p);
    pack_cell_uv((u * 16.0).round() as u32, (v * 16.0).round() as u32)
}

/// Up to four cell-local quad boxes (min, max) plus their count.
pub(crate) type PlaneQuads = ([([f32; 3], [f32; 3]); 4], usize);

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

/// One plane's four face-corner lighting samples, in `Face::quad_box` corner
/// order (so corner `i` sits at UV `corner_local(i)`), bilinearly sampled at
/// sub-quad corners. Interpolated integer channels round half-up; coincident
/// corners of coplanar quads sample identical values, so shading is seamless.
struct PlaneLight {
    ao: [u32; 4],
    sky: [u32; 4],
    block: [u32; 4],
    warm: [f32; 4],
}

impl PlaneLight {
    fn sample(&self, u: f32, v: f32) -> (u32, u32, u32, f32) {
        // Corner UVs: 0=(0,1) 1=(1,1) 2=(1,0) 3=(0,0).
        let w = [(1.0 - u) * v, u * v, u * (1.0 - v), (1.0 - u) * (1.0 - v)];
        let blend = |c: [u32; 4]| -> u32 {
            let f: f32 = c.iter().zip(w).map(|(&x, wi)| x as f32 * wi).sum();
            (f + 0.5) as u32
        };
        let warm: f32 = self.warm.iter().zip(w).map(|(&x, wi)| x * wi).sum();
        (blend(self.ao), blend(self.sky), blend(self.block), warm)
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn emit_stair_block<B, L, K, T>(
    opaque: &mut Vec<Vertex>,
    opaque_idx: &mut Vec<u32>,
    wx: i32,
    wy: i32,
    wz: i32,
    shape: StairShape,
    tiles: [Tile; 3],
    tint_for: &T,
    block_at: &B,
    neighbour_light: &L,
    neighbour_blocklight: &K,
) where
    B: Fn(i32, i32, i32) -> Block,
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
fn emit_face_plane<B, L, K, T>(
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
    neighbour_light: &L,
    neighbour_blocklight: &K,
) where
    B: Fn(i32, i32, i32) -> Block,
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
    if outer && block_at(fx, fy, fz).is_opaque() {
        return;
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

#[allow(clippy::too_many_arguments)]
fn push_plane_quad(
    vbuf: &mut Vec<Vertex>,
    ibuf: &mut Vec<u32>,
    wx: i32,
    wy: i32,
    wz: i32,
    min: [f32; 3],
    max: [f32; 3],
    face: Face,
    tile: Tile,
    tint: [f32; 3],
    light: &PlaneLight,
) {
    let world_min = [wx as f32 + min[0], wy as f32 + min[1], wz as f32 + min[2]];
    let world_max = [wx as f32 + max[0], wy as f32 + max[1], wz as f32 + max[2]];
    let corners = face.quad_box(world_min, world_max);
    let local = face.quad_box(min, max);
    let shade_idx = face.shade_idx();
    let start = vbuf.len() as u32;
    let mut quad_ao = [0u32; 4];
    for (corner, pos) in corners.into_iter().enumerate() {
        let [u, v] = cell_uv(face, local[corner]);
        let (ao, sky6, block6, warm) = light.sample(u, v);
        quad_ao[corner] = ao;
        vbuf.push(Vertex {
            pos,
            tint: if warm == 0.0 {
                tint
            } else {
                warm_tint(tint, warm)
            },
            packed: pack_vertex(
                tile.index() as u32,
                corner as u32,
                shade_idx,
                0,
                false,
                ao,
                sky6,
            ) | (UV_MODE_CELL_LOCAL << UV_MODE_SHIFT),
            packed2: pack_vertex2(block6) | cell_uv16(face, local[corner]),
        });
    }
    let tris: [u32; 6] = if should_flip(quad_ao) {
        [0, 1, 3, 1, 2, 3]
    } else {
        [0, 1, 2, 0, 2, 3]
    };
    ibuf.extend(tris.map(|i| start + i));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The cell-local UV mapping must agree with the plain cube face: a
    /// full-cell stair quad textures exactly like a full block. Corner `i` of
    /// `Face::quad_box` over the unit cell carries the shader's `corner_local`
    /// UV (0 -> (0,1), 1 -> (1,1), 2 -> (1,0), 3 -> (0,0)).
    #[test]
    fn cell_uv_matches_full_cube_face_orientation() {
        const CORNER_LOCAL: [[f32; 2]; 4] = [[0.0, 1.0], [1.0, 1.0], [1.0, 0.0], [0.0, 0.0]];
        for face in FACES {
            let corners = face.quad_box([0.0; 3], [1.0; 3]);
            for (i, p) in corners.into_iter().enumerate() {
                assert_eq!(
                    cell_uv(face, p),
                    CORNER_LOCAL[i],
                    "{face:?} corner {i} must match the full-cube UV convention"
                );
            }
        }
    }
}
