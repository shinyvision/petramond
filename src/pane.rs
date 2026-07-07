//! Glass-pane connection shape shared by placement, collision, selection, and
//! meshing. A pane stores NO per-cell state: its shape is a 4-bit mask of
//! horizontal connections, resolved from the current neighbours every time it is
//! queried (like stair corners), so placing or removing a neighbour reshapes the
//! pane through the ordinary neighbourhood remesh with nothing to persist.
//!
//! A pane connects toward a side when the neighbour meets it with glass-tight
//! geometry: another pane, or a block whose facing face is a complete 1x1 square
//! (a full solid cube — furnaces included; the flat high/back side of a stair; a
//! full slab stack). Cube-row blocks whose real shape is NOT the full cell (the
//! inset cactus and chest) opt out via [`BlockTag::NO_PANE_CONNECT`].

use crate::block::{Aabb, Block, BlockTag, RenderShape};
use crate::mathh::{IVec3, Vec3, MAX_SELECTION_BOXES};
use crate::stair::StairShape;

/// Connection-mask bits, one per horizontal side.
pub const WEST: u8 = 0b0001;
pub const EAST: u8 = 0b0010;
pub const NORTH: u8 = 0b0100;
pub const SOUTH: u8 = 0b1000;

/// `(bit, offset)` per side, in mask-bit order.
pub const SIDES: [(u8, (i32, i32)); 4] = [
    (WEST, (-1, 0)),
    (EAST, (1, 0)),
    (NORTH, (0, -1)),
    (SOUTH, (0, 1)),
];

/// The pane slab's thin extent: `2/16` across, centred in the cell.
pub const LO: f32 = 7.0 / 16.0;
pub const HI: f32 = 9.0 / 16.0;

#[inline]
pub fn is_pane(block: Block) -> bool {
    block.render_shape() == RenderShape::Pane
}

/// Resolve the 4-bit connection mask for a pane at `pos` from its horizontal
/// neighbours. Callers supply the neighbour reads so the same rules serve the
/// world (collision/selection/placement) and the mesher (padded snapshot):
/// `stair_shape` is consulted only for stair neighbours (resolved corner shape),
/// `slab_full` only for slab neighbours.
pub fn resolved_mask<B, T, L>(
    pos: IVec3,
    mut block_at: B,
    mut stair_shape: T,
    mut slab_full: L,
) -> u8
where
    B: FnMut(IVec3) -> Block,
    T: FnMut(IVec3) -> StairShape,
    L: FnMut(IVec3) -> bool,
{
    let mut mask = 0;
    for (bit, (dx, dz)) in SIDES {
        let n = pos + IVec3::new(dx, 0, dz);
        let nb = block_at(n);
        if connects_from(nb, || stair_shape(n), || slab_full(n), (dx, dz)) {
            mask |= bit;
        }
    }
    mask
}

/// Whether a pane connects toward neighbour `nb` in direction `(dx, dz)`
/// (pane → neighbour). The neighbour joins when it meets the pane with a
/// glass-tight face: another pane, or a complete 1x1 face toward the pane.
fn connects_from(
    nb: Block,
    stair_shape: impl FnOnce() -> StairShape,
    slab_full: impl FnOnce() -> bool,
    (dx, dz): (i32, i32),
) -> bool {
    if is_pane(nb) {
        return true;
    }
    // The opt-out for cube-row blocks whose real shape is not the full cell.
    if nb.has_tag(BlockTag::NO_PANE_CONNECT) {
        return false;
    }
    match nb.render_shape() {
        RenderShape::Cube => nb.is_solid(),
        // The face the stair turns toward the pane (outward normal `(-dx, -dz)`)
        // must be completely occupied — the flat high/back side of a straight
        // stair joins; the open or stepped sides do not. Same face rule as wall
        // torches (`World::torch_supported_at`).
        RenderShape::Stair => stair_side_face_full(stair_shape(), (-dx, -dz)),
        // Only a stacked/double slab presents a full face; single slabs never join.
        RenderShape::Slab => slab_full(),
        _ => false,
    }
}

