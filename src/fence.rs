//! Fence connection shape shared by placement, collision, selection, and
//! meshing. A fence stores NO per-cell state: its shape is a 4-bit mask of
//! horizontal connections, resolved from the current neighbours every time it
//! is queried (like stair corners and panes), so placing or removing a
//! neighbour reshapes the fence through the ordinary neighbourhood remesh with
//! nothing to persist.
//!
//! A fence connects toward a side when the neighbour offers wood-tight backing:
//! another fence (any wood type), a full solid OPAQUE cube (leaves, glass and
//! other transparent blocks never join), or the flat high/back side of a stair.
//! A slab cell joins only as a full stack; single slabs never do. Rendered, a
//! fence is a centre post growing a pair of horizontal rails per connected
//! side; collision/selection use the simpler post + full-height arm runs
//! (pane-style, at the post's thickness).

use crate::block::{Aabb, Block, RenderShape};
use crate::mathh::{IVec3, Vec3, MAX_SELECTION_BOXES};
use crate::pane::{EAST, NORTH, SIDES, SOUTH, WEST};
use crate::stair::StairShape;

/// The post's horizontal extent: `4/16` across, centred in the cell.
pub const POST_LO: f32 = 6.0 / 16.0;
pub const POST_HI: f32 = 10.0 / 16.0;

/// A rail's horizontal cross extent (`3/16` across, centred in the cell — so
/// its bounds sit on half-texels; the cell-local UV rounding keeps the face
/// sampling exact).
pub const RAIL_LO: f32 = 6.5 / 16.0;
pub const RAIL_HI: f32 = 9.5 / 16.0;

/// The top rail sits 2/16 below the cell top; the bottom rail 2/16 above the
/// cell floor. Both are 3/16 thick.
pub const RAIL_TOP_LO: f32 = 11.0 / 16.0;
pub const RAIL_TOP_HI: f32 = 14.0 / 16.0;
pub const RAIL_BOT_LO: f32 = 2.0 / 16.0;
pub const RAIL_BOT_HI: f32 = 5.0 / 16.0;

#[inline]
pub fn is_fence(block: Block) -> bool {
    block.render_shape() == RenderShape::Fence
}

/// Resolve the 4-bit connection mask for a fence at `pos` from its horizontal
/// neighbours. Callers supply the neighbour reads so the same rules serve the
/// world (collision/selection/placement) and the mesher (padded snapshot):
/// `stair_shape` is consulted only for stair neighbours (resolved corner
/// shape), `slab_full` only for slab neighbours.
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

