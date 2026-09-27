use crate::block::Aabb;
use crate::facing::Facing;
use crate::tile::Tile;

use crate::block::shape::{ShapeBox, ShapeFace};
use crate::face::Face;

fn buried_face(facing: Facing) -> Face {
    match facing {
        Facing::North => Face::PosZ,
        Facing::South => Face::NegZ,
        Facing::West => Face::PosX,
        Facing::East => Face::NegX,
    }
}

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
        uv_rect: None,
    }); 6];
    faces[buried_face(facing) as usize] = None;
    out.push(ShapeBox {
        aabb: Aabb { min, max },
        faces,
        ..ShapeBox::PLAIN
    });
}