/// Whether the stair side face with outward normal `(nx, nz)` is a complete 1x1
/// square: every half-cell on that side of the resolved shape is occupied.
fn stair_side_face_full(shape: StairShape, (nx, nz): (i32, i32)) -> bool {
    let occupied = |ix, iy, iz| crate::stair::shape_half_cell_occupied(shape, ix, iy, iz);
    if nx != 0 {
        let ix = usize::from(nx > 0);
        (0..2).all(|iy| (0..2).all(|iz| occupied(ix, iy, iz)))
    } else {
        let iz = usize::from(nz > 0);
        (0..2).all(|ix| (0..2).all(|iy| occupied(ix, iy, iz)))
    }
}

const MAX_BOXES: usize = 2;

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

const EMPTY_BOX: Aabb = Aabb {
    min: [0.0, 0.0, 0.0],
    max: [0.0, 0.0, 0.0],
};

const EMPTY_SHAPE: Shape = Shape {
    boxes: [EMPTY_BOX; MAX_BOXES],
    len: 0,
};

static SHAPES: [Shape; 16] = make_shapes();

/// The collision/selection boxes for a connection mask: the centre post alone,
/// or up to two full-height runs (one per axis) that extend to the connected
/// cell edges and stop at the centre on unconnected sides.
#[inline]
pub fn boxes_for_mask(mask: u8) -> &'static [Aabb] {
    SHAPES[(mask & 0b1111) as usize].as_slice()
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
    let mut shape = EMPTY_SHAPE;
    if mask == 0 {
        return push(
            shape,
            Aabb {
                min: [LO, 0.0, LO],
                max: [HI, 1.0, HI],
            },
        );
    }
    if mask & (NORTH | SOUTH) != 0 {
        shape = push(
            shape,
            Aabb {
                min: [LO, 0.0, if mask & NORTH != 0 { 0.0 } else { LO }],
                max: [HI, 1.0, if mask & SOUTH != 0 { 1.0 } else { HI }],
            },
        );
    }
    if mask & (WEST | EAST) != 0 {
        shape = push(
            shape,
            Aabb {
                min: [if mask & WEST != 0 { 0.0 } else { LO }, 0.0, LO],
                max: [if mask & EAST != 0 { 1.0 } else { HI }, 1.0, HI],
            },
        );
    }
    shape
}

const fn push(mut shape: Shape, b: Aabb) -> Shape {
    shape.boxes[shape.len] = b;
    shape.len += 1;
    shape
}

