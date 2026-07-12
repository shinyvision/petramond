use crate::block_state::LogAxis;

/// Face direction enum. Shared by the chunk mesher (`mesh::builder`) and the
/// dynamic-geometry builder (`render::item_cube`): both pick faces from
/// [`Face::ALL`], shade them via [`Face::shade_idx`], and wind their quads via
/// [`Face::quad_box`], so the two stay byte-identical by construction.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum Face {
    PosX,
    NegX,
    PosY,
    NegY,
    PosZ,
    NegZ,
}

impl Face {
    /// The six faces in canonical order (`PosX, NegX, PosY, NegY, PosZ, NegZ`).
    /// Both mesher loops iterate this, so face/index correspondence is shared.
    pub(crate) const ALL: [Face; 6] = [
        Face::PosX,
        Face::NegX,
        Face::PosY,
        Face::NegY,
        Face::PosZ,
        Face::NegZ,
    ];

    pub(crate) fn dir(self) -> (i32, i32, i32) {
        match self {
            Face::PosX => (1, 0, 0),
            Face::NegX => (-1, 0, 0),
            Face::PosY => (0, 1, 0),
            Face::NegY => (0, -1, 0),
            Face::PosZ => (0, 0, 1),
            Face::NegZ => (0, 0, -1),
        }
    }

    /// Index into `SHADES` (and the shader's mirror) for this face -- packed into
    /// the vertex instead of the raw float.
    pub(crate) fn shade_idx(self) -> u32 {
        match self {
            Face::PosY => 0,
            Face::PosZ | Face::NegZ => 1,
            Face::PosX | Face::NegX => 2,
            Face::NegY => 3,
        }
    }

    /// Face-normal code for `Vertex::packed2` bits 16..19 (see
    /// [`super::vertex::pack_normal_code`]): 1..=6 in `Face::ALL` order. Code 0
    /// is reserved for "neutral" geometry with no meaningful world-space face
    /// direction (cross plants, torches, dynamic props) — the shader falls back
    /// to the classic `SHADES` table for it instead of sun N·L shading.
    pub(crate) fn normal_code(self) -> u32 {
        match self {
            Face::PosX => 1,
            Face::NegX => 2,
            Face::PosY => 3,
            Face::NegY => 4,
            Face::PosZ => 5,
            Face::NegZ => 6,
        }
    }

    /// First tangent axis (unit vector) used when sampling AO occluders -- one of
    /// the two world axes perpendicular to the face normal.
    pub(super) fn ao_u(self) -> (i32, i32, i32) {
        match self {
            Face::PosX | Face::NegX => (0, 1, 0), // Y
            Face::PosY | Face::NegY => (1, 0, 0), // X
            Face::PosZ | Face::NegZ => (1, 0, 0), // X
        }
    }

    /// Second tangent axis (unit vector) for AO occluder sampling.
    pub(super) fn ao_v(self) -> (i32, i32, i32) {
        match self {
            Face::PosX | Face::NegX => (0, 0, 1), // Z
            Face::PosY | Face::NegY => (0, 0, 1), // Z
            Face::PosZ | Face::NegZ => (0, 1, 0), // Y
        }
    }

    /// Per-corner tangent signs `(su, sv)` for the quad corners `p0..p3` in the
    /// same CCW order `quad_box` emits. `su`/`sv` pick which side along `ao_u`/
    /// `ao_v` (relative to the front voxel `block + normal`) each corner's three
    /// AO occluders sit on. Derived from `quad_box` and independently verified
    /// per face; keep in lockstep with `quad_box` if corner order ever changes.
    pub(super) fn ao_signs(self) -> [(i32, i32); 4] {
        match self {
            Face::PosX => [(-1, 1), (-1, -1), (1, -1), (1, 1)],
            Face::NegX => [(-1, -1), (-1, 1), (1, 1), (1, -1)],
            Face::PosY => [(-1, 1), (1, 1), (1, -1), (-1, -1)],
            Face::NegY => [(-1, -1), (1, -1), (1, 1), (-1, 1)],
            Face::PosZ => [(-1, -1), (1, -1), (1, 1), (-1, 1)],
            Face::NegZ => [(1, -1), (-1, -1), (-1, 1), (1, 1)],
        }
    }

    /// The four corners of this face, CCW as seen from outside, spanning the
    /// arbitrary axis-aligned box `[min, max]` (per-axis extents). The unit-cell
    /// `quad_for(face, x, y, z)` is exactly this over `[(x,y,z), (x+1,y+1,z+1)]`;
    /// `render::item_cube` calls it with non-cube boxes (the chest's inset body
    /// and lid). Corner order (p0 bottom-left, p1 bottom-right, p2 top-right, p3
    /// top-left) matches the shader's `corner_uv`, so tiles map upright.
    pub(crate) fn quad_box(self, min: [f32; 3], max: [f32; 3]) -> [[f32; 3]; 4] {
        // Select min/max on each axis: dx/dy/dz of 0 picks min, 1 picks max.
        let p = |dx: usize, dy: usize, dz: usize| {
            [
                if dx == 0 { min[0] } else { max[0] },
                if dy == 0 { min[1] } else { max[1] },
                if dz == 0 { min[2] } else { max[2] },
            ]
        };
        match self {
            Face::PosX => [p(1, 0, 1), p(1, 0, 0), p(1, 1, 0), p(1, 1, 1)],
            Face::NegX => [p(0, 0, 0), p(0, 0, 1), p(0, 1, 1), p(0, 1, 0)],
            Face::PosY => [p(0, 1, 1), p(1, 1, 1), p(1, 1, 0), p(0, 1, 0)],
            Face::NegY => [p(0, 0, 0), p(1, 0, 0), p(1, 0, 1), p(0, 0, 1)],
            Face::PosZ => [p(0, 0, 1), p(1, 0, 1), p(1, 1, 1), p(0, 1, 1)],
            Face::NegZ => [p(1, 0, 0), p(0, 0, 0), p(0, 1, 0), p(1, 1, 0)],
        }
    }

