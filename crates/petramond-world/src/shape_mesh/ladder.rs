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

use crate::tile::Tile;
use crate::block::Aabb;
use crate::facing::Facing;

use crate::block::shape::{ShapeBox, ShapeFace};
use crate::face::Face;

/// The face buried in the supporting wall for a ladder facing `facing`.
fn buried_face(facing: Facing) -> Face {
    match facing {
        Facing::North => Face::PosZ,
        Facing::South => Face::NegZ,
        Facing::West => Face::PosX,
        Facing::East => Face::NegX,
    }
}

/// The wall panel as a [`ShapeBox`] for the unified emitter: one tile on
/// every face, the wall-side face omitted.
pub fn push_mesh_box(
    out: &mut Vec<ShapeBox>,
    facing: Facing,
    thickness: f32,
    height: f32,
    tile: Tile,
    tint: [f32; 3],
) {
    let (min, max) = crate::ladder::panel_aabb_dim(facing, thickness, height);
    let mut faces = [Some(ShapeFace {
        tile,
        swap_uv: false,
        uv_turns: 0,
        tint,
    }); 6];
    faces[buried_face(facing) as usize] = None;
    out.push(ShapeBox {
        aabb: Aabb { min, max },
        faces,
        ..ShapeBox::PLAIN
    });
}
