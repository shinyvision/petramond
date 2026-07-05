//! Shared sub-cell plane-quad emission for "full block with a piece cut out"
//! shapes (stairs, slabs): the cell-local UV mapping that matches a full cube
//! face, and the once-per-plane bilinear lighting push that keeps coplanar
//! quads seam-free (see WIKI/stairs.md for the invariants this pins).

use crate::atlas::Tile;
use crate::torch::warm_tint;

use super::face::{should_flip, Face};
use super::vertex::{pack_cell_uv, pack_vertex, pack_vertex2, Vertex, UV_MODE_CELL_LOCAL};
use super::UV_MODE_SHIFT;

/// The tile-local UV of a point inside a block cell, per face, matching the
/// orientation a full cube face gets from the shader's `corner_local` (corner
/// 0 -> (0,1), 1 -> (1,1), 2 -> (1,0), 3 -> (0,0) in `Face::quad_box` corner
/// order). Shared by the chunk mesher and the item cube so a cut shape
/// textures identically to the full block it is cut from, everywhere drawn.
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

/// Up to four cell-local quad boxes (min, max) plus their count.
pub(crate) type PlaneQuads = ([([f32; 3], [f32; 3]); 4], usize);

/// One plane's four face-corner lighting samples, in `Face::quad_box` corner
/// order (so corner `i` sits at UV `corner_local(i)`), bilinearly sampled at
/// sub-quad corners. Interpolated integer channels round half-up; coincident
/// corners of coplanar quads sample identical values, so shading is seamless.
pub(super) struct PlaneLight {
    pub(super) ao: [u32; 4],
    pub(super) sky: [u32; 4],
    pub(super) block: [u32; 4],
    pub(super) warm: [f32; 4],
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

/// Push one cell-local quad box's `face` into the buffers with the plane's
/// bilinear lighting and cell-local UVs quantized to the 1/16th grid.
#[allow(clippy::too_many_arguments)]
pub(super) fn push_plane_quad(
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
            packed2: pack_vertex2(block6)
                | pack_cell_uv((u * 16.0).round() as u32, (v * 16.0).round() as u32),
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
    use super::super::face::FACES;
    use super::*;

    /// The cell-local UV mapping must agree with the plain cube face: a
    /// full-cell cut-shape quad textures exactly like a full block. Corner `i`
    /// of `Face::quad_box` over the unit cell carries the shader's
    /// `corner_local` UV (0 -> (0,1), 1 -> (1,1), 2 -> (1,0), 3 -> (0,0)).
    #[test]
    fn cell_uv_matches_full_cube_face_orientation() {
        const CORNER_LOCAL: [[f32; 2]; 4] = [[0.0, 1.0], [1.0, 1.0], [1.0, 0.0], [0.0, 0.0]];
        for face in FACES {
            let corners = face.quad_box([0.0; 3], [1.0; 3]);
            for (i, p) in corners.into_iter().enumerate() {
                assert_eq!(
                    cell_uv(face, p),
                    CORNER_LOCAL[i],
                    "{face:?} corner {i} must match the shader's corner_local"
                );
            }
        }
    }
}
