use petramond_world::block_state::LogAxis;
use petramond_world::facing::Facing;
use petramond_world::tile::Tile;

use super::super::face::Face;

#[inline]
pub(super) fn facing_face(facing: Facing) -> Face {
    match facing {
        Facing::North => Face::NegZ,
        Facing::South => Face::PosZ,
        Facing::West => Face::NegX,
        Facing::East => Face::PosX,
    }
}

#[inline]
pub(super) fn cube_face_tile(
    is_log: bool,
    face: Face,
    tiles: [Tile; 3],
    front: Option<(Face, Tile)>,
    log_axis: LogAxis,
) -> Tile {
    let [tile_top, tile_bot, tile_side] = tiles;
    if is_log {
        return match (log_axis, face) {
            (LogAxis::X, Face::PosX) | (LogAxis::Y, Face::PosY) | (LogAxis::Z, Face::PosZ) => {
                tile_top
            }
            (LogAxis::X, Face::NegX) | (LogAxis::Y, Face::NegY) | (LogAxis::Z, Face::NegZ) => {
                tile_bot
            }
            _ => tile_side,
        };
    }
    match face {
        Face::PosY => tile_top,
        Face::NegY => tile_bot,
        _ => match front {
            Some((front_face, front_tile)) if face == front_face => front_tile,
            _ => tile_side,
        },
    }
}

#[inline]
pub(super) fn cube_face_uv_turn(
    uv_turns: [u8; 3],
    is_log: bool,
    face: Face,
    front: Option<Face>,
    log_axis: LogAxis,
) -> u32 {
    let [top, bottom, side] = uv_turns.map(u32::from);
    if is_log {
        return match (log_axis, face) {
            (LogAxis::X, Face::PosX) | (LogAxis::Y, Face::PosY) | (LogAxis::Z, Face::PosZ) => top,
            (LogAxis::X, Face::NegX) | (LogAxis::Y, Face::NegY) | (LogAxis::Z, Face::NegZ) => {
                bottom
            }
            _ => side,
        };
    }
    match face {
        Face::PosY => top,
        Face::NegY => bottom,
        _ => match front {
            Some(front_face) if face == front_face => 0,
            _ => side,
        },
    }
}

#[inline]
fn uv_16ths(value: f32) -> u32 {
    crate::vertex::round_i32(value.clamp(0.0, 1.0) * 16.0) as u32
}

#[inline]
pub(super) fn log_side_uvs_apply(axis: LogAxis, face: Face) -> bool {
    let axis_idx = match axis {
        LogAxis::X => 0,
        LogAxis::Y => return false,
        LogAxis::Z => 2,
    };
    let normal_idx = match face {
        Face::PosX | Face::NegX => 0,
        Face::PosY | Face::NegY => 1,
        Face::PosZ | Face::NegZ => 2,
    };
    normal_idx != axis_idx
}

#[inline]
pub(super) fn log_side_cell_uvs(
    axis: LogAxis,
    face: Face,
    corners: [[f32; 3]; 4],
    base: [f32; 3],
) -> Option<[(u32, u32); 4]> {
    let mut uvs = [(0, 0); 4];
    for (i, corner) in corners.into_iter().enumerate() {
        let local = [
            corner[0] - base[0],
            corner[1] - base[1],
            corner[2] - base[2],
        ];
        let [u, v] = crate::face::log_side_cell_uv(face, axis, local)?;
        uvs[i] = (uv_16ths(u), uv_16ths(v));
    }
    Some(uvs)
}

#[inline]
pub(crate) fn face_axes(face: Face) -> (usize, usize, usize) {
    match face {
        Face::PosX | Face::NegX => (0, 2, 1),
        Face::PosY | Face::NegY => (1, 0, 2),
        Face::PosZ | Face::NegZ => (2, 0, 1),
    }
}

#[inline]
pub(super) fn face_index(face: Face) -> usize {
    match face {
        Face::PosX => 0,
        Face::NegX => 1,
        Face::PosY => 2,
        Face::NegY => 3,
        Face::PosZ => 4,
        Face::NegZ => 5,
    }
}
