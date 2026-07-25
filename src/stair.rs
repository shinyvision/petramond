//! Stair shape and orientation shared by placement, collision, selection, and meshing.

use crate::block::{Aabb, Block, ShapeFamily};
use crate::block_state::{StairHalf, StairState};
use crate::facing::Facing;
use crate::mathh::IVec3;

pub const MAX_BOXES: usize = 3;

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
static TOP_SHAPES: [Shape; 16] = make_top_shapes();

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct StairShape {
    pub mask: u8,
    pub half: StairHalf,
}

#[inline]
pub fn mask(facing: Facing) -> u8 {
    back_mask(facing)
}

#[inline]
pub fn boxes(facing: Facing) -> &'static [Aabb] {
    boxes_for_top_mask(mask(facing))
}

#[inline]
pub fn shape(state: StairState) -> StairShape {
    StairShape {
        mask: mask(state.facing),
        half: state.half,
    }
}

#[inline]
#[cfg(test)]
pub fn resolved_mask<N>(pos: IVec3, facing: Facing, mut neighbour_stair: N) -> u8
where
    N: FnMut(IVec3) -> Option<Facing>,
{
    let high = -facing.dir();
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
pub fn resolved_shape<N>(pos: IVec3, state: StairState, mut neighbour_stair: N) -> StairShape
where
    N: FnMut(IVec3) -> Option<StairState>,
{
    let facing = state.facing;
    let high = -facing.dir();
    if let Some(next) = neighbour_stair(pos + high) {
        if next.half == state.half && perpendicular(facing, next.facing) {
            return StairShape {
                mask: back_mask(facing) & back_mask(next.facing),
                half: state.half,
            };
        }
    }
    if let Some(next) = neighbour_stair(pos - high) {
        if next.half == state.half && perpendicular(facing, next.facing) {
            return StairShape {
                mask: back_mask(facing) | back_mask(next.facing),
                half: state.half,
            };
        }
    }

    StairShape {
        mask: back_mask(facing),
        half: state.half,
    }
}

#[inline]
pub fn resolved_boxes_state<N>(pos: IVec3, state: StairState, neighbour_stair: N) -> &'static [Aabb]
where
    N: FnMut(IVec3) -> Option<StairState>,
{
    boxes_for_shape(resolved_shape(pos, state, neighbour_stair))
}

impl crate::block::CellView for StairShape {
    fn owns(block: Block) -> bool {
        is_stair(block)
    }
    /// The REFINED corner view of a stair cell's bytes (byte 1, written by
    /// the refine cascade). `0` is the "not yet refined" sentinel — no placed
    /// stair resolves to an empty quadrant mask — and falls back to the
    /// straight shape the placed facing implies, so a state written before
    /// the cascade ran (or a bare fixture) degrades to the unjoined stair,
    /// never garbage.
    fn from_cell(s: crate::block::ShapeState) -> Self {
        let placed = StairState::decode(s.byte(0));
        let mask = s.byte(1);
        if mask == 0 {
            return shape(placed);
        }
        StairShape {
            mask,
            half: placed.half,
        }
    }
}

/// Whether the face of `shape` with outward normal `dir` is a complete
/// surface: every half-cell on that face occupied. The stair family's answer
/// to the cross-family `full_face` question (a fence arm against the flat
/// back, a wall torch on it, a ladder against it).
pub fn face_full(shape: StairShape, dir: crate::mathh::IVec3) -> bool {
    let occ = |ix, iy, iz| shape_half_cell_occupied(shape, ix, iy, iz);
    if dir.x != 0 {
        let ix = usize::from(dir.x > 0);
        (0..2).all(|iy| (0..2).all(|iz| occ(ix, iy, iz)))
    } else if dir.z != 0 {
        let iz = usize::from(dir.z > 0);
        (0..2).all(|ix| (0..2).all(|iy| occ(ix, iy, iz)))
    } else {
        let iy = usize::from(dir.y > 0);
        (0..2).all(|ix| (0..2).all(|iz| occ(ix, iy, iz)))
    }
}

#[inline]
pub fn shape_half_cell_occupied(shape: StairShape, ix: usize, iy: usize, iz: usize) -> bool {
    debug_assert!(ix < 2 && iy < 2 && iz < 2);
    let mask = normalize_top_mask(shape.mask);
    match shape.half {
        StairHalf::Bottom => iy == 0 || mask & quadrant_bit(ix, iz) != 0,
        StairHalf::Top => iy == 1 || mask & quadrant_bit(ix, iz) != 0,
    }
}

#[inline]
pub fn adjacent_shape_half_cell_occupied(
    shape: StairShape,
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
        && shape_half_cell_occupied(shape, nx as usize, ny as usize, nz as usize)
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
    block.shape_family() == ShapeFamily::Stair
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

const fn make_top_shapes() -> [Shape; 16] {
    let mut shapes = [EMPTY_SHAPE; 16];
    let mut mask = 0;
    while mask < shapes.len() {
        shapes[mask] = make_top_shape(mask as u8);
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

const fn make_top_shape(mask: u8) -> Shape {
    let mask = normalize_top_mask(mask);
    let mut shape = EMPTY_SHAPE;

    if mask == ALL {
        return push(shape, rect(0, 2, 0, 2, 0.0, 1.0));
    }

    shape = push(shape, rect(0, 2, 0, 2, H, 1.0));
    if mask != 0 {
        let line = contained_line(mask);
        if line != 0 {
            shape = push(shape, mask_rect(line, 0.0, H));
            let rest = mask ^ line;
            if rest != 0 {
                shape = push(shape, mask_rect(rest, 0.0, H));
            }
        } else {
            shape = push(shape, mask_rect(mask, 0.0, H));
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
pub fn boxes_for_shape(shape: StairShape) -> &'static [Aabb] {
    match shape.half {
        StairHalf::Bottom => SHAPES[normalize_top_mask(shape.mask) as usize].as_slice(),
        StairHalf::Top => TOP_SHAPES[normalize_top_mask(shape.mask) as usize].as_slice(),
    }
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
                let high_pos = pos - facing.dir();
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
                let low_pos = pos + facing.dir();
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

        let outside_turn = pos - Facing::South.dir();
        let outside_attachment = pos + Facing::East.dir();
        let outside = resolved_mask(pos, Facing::South, |p| match p {
            p if p == outside_turn => Some(Facing::West),
            p if p == outside_attachment => Some(Facing::South),
            _ => None,
        });
        assert_eq!(outside, back_mask(Facing::South) & back_mask(Facing::West));

        let inside_turn = pos + Facing::South.dir();
        let inside_attachment = pos + Facing::West.dir();
        let inside = resolved_mask(pos, Facing::South, |p| match p {
            p if p == inside_turn => Some(Facing::West),
            p if p == inside_attachment => Some(Facing::South),
            _ => None,
        });
        assert_eq!(inside, back_mask(Facing::South) | back_mask(Facing::West));
    }
}