/// Whether a fence connects toward neighbour `nb` in direction `(dx, dz)`
/// (fence → neighbour). The neighbour joins when it backs the rails with a
/// solid full-height face: another fence, an opaque full cube, the flat back
/// side of a stair, or a full slab stack.
fn connects_from(
    nb: Block,
    stair_shape: impl FnOnce() -> StairShape,
    slab_full: impl FnOnce() -> bool,
    (dx, dz): (i32, i32),
) -> bool {
    if is_fence(nb) {
        return true;
    }
    match nb.render_shape() {
        // Wood-tight means opaque: leaves, glass and every other transparent
        // or semi-transparent cube never join — nor do the non-full irregulars
        // (cactus, chest), which are not opaque either.
        RenderShape::Cube => nb.is_solid() && nb.is_opaque(),
        // Same face rule as panes and wall torches: the face the stair turns
        // toward the fence (outward normal `(-dx, -dz)`) must be completely
        // occupied — the flat high/back side joins, the open/stepped sides not.
        RenderShape::Stair => crate::pane::stair_side_face_full(stair_shape(), (-dx, -dz)),
        // Only a stacked/double slab presents a full face; single slabs never join.
        RenderShape::Slab => slab_full(),
        _ => false,
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
/// cell edges and stop at the centre on unconnected sides — the pane box model
/// at the post's thickness, so bodies never slip through the gap between the
/// two rendered rails.
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
                min: [POST_LO, 0.0, POST_LO],
                max: [POST_HI, 1.0, POST_HI],
            },
        );
    }
    if mask & (NORTH | SOUTH) != 0 {
        shape = push(
            shape,
            Aabb {
                min: [POST_LO, 0.0, if mask & NORTH != 0 { 0.0 } else { POST_LO }],
                max: [POST_HI, 1.0, if mask & SOUTH != 0 { 1.0 } else { POST_HI }],
            },
        );
    }
    if mask & (WEST | EAST) != 0 {
        shape = push(
            shape,
            Aabb {
                min: [if mask & WEST != 0 { 0.0 } else { POST_LO }, 0.0, POST_LO],
                max: [if mask & EAST != 0 { 1.0 } else { POST_HI }, 1.0, POST_HI],
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
/// `pane::world_boxes` (a fence has at most 2 runs, under the outline cap).
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

/// The out-of-world fence (inventory icon, held item, dropped stack): two posts
/// at the cell's edges joined by the two rails — a complete fence segment in
/// one cell, drawn with the same cell-local UVs as the placed shape. The posts
/// sit at the edges so the rails get their full visible span and the segment
/// reads as a fence, not a pair of columns.
pub const ITEM_POSTS: [Aabb; 2] = [
    Aabb {
        min: [0.0, 0.0, POST_LO],
        max: [4.0 / 16.0, 1.0, POST_HI],
    },
    Aabb {
        min: [12.0 / 16.0, 0.0, POST_LO],
        max: [1.0, 1.0, POST_HI],
    },
];

/// The item rails bridge the gap between the two posts along X; their ends butt
/// against the post faces, so only the four long faces are ever drawn.
pub const ITEM_RAILS: [Aabb; 2] = [
    Aabb {
        min: [4.0 / 16.0, RAIL_TOP_LO, RAIL_LO],
        max: [12.0 / 16.0, RAIL_TOP_HI, RAIL_HI],
    },
    Aabb {
        min: [4.0 / 16.0, RAIL_BOT_LO, RAIL_LO],
        max: [12.0 / 16.0, RAIL_BOT_HI, RAIL_HI],
    },
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block_state::{StairHalf, StairState};
    use crate::facing::Facing;

    fn mask_with(neighbours: impl Fn(IVec3) -> Block) -> u8 {
        resolved_mask(
            IVec3::ZERO,
            neighbours,
            |_| StairShape::default(),
            |_| false,
        )
    }

    #[test]
    fn lone_fence_is_an_unconnected_post() {
        assert_eq!(mask_with(|_| Block::Air), 0);
        assert_eq!(boxes_for_mask(0).len(), 1);
        let b = boxes_for_mask(0)[0];
        assert_eq!(b.min, [POST_LO, 0.0, POST_LO]);
        assert_eq!(b.max, [POST_HI, 1.0, POST_HI]);
    }

    #[test]
    fn fence_connects_to_fences_and_opaque_cubes_but_not_transparent_ones() {
        let east = IVec3::new(1, 0, 0);
        assert_eq!(
            mask_with(|p| if p == east {
                Block::OakFence
            } else {
                Block::Air
            }),
            EAST,
            "fences join across wood types and their own"
        );
        assert_eq!(
            mask_with(|p| if p == east { Block::Stone } else { Block::Air }),
            EAST
        );
        for transparent in [Block::OakLeaves, Block::Glass, Block::Ice] {
            assert_eq!(
                mask_with(|p| if p == east { transparent } else { Block::Air }),
                0,
                "{transparent:?} is transparent and must not join a fence"
            );
        }
        for non_full in [Block::Cactus, Block::Chest, Block::Fern, Block::Torch] {
            assert_eq!(
                mask_with(|p| if p == east { non_full } else { Block::Air }),
                0,
                "{non_full:?} offers no full solid face and must not join a fence"
            );
        }
    }

    #[test]
    fn stairs_connect_only_on_their_flat_back_side() {
        // Fence at origin, stair to the EAST. The stair's face toward the fence
        // has outward normal (-1, 0): complete only when the stair's high/back
        // half is its west side, i.e. it faces EAST (low side away).
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
                reaches(0.0, POST_LO, 0),
                "west, mask {mask:04b}"
            );
            assert_eq!(
                mask & EAST != 0,
                reaches(POST_HI, 1.0, 0),
                "east, mask {mask:04b}"
            );
            assert_eq!(
                mask & NORTH != 0,
                reaches(0.0, POST_LO, 2),
                "north, mask {mask:04b}"
            );
            assert_eq!(
                mask & SOUTH != 0,
                reaches(POST_HI, 1.0, 2),
                "south, mask {mask:04b}"
            );
            for b in boxes {
                for a in 0..3 {
                    assert!(b.min[a] >= 0.0 && b.max[a] <= 1.0 && b.min[a] < b.max[a]);
                }
            }
        }
    }

    #[test]
    fn item_shape_is_two_posts_bridged_by_the_rails() {
        for post in ITEM_POSTS {
            assert_eq!(post.min[1], 0.0);
            assert_eq!(post.max[1], 1.0);
            assert_eq!(post.min[2], POST_LO);
            assert_eq!(post.max[2], POST_HI);
        }
        let [west_post, east_post] = ITEM_POSTS;
        for rail in ITEM_RAILS {
            // The rails exactly bridge the gap, butting both post faces.
            assert_eq!(rail.min[0], west_post.max[0]);
            assert_eq!(rail.max[0], east_post.min[0]);
            assert!(rail.min[1] > 0.0 && rail.max[1] < 1.0);
            assert!(rail.min[2] >= POST_LO && rail.max[2] <= POST_HI);
        }
    }
}
