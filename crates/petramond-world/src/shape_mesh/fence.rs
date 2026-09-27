use crate::block::Aabb;
use crate::fence::{rail_cross, RAIL_BOT_HI, RAIL_BOT_LO, RAIL_TOP_HI, RAIL_TOP_LO};
use crate::pane::{EAST, NORTH, SOUTH, WEST};
use crate::tile::Tile;

use crate::block::shape::{ShapeBox, ShapeFace};
use crate::face::Face;

#[derive(Copy, Clone, PartialEq, Eq)]
pub enum FenceBox {
    Post,
    Rail(bool),
}

pub fn shape_boxes(
    post_lo: f32,
    post_hi: f32,
    mask: u8,
    mut visit: impl FnMut([f32; 3], [f32; 3], FenceBox),
) {
    visit(
        [post_lo, 0.0, post_lo],
        [post_hi, 1.0, post_hi],
        FenceBox::Post,
    );

    let (rail_lo, rail_hi) = rail_cross(post_lo, post_hi);
    for (bit, along_x, from, to) in [
        (NORTH, false, 0.0, post_lo),
        (SOUTH, false, post_hi, 1.0),
        (WEST, true, 0.0, post_lo),
        (EAST, true, post_hi, 1.0),
    ] {
        if mask & bit == 0 {
            continue;
        }
        for (y_lo, y_hi) in [(RAIL_BOT_LO, RAIL_BOT_HI), (RAIL_TOP_LO, RAIL_TOP_HI)] {
            let (min, max) = if along_x {
                ([from, y_lo, rail_lo], [to, y_hi, rail_hi])
            } else {
                ([rail_lo, y_lo, from], [rail_hi, y_hi, to])
            };
            visit(min, max, FenceBox::Rail(along_x));
        }
    }
}

#[inline]
fn rail_end(kind: FenceBox, face: Face) -> bool {
    match kind {
        FenceBox::Post => false,
        FenceBox::Rail(along_x) => match face {
            Face::NegX | Face::PosX => along_x,
            Face::NegZ | Face::PosZ => !along_x,
            _ => false,
        },
    }
}

pub fn push_mesh_boxes(
    out: &mut Vec<ShapeBox>,
    post_lo: f32,
    post_hi: f32,
    mask: u8,
    tiles: [Tile; 3],
    tint: [f32; 3],
) {
    shape_boxes(post_lo, post_hi, mask, |min, max, kind| {
        let style = |tile: Tile| {
            Some(ShapeFace {
                tile,
                swap_uv: false,
                uv_turns: 0,
                tint,
                uv_rect: None,
            })
        };
        let mut faces = [style(tiles[2]); 6];
        faces[Face::PosY as usize] = style(tiles[0]);
        faces[Face::NegY as usize] = style(tiles[1]);
        for face in Face::ALL {
            if rail_end(kind, face) {
                faces[face as usize] = None;
            }
        }
        out.push(ShapeBox {
            aabb: Aabb { min, max },
            faces,
            ..ShapeBox::PLAIN
        });
    });
}
