//! Face-direction primitive shared by the chunk mesher (`mesh::builder`) and
//! the dynamic-geometry builder (`render::item_cube`): both pick faces from
//! [`Face::ALL`], shade them via [`Face::shade_idx`], and wind their quads via
//! [`Face::quad_box`], so the two stay byte-identical by construction.

/// Face direction enum. Shared by the chunk mesher (`mesh::builder`) and the
/// dynamic-geometry builder (`render::item_cube`): both pick faces from
/// [`Face::ALL`], shade them via [`Face::shade_idx`], and wind their quads via
/// [`Face::quad_box`], so the two stay byte-identical by construction.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Face {
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
    pub const ALL: [Face; 6] = [
        Face::PosX,
        Face::NegX,
        Face::PosY,
        Face::NegY,
        Face::PosZ,
        Face::NegZ,
    ];

    /// The face's outward unit offset — row `self` of the shared
    /// [`FACE_NEIGHBORS`](crate::math::FACE_NEIGHBORS) table (variant order
    /// matches the table by construction).
    pub fn dir(self) -> (i32, i32, i32) {
        let d = crate::math::FACE_NEIGHBORS[self as usize];
        (d.x, d.y, d.z)
    }

    /// Index into `SHADES` (and the shader's mirror) for this face -- packed into
    /// the vertex instead of the raw float.
    pub fn shade_idx(self) -> u32 {
        match self {
            Face::PosY => 0,
            Face::PosZ | Face::NegZ => 1,
            Face::PosX | Face::NegX => 2,
            Face::NegY => 3,
        }
    }

    /// Face-normal code for `Vertex::packed2` bits 16..19 (see
    /// `super::vertex::pack_normal_code`): 1..=6 in `Face::ALL` order. Code 0
    /// is reserved for "neutral" geometry with no meaningful world-space face
    /// direction (cross plants, torches, dynamic props) — the shader falls back
    /// to the classic `SHADES` table for it instead of sun N·L shading.
    pub fn normal_code(self) -> u32 {
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
    pub fn ao_u(self) -> (i32, i32, i32) {
        match self {
            Face::PosX | Face::NegX => (0, 1, 0), // Y
            Face::PosY | Face::NegY => (1, 0, 0), // X
            Face::PosZ | Face::NegZ => (1, 0, 0), // X
        }
    }

    /// Second tangent axis (unit vector) for AO occluder sampling.
    pub fn ao_v(self) -> (i32, i32, i32) {
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
    pub fn ao_signs(self) -> [(i32, i32); 4] {
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
    pub fn quad_box(self, min: [f32; 3], max: [f32; 3]) -> [[f32; 3]; 4] {
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
}
