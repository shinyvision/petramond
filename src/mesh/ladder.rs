//! In-world geometry for a ladder: a 1/16-thick cutout panel flush against the
//! wall it hangs on, baked into the chunk's opaque (cutout) pass.
//!
//! The panel box comes from [`crate::ladder::panel_aabb`] — the same box the
//! raycast target, the selection outline, and the break-crack overlay use.
//! The chunk mesher wraps it into the unified [`super::boxset`] emitter with
//! the face flush against the supporting wall declared never-emitted: the
//! wall (a complete face, by the support rule) covers that plane, and
//! emitting it would z-fight. Cell-local UVs, so the rung art maps 0..1
//! across the wall face and the thin edges sample their own texel strips.
//! Smooth-lit like every box family (2026-07-23).

use crate::atlas::Tile;
use crate::facing::Facing;

use super::boxset::{FaceStyle, MeshBox};
use super::face::Face;

/// The face buried in the supporting wall for a ladder facing `facing`.
fn buried_face(facing: Facing) -> Face {
    match facing {
        Facing::North => Face::PosZ,
        Facing::South => Face::NegZ,
        Facing::West => Face::PosX,
        Facing::East => Face::NegX,
    }
}

/// Visit every emitted face of a ladder facing `facing`, in emission order —
/// consumed by the break-crack overlay so the crack is coincident with the
/// mesh. The face flush against the supporting wall is never visited.
pub(crate) fn shape_faces(facing: Facing, visit: impl FnMut([f32; 3], [f32; 3], Face)) {
    let (t, h) = (crate::ladder::THICKNESS, 1.0);
    shape_faces_dim(facing, t, h, visit);
}

/// [`shape_faces`] for a Layer-2 wall panel of retuned `thickness`/`height` — the
/// in-world mesh reads the same [`crate::ladder::panel_box`] the collision does.
pub(crate) fn shape_faces_dim(
    facing: Facing,
    thickness: f32,
    height: f32,
    mut visit: impl FnMut([f32; 3], [f32; 3], Face),
) {
    let (min, max) = crate::ladder::panel_aabb_dim(facing, thickness, height);
    let buried = buried_face(facing);
    for face in Face::ALL {
        if face != buried {
            visit(min, max, face);
        }
    }
}

/// The wall panel as a [`MeshBox`] for the unified emitter: one tile on
/// every face, the wall-side face omitted.
pub(super) fn push_mesh_box(
    out: &mut Vec<MeshBox>,
    facing: Facing,
    thickness: f32,
    height: f32,
    tile: Tile,
    tint: [f32; 3],
) {
    let (min, max) = crate::ladder::panel_aabb_dim(facing, thickness, height);
    let mut faces = [Some(FaceStyle {
        tile,
        swap_uv: false,
        tint,
    }); 6];
    faces[buried_face(facing) as usize] = None;
    out.push(MeshBox { min, max, faces });
}
