//! Stair shape and orientation shared by placement, collision, selection, and meshing.

use crate::block::{Aabb, Block, RenderShape};
use crate::furnace::Facing;
use crate::mathh::{IVec3, Vec3};

pub const MAX_BOXES: usize = 3;
pub type WorldBoxList = ([(Vec3, Vec3); MAX_BOXES], u8);

const H: f32 = 0.5;

const NW: u8 = 0b0001;
const NE: u8 = 0b0010;
const SW: u8 = 0b0100;
const SE: u8 = 0b1000;
const ALL: u8 = NW | NE | SW | SE;

const TOP_NORTH: u8 = SW | SE;
const TOP_SOUTH: u8 = NW | NE;
const TOP_WEST: u8 = NE | SE;
const TOP_EAST: u8 = NW | SW;

const EMPTY_BOX: Aabb = Aabb {
    min: [0.0, 0.0, 0.0],
    max: [0.0, 0.0, 0.0],
};

#[derive(Copy, Clone)]
struct Shape {
    boxes: [Aabb; MAX_BOXES],
    len: usize,
}

impl Shape {
    #[inline]
    fn as_slice(&'static self) -> &'static [Aabb] {
        &self.boxes[..self.len]
    }
}

const EMPTY_SHAPE: Shape = Shape {
    boxes: [EMPTY_BOX; MAX_BOXES],
    len: 0,
};

static SHAPES: [Shape; 16] = make_shapes();

#[inline]
pub fn mask(facing: Facing) -> u8 {
    back_mask(facing)
}

#[inline]
pub fn boxes(facing: Facing) -> &'static [Aabb] {
    boxes_for_top_mask(mask(facing))
}

#[inline]
pub fn resolved_mask<N>(pos: IVec3, facing: Facing, mut neighbour_stair: N) -> u8
where
    N: FnMut(IVec3) -> Option<Facing>,
{
    let high = -facing_offset(facing);
    if let Some(next) = neighbour_stair(pos + high) {
        if perpendicular(facing, next) {
            return back_mask(facing) & back_mask(next);
        }
    }

    if let Some(next) = neighbour_stair(pos - high) {
        if perpendicular(facing, next) {
            return back_mask(facing) | back_mask(next);
        }
    }

    back_mask(facing)
}

#[inline]
pub fn resolved_boxes<N>(pos: IVec3, facing: Facing, neighbour_stair: N) -> &'static [Aabb]
where
    N: FnMut(IVec3) -> Option<Facing>,
{
    boxes_for_top_mask(resolved_mask(pos, facing, neighbour_stair))
}

#[inline]
pub fn world_boxes(origin: IVec3, boxes: &[Aabb]) -> WorldBoxList {
    let base = Vec3::new(origin.x as f32, origin.y as f32, origin.z as f32);
    let mut out = [(Vec3::ZERO, Vec3::ZERO); MAX_BOXES];
    let len = boxes.len().min(MAX_BOXES);
    for (dst, b) in out.iter_mut().zip(boxes.iter()).take(len) {
        *dst = (
            base + Vec3::new(b.min[0], b.min[1], b.min[2]),
            base + Vec3::new(b.max[0], b.max[1], b.max[2]),
        );
    }
    (out, len as u8)
}

#[inline]
pub fn half_cell_occupied(mask: u8, ix: usize, iy: usize, iz: usize) -> bool {
    debug_assert!(ix < 2 && iy < 2 && iz < 2);
    iy == 0 || normalize_top_mask(mask) & quadrant_bit(ix, iz) != 0
}

#[inline]
pub fn adjacent_half_cell_occupied(
    mask: u8,
    ix: usize,
    iy: usize,
    iz: usize,
    dir: (i32, i32, i32),
) -> bool {
    let nx = ix as i32 + dir.0;
    let ny = iy as i32 + dir.1;
    let nz = iz as i32 + dir.2;
    (0..2).contains(&nx)
        && (0..2).contains(&ny)
        && (0..2).contains(&nz)
        && half_cell_occupied(mask, nx as usize, ny as usize, nz as usize)
}

#[inline]
pub fn half_cell_bounds(ix: usize, iy: usize, iz: usize) -> ([f32; 3], [f32; 3]) {
    debug_assert!(ix < 2 && iy < 2 && iz < 2);
    let min = [ix as f32 * H, iy as f32 * H, iz as f32 * H];
    let max = [min[0] + H, min[1] + H, min[2] + H];
    (min, max)
}

#[inline]
pub fn is_stair(block: Block) -> bool {
    block.render_shape() == RenderShape::Stair
}

const fn make_shapes() -> [Shape; 16] {
    let mut shapes = [EMPTY_SHAPE; 16];
    let mut mask = 0;
    while mask < shapes.len() {
        shapes[mask] = make_shape(mask as u8);
        mask += 1;
    }
    shapes
}

