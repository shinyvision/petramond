use petramond_world::block_state::LogAxis;

pub use petramond_math::face::Face;

/// Explicit tile-local UV for the bark side of a horizontal log. The texture's
    /// vertical axis follows the log axis, matching the default vertical-log mapping
    /// where bark runs along world Y.
    pub fn log_side_cell_uv(face: Face, axis: LogAxis, local: [f32; 3]) -> Option<[f32; 2]> {
        let axis_idx = match axis {
            LogAxis::X => 0,
            LogAxis::Y => return None,
            LogAxis::Z => 2,
        };
        let normal_idx = match face {
            Face::PosX | Face::NegX => 0,
            Face::PosY | Face::NegY => 1,
            Face::PosZ | Face::NegZ => 2,
        };
        if normal_idx == axis_idx {
            return None;
        }
        let cross_idx = 3 - axis_idx - normal_idx;
        Some([local[cross_idx], 1.0 - local[axis_idx]])
    }

/// The two diagonal billboard quads of an X-shaped plant model, filling the cell
/// `[x,x+1] × [y,y+1] × [z,z+1]`. Corner order matches `quad_for` (p0 bottom-left,
/// p1 bottom-right, p2 top-right, p3 top-left) so the shader's `corner_uv` maps the
/// tile upright. Each plane is drawn in both windings by the mesher so the plant is
/// visible from both sides under back-face culling.
pub(super) fn cross_quads(x: f32, y: f32, z: f32, inset: f32) -> [[[f32; 3]; 4]; 2] {
    // `inset` (a parameterized dimension; 0 for the engine cross) pulls both diagonal
    // endpoints in from the cell corners, shrinking the X toward centre.
    let lo = inset;
    let hi = 1.0 - inset;
    [
        // Plane (x+lo,z+lo) -> (x+hi,z+hi).
        [
            [x + lo, y, z + lo],
            [x + hi, y, z + hi],
            [x + hi, y + 1.0, z + hi],
            [x + lo, y + 1.0, z + lo],
        ],
        // Plane (x+lo,z+hi) -> (x+hi,z+lo).
        [
            [x + lo, y, z + hi],
            [x + hi, y, z + lo],
            [x + hi, y + 1.0, z + lo],
            [x + lo, y + 1.0, z + hi],
        ],
    ]
}

/// The four axis-aligned billboard quads of a planted-crop lattice
/// ([`ShapeFamily::Crop`](petramond_world::block::ShapeFamily::Crop)): one pair
/// perpendicular to each horizontal axis, inset
/// [`CROP_PLANE_INSET`](petramond_world::block::CROP_PLANE_INSET) from the cell faces
/// and running edge to edge along their long axis — a `#` from above. Corner
/// order matches [`cross_quads`] (bottom-left, bottom-right, top-right,
/// top-left) so the tile maps upright; the mesher draws each plane in both
/// windings.
pub(super) fn crop_quads(x: f32, y: f32, z: f32, inset: f32, drop: f32) -> [[[f32; 3]; 4]; 4] {
    let a = inset;
    let b = 1.0 - a;
    // Dropped `drop` (engine default 1/16) so the art roots on sunken farmland.
    let y0 = y - drop;
    let y1 = y0 + 1.0;
    [
        // The pair perpendicular to X, spanning the full Z edge.
        [
            [x + a, y0, z],
            [x + a, y0, z + 1.0],
            [x + a, y1, z + 1.0],
            [x + a, y1, z],
        ],
        [
            [x + b, y0, z],
            [x + b, y0, z + 1.0],
            [x + b, y1, z + 1.0],
            [x + b, y1, z],
        ],
        // The pair perpendicular to Z, spanning the full X edge.
        [
            [x, y0, z + a],
            [x + 1.0, y0, z + a],
            [x + 1.0, y1, z + a],
            [x, y1, z + a],
        ],
        [
            [x, y0, z + b],
            [x + 1.0, y0, z + b],
            [x + 1.0, y1, z + b],
            [x, y1, z + b],
        ],
    ]
}

pub(super) const FACES: [Face; 6] = Face::ALL;

/// Per-vertex AO occlusion level: 0 = darkest (corner buried in a
/// crevice), 3 = no occlusion. `side1`/`side2` are the two edge-adjacent
/// neighbours of the corner in the voxel plane just outside the face; `corner`
/// is the diagonal one. Two solid edges bury the corner regardless of the
/// diagonal, so that case is forced to 0 (the well-known special case).
#[cfg(test)]
pub(super) fn vertex_ao(side1: bool, side2: bool, corner: bool) -> u32 {
    quad_ao(false, side1, side2, corner)
}

/// [`vertex_ao`] generalized to all FOUR quadrants around a corner in the
/// front slab, including the INTERIOR one (`q_int` — inside the face's own
/// front cell, toward the face interior). Grid AO could assume the interior
/// quadrant empty (matter in front of a cube face culls the face), but
/// sub-cell matter can stand ON a face it doesn't cull, and the exposed part
/// of that face must darken toward it. Symmetric in the quadrants, so two
/// coplanar faces sharing a corner — each seeing the same four quadrants
/// under different face-relative names — compute the SAME level: no seam at
/// the cell boundary between a shape's supporting face and its neighbours.
/// With `q_int = false` this is byte-identical to classic vertex AO (an
/// opposite-quadrant pair buries the corner; otherwise one level per
/// occupied quadrant).
#[inline]
pub(super) fn quad_ao(q_int: bool, side1: bool, side2: bool, corner: bool) -> u32 {
    if (side1 && side2) || (q_int && corner) {
        0
    } else {
        3u32.saturating_sub(q_int as u32 + side1 as u32 + side2 as u32 + corner as u32)
    }
}

/// Pick the quad's triangulation diagonal. Default splits along corners 0-2;
/// flip to the 1-3 diagonal when 0-2 is the brighter pair, so the seam runs
/// along the darker diagonal and the interpolated AO gradient stays symmetric
/// (the standard voxel-AO anisotropy fix). Strict `>` leaves ties on the default.
#[inline]
pub(super) fn should_flip(ao: [u32; 4]) -> bool {
    ao[0] + ao[2] > ao[1] + ao[3]
}

/// The unit-cell quad: 4 corners CCW as seen from the +axis direction, spanning
/// `[(x,y,z), (x+1,y+1,z+1)]`. A thin wrapper over [`Face::quad_box`].
pub(super) fn quad_for(face: Face, x: f32, y: f32, z: f32) -> [[f32; 3]; 4] {
    face.quad_box([x, y, z], [x + 1.0, y + 1.0, z + 1.0])
}