    /// Explicit tile-local UV for the bark side of a horizontal log. The texture's
    /// vertical axis follows the log axis, matching the default vertical-log mapping
    /// where bark runs along world Y.
    pub(crate) fn log_side_cell_uv(self, axis: LogAxis, local: [f32; 3]) -> Option<[f32; 2]> {
        let axis_idx = match axis {
            LogAxis::X => 0,
            LogAxis::Y => return None,
            LogAxis::Z => 2,
        };
        let normal_idx = match self {
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
}

/// The two diagonal billboard quads of an X-shaped plant model, filling the cell
/// `[x,x+1] × [y,y+1] × [z,z+1]`. Corner order matches `quad_for` (p0 bottom-left,
/// p1 bottom-right, p2 top-right, p3 top-left) so the shader's `corner_uv` maps the
/// tile upright. Each plane is drawn in both windings by the mesher so the plant is
/// visible from both sides under back-face culling.
pub(super) fn cross_quads(x: f32, y: f32, z: f32) -> [[[f32; 3]; 4]; 2] {
    [
        // Plane (x,z) -> (x+1,z+1).
        [
            [x, y, z],
            [x + 1.0, y, z + 1.0],
            [x + 1.0, y + 1.0, z + 1.0],
            [x, y + 1.0, z],
        ],
        // Plane (x,z+1) -> (x+1,z).
        [
            [x, y, z + 1.0],
            [x + 1.0, y, z],
            [x + 1.0, y + 1.0, z],
            [x, y + 1.0, z + 1.0],
        ],
    ]
}

/// The four axis-aligned billboard quads of a planted-crop lattice
/// ([`RenderShape::Crop`](crate::block::RenderShape::Crop)): one pair
/// perpendicular to each horizontal axis, inset
/// [`CROP_PLANE_INSET`](crate::block::CROP_PLANE_INSET) from the cell faces
/// and running edge to edge along their long axis — a `#` from above. Corner
/// order matches [`cross_quads`] (bottom-left, bottom-right, top-right,
/// top-left) so the tile maps upright; the mesher draws each plane in both
/// windings.
pub(super) fn crop_quads(x: f32, y: f32, z: f32) -> [[[f32; 3]; 4]; 4] {
    let a = crate::block::CROP_PLANE_INSET;
    let b = 1.0 - a;
    // Dropped 1/16 so the art roots on sunken farmland (see CROP_PLANE_DROP).
    let y0 = y - crate::block::CROP_PLANE_DROP;
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

pub(super) const FACES: [Face; 6] = [
    Face::PosX,
    Face::NegX,
    Face::PosY,
    Face::NegY,
    Face::PosZ,
    Face::NegZ,
];

/// Per-vertex AO occlusion level: 0 = darkest (corner buried in a
/// crevice), 3 = no occlusion. `side1`/`side2` are the two edge-adjacent
/// neighbours of the corner in the voxel plane just outside the face; `corner`
/// is the diagonal one. Two solid edges bury the corner regardless of the
/// diagonal, so that case is forced to 0 (the well-known special case).
#[inline]
pub(super) fn vertex_ao(side1: bool, side2: bool, corner: bool) -> u32 {
    if side1 && side2 {
        0
    } else {
        3 - (side1 as u32 + side2 as u32 + corner as u32)
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

/// A cactus face over the box `[min, max]`: the four side faces are recessed 1/16 of
/// the box inward along their own normal (so the spines, drawn at the texture edges,
/// stand proud of the 14-wide trunk and read against the gap), while the top and bottom
/// stay flush — the box's top cap therefore slightly overhangs the recessed sides, the
/// canonical inset-cactus look. Shared by the chunk mesher and the icon / held / dropped
/// cube (`render::item_cube`) so a cactus reads the same in the world and in the hand.
pub(crate) fn cactus_quad(face: Face, min: [f32; 3], max: [f32; 3]) -> [[f32; 3]; 4] {
    let inset_x = (max[0] - min[0]) / 16.0;
    let inset_z = (max[2] - min[2]) / 16.0;
    let (mut mn, mut mx) = (min, max);
    match face {
        Face::PosX => mx[0] -= inset_x,
        Face::NegX => mn[0] += inset_x,
        Face::PosZ => mx[2] -= inset_z,
        Face::NegZ => mn[2] += inset_z,
        // Top and bottom stay full so the cap overhangs the recessed trunk.
        Face::PosY | Face::NegY => {}
    }
    face.quad_box(mn, mx)
}
