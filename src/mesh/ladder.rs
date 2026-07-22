//! In-world geometry for a ladder: a 1/16-thick cutout panel flush against the
//! wall it hangs on, baked into the chunk's opaque (cutout) pass.
//!
//! The panel box comes from [`crate::ladder::panel_aabb`] — the same box the
//! raycast target, the selection outline, and the break-crack overlay use — and
//! every face except the one buried in the supporting wall is emitted with
//! cell-local UVs, so the rung art maps 0..1 across the wall face and the thin
//! edges sample their own texel strips (the side of a 1/16 slice cut from the
//! tile). Flat-lit with directional face shade, like the pane: thin geometry
//! reads best without per-corner AO.

use crate::atlas::Tile;
use crate::facing::Facing;
use crate::torch::warm_tint;

use super::face::Face;
use super::vertex::Vertex;

/// Visit every emitted face of a ladder facing `facing`, in emission order —
/// the SINGLE face list shared by the chunk mesher and the break-crack overlay,
/// so the crack is coincident with the mesh. The face flush against the
/// supporting wall is never emitted: the wall (a complete face, by the support
/// rule) covers that plane, and emitting it would z-fight.
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
    let buried = match facing {
        Facing::North => Face::PosZ,
        Facing::South => Face::NegZ,
        Facing::West => Face::PosX,
        Facing::East => Face::NegX,
    };
    for face in Face::ALL {
        if face != buried {
            visit(min, max, face);
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn emit_ladder_block(
    opaque: &mut Vec<Vertex>,
    opaque_idx: &mut Vec<u32>,
    wx: i32,
    wy: i32,
    wz: i32,
    facing: Facing,
    tile: Tile,
    tint: [f32; 3],
    sky6: u32,
    block6: u32,
    warm: f32,
    thickness: f32,
    height: f32,
) {
    let origin = [wx as f32, wy as f32, wz as f32];
    let tint = if warm == 0.0 {
        tint
    } else {
        warm_tint(tint, warm)
    };
    shape_faces_dim(facing, thickness, height, |min, max, face| {
        super::pane::push_face(
            opaque, opaque_idx, origin, min, max, face, tile, false, tint, sky6, block6,
        );
    });
}