const fn make_shape(mask: u8) -> Shape {
    let mask = normalize_top_mask(mask);
    let mut shape = EMPTY_SHAPE;

    if mask == ALL {
        return push(shape, rect(0, 2, 0, 2, 0.0, 1.0));
    }

    shape = push(shape, rect(0, 2, 0, 2, 0.0, H));
    if mask != 0 {
        let line = contained_line(mask);
        if line != 0 {
            shape = push(shape, mask_rect(line, H, 1.0));
            let rest = mask ^ line;
            if rest != 0 {
                shape = push(shape, mask_rect(rest, H, 1.0));
            }
        } else {
            shape = push(shape, mask_rect(mask, H, 1.0));
        }
    }

    shape
}

const fn push(mut shape: Shape, b: Aabb) -> Shape {
    shape.boxes[shape.len] = b;
    shape.len += 1;
    shape
}

const fn normalize_top_mask(mask: u8) -> u8 {
    let mask = mask & ALL;
    if mask == (NW | SE) || mask == (NE | SW) {
        TOP_NORTH
    } else {
        mask
    }
}

const fn contained_line(mask: u8) -> u8 {
    if mask & TOP_NORTH == TOP_NORTH {
        TOP_NORTH
    } else if mask & TOP_SOUTH == TOP_SOUTH {
        TOP_SOUTH
    } else if mask & TOP_WEST == TOP_WEST {
        TOP_WEST
    } else if mask & TOP_EAST == TOP_EAST {
        TOP_EAST
    } else {
        0
    }
}

const fn edge(i: usize) -> f32 {
    if i == 0 {
        0.0
    } else if i == 1 {
        H
    } else {
        1.0
    }
}

const fn rect(x0: usize, x1: usize, z0: usize, z1: usize, y0: f32, y1: f32) -> Aabb {
    Aabb {
        min: [edge(x0), y0, edge(z0)],
        max: [edge(x1), y1, edge(z1)],
    }
}

const fn mask_rect(mask: u8, y0: f32, y1: f32) -> Aabb {
    match mask {
        TOP_NORTH => rect(0, 2, 1, 2, y0, y1),
        TOP_SOUTH => rect(0, 2, 0, 1, y0, y1),
        TOP_WEST => rect(1, 2, 0, 2, y0, y1),
        TOP_EAST => rect(0, 1, 0, 2, y0, y1),
        NW => rect(0, 1, 0, 1, y0, y1),
        NE => rect(1, 2, 0, 1, y0, y1),
        SW => rect(0, 1, 1, 2, y0, y1),
        SE => rect(1, 2, 1, 2, y0, y1),
        _ => EMPTY_BOX,
    }
}

#[inline]
fn quadrant_bit(ix: usize, iz: usize) -> u8 {
    match (ix, iz) {
        (0, 0) => NW,
        (1, 0) => NE,
        (0, 1) => SW,
        _ => SE,
    }
}

#[inline]
fn perpendicular(a: Facing, b: Facing) -> bool {
    matches!(
        (a, b),
        (Facing::North | Facing::South, Facing::East | Facing::West)
            | (Facing::East | Facing::West, Facing::North | Facing::South)
    )
}

#[inline]
fn boxes_for_top_mask(mask: u8) -> &'static [Aabb] {
    SHAPES[normalize_top_mask(mask) as usize].as_slice()
}

#[inline]
fn facing_offset(facing: Facing) -> IVec3 {
    let (x, z) = facing_xz(facing);
    IVec3::new(x, 0, z)
}

#[inline]
fn back_mask(facing: Facing) -> u8 {
    match facing {
        Facing::North => TOP_NORTH,
        Facing::South => TOP_SOUTH,
        Facing::West => TOP_WEST,
        Facing::East => TOP_EAST,
    }
}

/// A 2x2 mask of the open half-face quadrants on a stair boundary. The light flood
/// intersects the two cells' masks; light crosses only where their gaps overlap.
#[inline]
pub fn light_side_mask(facing: Facing, dx: i32, dy: i32, dz: i32) -> u8 {
    if dy < 0 {
        return 0;
    }
    if dy > 0 {
        return ALL ^ back_mask(facing);
    }
    let (fx, fz) = facing_xz(facing);
    if (dx, dz) == (-fx, -fz) {
        return 0;
    }
    if (dx, dz) == (fx, fz) {
        return UPPER_FULL;
    }
    match (dx, dz, facing) {
        (-1, 0, Facing::North) | (1, 0, Facing::North) => UPPER_U0,
        (-1, 0, Facing::South) | (1, 0, Facing::South) => UPPER_U1,
        (0, -1, Facing::West) | (0, 1, Facing::West) => UPPER_U0,
        (0, -1, Facing::East) | (0, 1, Facing::East) => UPPER_U1,
        _ => 0,
    }
}

#[inline]
fn facing_xz(facing: Facing) -> (i32, i32) {
    match facing {
        Facing::North => (0, -1),
        Facing::South => (0, 1),
        Facing::West => (-1, 0),
        Facing::East => (1, 0),
    }
}