/// Cell-local boxes lifted to world space for the selection outline, like
/// `slab::world_boxes` (a pane has at most 2 runs, under the outline cap).
#[inline]
pub fn world_boxes(origin: IVec3, boxes: &[Aabb]) -> ([(Vec3, Vec3); MAX_SELECTION_BOXES], u8) {
    let base = Vec3::new(origin.x as f32, origin.y as f32, origin.z as f32);
    let mut out = [(Vec3::ZERO, Vec3::ZERO); MAX_SELECTION_BOXES];
    let len = boxes.len().min(MAX_SELECTION_BOXES);
    for (dst, b) in out.iter_mut().zip(boxes.iter()) {
        *dst = (
            base + Vec3::new(b.min[0], b.min[1], b.min[2]),
            base + Vec3::new(b.max[0], b.max[1], b.max[2]),
        );
    }
    (out, len as u8)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block_state::{StairHalf, StairState};
    use crate::furnace::Facing;

    fn mask_with(neighbours: impl Fn(IVec3) -> Block) -> u8 {
        resolved_mask(
            IVec3::ZERO,
            neighbours,
            |_| StairShape::default(),
            |_| false,
        )
    }

    #[test]
    fn lone_pane_is_an_unconnected_post() {
        assert_eq!(mask_with(|_| Block::Air), 0);
        assert_eq!(boxes_for_mask(0).len(), 1);
        let b = boxes_for_mask(0)[0];
        assert_eq!(b.min, [LO, 0.0, LO]);
        assert_eq!(b.max, [HI, 1.0, HI]);
    }

    #[test]
    fn panes_connect_to_panes_and_full_cubes_but_not_air_or_plants() {
        let east = IVec3::new(1, 0, 0);
        assert_eq!(
            mask_with(|p| if p == east {
                Block::GlassPane
            } else {
                Block::Air
            }),
            EAST
        );
        assert_eq!(
            mask_with(|p| if p == east { Block::Stone } else { Block::Air }),
            EAST
        );
        assert_eq!(
            mask_with(|p| if p == east { Block::Glass } else { Block::Air }),
            EAST
        );
        assert_eq!(
            mask_with(|p| if p == east { Block::Fern } else { Block::Air }),
            0
        );
    }

    #[test]
    fn tagged_irregular_cubes_do_not_connect() {
        let east = IVec3::new(1, 0, 0);
        for irregular in [Block::Cactus, Block::Chest] {
            assert_eq!(
                mask_with(|p| if p == east { irregular } else { Block::Air }),
                0,
                "{irregular:?} is not a full-cell shape and must not join a pane"
            );
        }
    }

    #[test]
    fn stairs_connect_only_on_their_flat_back_side() {
        // Pane at origin, stair to the EAST. The stair's face toward the pane has
        // outward normal (-1, 0): complete only when the stair's high/back half is
        // its west side, i.e. it faces EAST (low side away from the pane).
        let east = IVec3::new(1, 0, 0);
        let mask_for = |facing| {
            resolved_mask(
                IVec3::ZERO,
                |p| {
                    if p == east {
                        Block::OakStairs
                    } else {
                        Block::Air
                    }
                },
                |_| crate::stair::shape(StairState::new(facing, StairHalf::Bottom)),
                |_| false,
            )
        };
        assert_eq!(mask_for(Facing::East), EAST, "flat back side joins");
        assert_eq!(mask_for(Facing::West), 0, "open front side does not");
        assert_eq!(mask_for(Facing::North), 0, "stepped side does not");
        assert_eq!(mask_for(Facing::South), 0, "stepped side does not");
    }

    #[test]
    fn slabs_connect_only_when_stacked_full() {
        let east = IVec3::new(1, 0, 0);
        let mask_for = |full: bool| {
            resolved_mask(
                IVec3::ZERO,
                |p| {
                    if p == east {
                        Block::OakSlab
                    } else {
                        Block::Air
                    }
                },
                |_| StairShape::default(),
                |_| full,
            )
        };
        assert_eq!(mask_for(true), EAST);
        assert_eq!(mask_for(false), 0);
    }

    #[test]
    fn connection_boxes_span_to_connected_edges_only() {
        for mask in 0..16u8 {
            let boxes = boxes_for_mask(mask);
            assert!(!boxes.is_empty());
            let reaches = |lo: f32, hi: f32, axis: usize| {
                boxes
                    .iter()
                    .any(|b| b.min[axis] <= lo + 1e-6 && b.max[axis] >= hi - 1e-6)
            };
            assert_eq!(
                mask & WEST != 0,
                reaches(0.0, LO, 0),
                "west, mask {mask:04b}"
            );
            assert_eq!(
                mask & EAST != 0,
                reaches(HI, 1.0, 0),
                "east, mask {mask:04b}"
            );
            assert_eq!(
                mask & NORTH != 0,
                reaches(0.0, LO, 2),
                "north, mask {mask:04b}"
            );
            assert_eq!(
                mask & SOUTH != 0,
                reaches(HI, 1.0, 2),
                "south, mask {mask:04b}"
            );
            for b in boxes {
                for a in 0..3 {
                    assert!(b.min[a] >= 0.0 && b.max[a] <= 1.0 && b.min[a] < b.max[a]);
                }
            }
        }
    }
}
