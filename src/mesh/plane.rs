//! Shared plane primitives for sub-cell shapes: the cell-local UV mapping
//! that matches a full cube face, and the per-plane four-corner light field
//! ([`PlaneLight`]) the unified box-set emitter ([`super::boxset`]) samples
//! bilinearly so coplanar quads stay seam-free.

use super::face::Face;
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
    pub(super) fn sample(&self, u: f32, v: f32) -> (u32, u32, u32, f32) {
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