const UPPER_FULL: u8 = 0b1100;
const UPPER_U0: u8 = 0b0100;
const UPPER_U1: u8 = 0b1000;

#[cfg(test)]
mod tests {
    use super::*;

    const FACINGS: [Facing; 4] = [Facing::North, Facing::South, Facing::West, Facing::East];

    #[test]
    fn valid_stair_shapes_stay_inside_one_cell() {
        for mask in valid_masks() {
            for b in boxes_for_top_mask(mask) {
                for axis in 0..3 {
                    assert!(b.min[axis] >= 0.0, "min {b:?}");
                    assert!(b.max[axis] <= 1.0, "max {b:?}");
                    assert!(b.min[axis] < b.max[axis], "span {b:?}");
                }
            }
        }
    }

    #[test]
    fn valid_stair_shapes_match_their_high_half_mask() {
        for mask in valid_masks() {
            assert_eq!(box_high_mask(boxes_for_top_mask(mask)), mask);
        }
    }

    fn valid_masks() -> impl Iterator<Item = u8> {
        (0..=ALL).filter(|&mask| normalize_top_mask(mask) == mask)
    }

    fn box_high_mask(shape: &[Aabb]) -> u8 {
        let mut mask = 0;
        for (ix, iz, bit) in [(0, 0, NW), (1, 0, NE), (0, 1, SW), (1, 1, SE)] {
            if boxes_occupy_half_cell(shape, ix, 1, iz) {
                mask |= bit;
            }
        }
        mask
    }

    fn boxes_occupy_half_cell(boxes: &[Aabb], ix: usize, iy: usize, iz: usize) -> bool {
        let p = [
            ix as f32 * H + H * 0.5,
            iy as f32 * H + H * 0.5,
            iz as f32 * H + H * 0.5,
        ];
        boxes.iter().any(|b| {
            p[0] > b.min[0]
                && p[0] < b.max[0]
                && p[1] > b.min[1]
                && p[1] < b.max[1]
                && p[2] > b.min[2]
                && p[2] < b.max[2]
        })
    }

    #[test]
    fn high_side_perpendicular_stairs_make_outside_corners() {
        for facing in FACINGS {
            for next in FACINGS {
                if !perpendicular(facing, next) {
                    continue;
                }

                let pos = IVec3::ZERO;
                let high_pos = pos - facing_offset(facing);
                let mask = resolved_mask(pos, facing, |p| (p == high_pos).then_some(next));
                let expected = back_mask(facing) & back_mask(next);

                assert_eq!(expected.count_ones(), 1);
                assert_eq!(
                    mask, expected,
                    "{facing:?} stair with high-side {next:?} neighbour should be outside"
                );
            }
        }
    }

    #[test]
    fn low_side_perpendicular_stairs_make_inside_corners() {
        for facing in FACINGS {
            for next in FACINGS {
                if !perpendicular(facing, next) {
                    continue;
                }

                let pos = IVec3::ZERO;
                let low_pos = pos + facing_offset(facing);
                let mask = resolved_mask(pos, facing, |p| (p == low_pos).then_some(next));
                let expected = back_mask(facing) | back_mask(next);

                assert_eq!(expected.count_ones(), 3);
                assert_eq!(
                    mask, expected,
                    "{facing:?} stair with low-side {next:?} neighbour should be inside"
                );
            }
        }
    }

    #[test]
    fn same_facing_attachments_do_not_cancel_corners() {
        let pos = IVec3::ZERO;

        let outside_turn = pos - facing_offset(Facing::South);
        let outside_attachment = pos + facing_offset(Facing::East);
        let outside = resolved_mask(pos, Facing::South, |p| match p {
            p if p == outside_turn => Some(Facing::West),
            p if p == outside_attachment => Some(Facing::South),
            _ => None,
        });
        assert_eq!(outside, back_mask(Facing::South) & back_mask(Facing::West));

        let inside_turn = pos + facing_offset(Facing::South);
        let inside_attachment = pos + facing_offset(Facing::West);
        let inside = resolved_mask(pos, Facing::South, |p| match p {
            p if p == inside_turn => Some(Facing::West),
            p if p == inside_attachment => Some(Facing::South),
            _ => None,
        });
        assert_eq!(inside, back_mask(Facing::South) | back_mask(Facing::West));
    }

    #[test]
    fn stair_light_masks_match_the_cut_out_gap() {
        assert_eq!(light_side_mask(Facing::East, -1, 0, 0), 0);
        assert_ne!(light_side_mask(Facing::East, 1, 0, 0), 0);
        assert_ne!(light_side_mask(Facing::East, 0, 1, 0), 0);
        assert_eq!(light_side_mask(Facing::East, 0, -1, 0), 0);

        let north_side_gap = light_side_mask(Facing::North, 1, 0, 0);
        let south_side_gap = light_side_mask(Facing::South, -1, 0, 0);
        assert_eq!(
            north_side_gap & south_side_gap,
            0,
            "two partial side gaps on opposite stair halves should not connect"
        );
    }
}
