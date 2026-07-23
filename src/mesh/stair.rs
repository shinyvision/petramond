//! Stair meshing geometry helpers.
//!
//! The chunk mesher draws a stair as a plain box set (`crate::stair::
//! boxes_for_shape`, the same table collision uses) through the unified
//! [`super::boxset`] emitter — hidden faces, per-plane cube lighting, and
//! crease self-AO all come from there, not from stair-specific code.
//!
//! [`plane_quads`] remains the PURE per-plane quad decomposition of a stair
//! shape at half-cell granularity, consumed by the break-crack overlay and
//! the item cube so their decals/icons trace the stair silhouette without a
//! world context.

use crate::stair::StairShape;

use super::face::Face;
use super::plane::PlaneQuads;

/// The exterior quads of one (face direction, plane) of a stair shape, as
/// cell-local boxes whose `face` side is the quad. `outer` selects the cell
/// boundary plane (faces that front the neighbour cell), `!outer` the cell's
/// mid plane. Exposed half-cells covering an exact rectangle merge into ONE
/// box; L-shaped exposures stay per half-cell so coplanar quads share whole
/// edges (no T-junction cracks) and never overlap.
///
/// Pure geometry, shared by the break-crack overlay and the item cube so the
/// crack decal / icon is built from the quads the stair silhouette exposes.
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
