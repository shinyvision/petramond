//! Shared connection-shape primitives: the 4-bit horizontal connection mask and
//! the post + full-height-arm box model that fences, panes, and the Layer-2
//! parameterized wall/bar families all build from.
//!
//! A connection shape stores NO per-cell state — its shape is a 4-bit mask of
//! horizontal connections resolved from the current neighbours every time it is
//! queried (like stair corners), so placing or removing a neighbour reshapes it
//! through the ordinary neighbourhood remesh with nothing to persist. Only the
//! post thickness (the box extent) and the per-neighbour connection rule differ
//! between families; both are parameters here, so a family is its dimensions +
//! its `connects` predicate, not a copy of this module.

use crate::block::{Aabb, Block, BlockTag, ConnectionRule, ShapeFamily};
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

/// Resolve the 4-bit connection mask for a connection shape at `pos` from its
/// horizontal neighbours. Callers supply the neighbour reads so the same rules
/// serve the world (collision/selection/placement) and the mesher (padded
/// snapshot); `stair_shape`/`slab_full` are consulted lazily (only for stair /
/// slab neighbours), so an expensive neighbour resolve happens only when the
/// `connects` predicate actually asks. `connects` receives the neighbour block,
/// the outgoing direction `(dx, dz)`, and thunks for that neighbour's resolved
/// stair shape and full-slab flag.
pub fn resolved_mask<B, T, L, C>(
    pos: IVec3,
    mut block_at: B,
    mut stair_shape: T,
    mut slab_full: L,
    connects: C,
) -> u8
where
    B: FnMut(IVec3) -> Block,
    T: FnMut(IVec3) -> StairShape,
    L: FnMut(IVec3) -> bool,
    C: Fn(Block, (i32, i32), &mut dyn FnMut() -> StairShape, &mut dyn FnMut() -> bool) -> bool,
{
    let mut mask = 0;
    for (bit, (dx, dz)) in SIDES {
        let n = pos + IVec3::new(dx, 0, dz);
        let nb = block_at(n);
        let mut st = || stair_shape(n);
        let mut sl = || slab_full(n);
        if connects(nb, (dx, dz), &mut st, &mut sl) {
            mask |= bit;
        }
    }
    mask
}

/// Whether a connection shape of `self_family` under `rule` grows an arm toward
/// neighbour `nb` in outgoing direction `(dx, dz)`. Same-family shapes join
/// under every rule but [`Never`](ConnectionRule::Never); the cube/stair/slab
/// cases follow the rule (opaque vs solid cubes, full-face stairs, full slab
/// stacks). The thunks are evaluated only for stair / slab neighbours. This is
/// the single param-driven predicate the engine fence/pane and every Layer-2
/// wall/bar pass to [`resolved_mask`].
pub fn connects(
    rule: ConnectionRule,
    self_family: ShapeFamily,
    nb: Block,
    (dx, dz): (i32, i32),
    stair_shape: &mut dyn FnMut() -> StairShape,
    slab_full: &mut dyn FnMut() -> bool,
) -> bool {
    if rule == ConnectionRule::Never {
        return false;
    }
    // Same family (any params) joins — a wall to a wall, a fence to a fence.
    if nb.shape_family() == self_family {
        return true;
    }
    match rule {
        ConnectionRule::Never | ConnectionRule::SameOnly => false,
        ConnectionRule::OpaqueOrSame | ConnectionRule::SolidOrSame => match nb.shape_family() {
            ShapeFamily::Cube => {
                // The pane opt-out for cube-row blocks whose real shape is not
                // the full cell (the inset cactus and chest).
                if rule == ConnectionRule::SolidOrSame && nb.has_tag(BlockTag::NO_PANE_CONNECT) {
                    return false;
                }
                if rule == ConnectionRule::OpaqueOrSame {
                    // Wood-tight: opaque full cubes only (leaves/glass never join).
                    nb.is_solid() && nb.is_opaque()
                } else {
                    // Glass-tight: any solid full cube (glass included).
                    nb.is_solid()
                }
            }
            // The face the stair turns toward this shape must be a complete 1×1.
            ShapeFamily::Stair => stair_side_face_full(stair_shape(), (-dx, -dz)),
            // Only a full slab stack presents a complete face.
            ShapeFamily::Slab => slab_full(),
            _ => false,
        },
    }
}

/// Whether the stair side face with outward normal `(nx, nz)` is a complete 1x1
/// square: every half-cell on that side of the resolved shape is occupied. The
/// flat high/back side of a straight stair joins; the open or stepped sides do
/// not. Shared by fence and pane (same face rule as wall torches).
pub(crate) fn stair_side_face_full(shape: StairShape, (nx, nz): (i32, i32)) -> bool {
    let occupied = |ix, iy, iz| crate::stair::shape_half_cell_occupied(shape, ix, iy, iz);
    if nx != 0 {
        let ix = usize::from(nx > 0);
        (0..2).all(|iy| (0..2).all(|iz| occupied(ix, iy, iz)))
    } else {
        let iz = usize::from(nz > 0);
        (0..2).all(|ix| (0..2).all(|iy| occupied(ix, iy, iz)))
    }
}

/// Up to two full-height runs (one per axis) is all a connection shape needs:
/// the centre post, plus one X run and one Z run that extend to their connected
/// cell edges.
pub const MAX_BOXES: usize = 2;

/// A connection shape's resolved boxes for one mask value: the centre post
/// alone, or the axis runs. Built at compile time by [`make_shapes`].
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Shape {
    boxes: [Aabb; MAX_BOXES],
    len: usize,
}

impl Shape {
    #[inline]
    pub fn as_slice(&'static self) -> &'static [Aabb] {
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

/// The 16 collision/selection box sets of a connection shape whose post spans
/// `lo..hi` on both horizontal axes: the centre post alone (mask 0), or up to
/// two full-height runs that extend to the connected cell edges and stop at the
/// centre on unconnected sides — the box model bodies never slip through.
pub const fn make_shapes(lo: f32, hi: f32) -> [Shape; 16] {
    let mut shapes = [EMPTY_SHAPE; 16];
    let mut mask = 0;
    while mask < shapes.len() {
        shapes[mask] = make_shape(mask as u8, lo, hi);
        mask += 1;
    }
    shapes
}

const fn make_shape(mask: u8, lo: f32, hi: f32) -> Shape {
    let mut shape = EMPTY_SHAPE;
    if mask == 0 {
        return push(
            shape,
            Aabb {
                min: [lo, 0.0, lo],
                max: [hi, 1.0, hi],
            },
        );
    }
    if mask & (NORTH | SOUTH) != 0 {
        shape = push(
            shape,
            Aabb {
                min: [lo, 0.0, if mask & NORTH != 0 { 0.0 } else { lo }],
                max: [hi, 1.0, if mask & SOUTH != 0 { 1.0 } else { hi }],
            },
        );
    }
    if mask & (WEST | EAST) != 0 {
        shape = push(
            shape,
            Aabb {
                min: [if mask & WEST != 0 { 0.0 } else { lo }, 0.0, lo],
                max: [if mask & EAST != 0 { 1.0 } else { hi }, 1.0, hi],
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

/// The collision/selection boxes for a connection mask, indexing a table built
/// by [`make_shapes`].
#[inline]
pub fn boxes_for_mask(shapes: &'static [Shape; 16], mask: u8) -> &'static [Aabb] {
    shapes[(mask & 0b1111) as usize].as_slice()
}

/// Cell-local boxes lifted to world space for the selection outline (a
/// connection shape has at most 2 runs, under the outline cap).
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
    use crate::block::Block;
    use crate::block_state::{StairHalf, StairState};
    use crate::facing::Facing;

    /// Resolve a one-neighbour-to-the-east mask under `rule`/`family`.
    fn east_mask(
        rule: ConnectionRule,
        family: ShapeFamily,
        neighbour: impl Fn(IVec3) -> Block,
    ) -> u8 {
        resolved_mask(
            IVec3::ZERO,
            &neighbour,
            |_| StairShape::default(),
            |_| false,
            |nb, dir, st, sl| connects(rule, family, nb, dir, st, sl),
        )
    }

    const EAST_CELL: IVec3 = IVec3 { x: 1, y: 0, z: 0 };

    #[test]
    fn opaque_rule_joins_opaque_cubes_and_same_family_not_transparent() {
        let (r, f) = (ConnectionRule::OpaqueOrSame, ShapeFamily::Fence);
        let east = |b| move |p| if p == EAST_CELL { b } else { Block::Air };
        assert_eq!(east_mask(r, f, east(Block::OakFence)), EAST, "same family");
        assert_eq!(east_mask(r, f, east(Block::Stone)), EAST, "opaque cube");
        for t in [Block::OakLeaves, Block::Glass, Block::Ice] {
            assert_eq!(east_mask(r, f, east(t)), 0, "{t:?} transparent");
        }
        for nf in [Block::Cactus, Block::Chest, Block::Fern, Block::Torch] {
            assert_eq!(east_mask(r, f, east(nf)), 0, "{nf:?} no full opaque face");
        }
    }

    #[test]
    fn solid_rule_joins_glass_but_no_pane_connect_opts_out() {
        let (r, f) = (ConnectionRule::SolidOrSame, ShapeFamily::Pane);
        let east = |b| move |p| if p == EAST_CELL { b } else { Block::Air };
        assert_eq!(east_mask(r, f, east(Block::GlassPane)), EAST, "same family");
        assert_eq!(
            east_mask(r, f, east(Block::Glass)),
            EAST,
            "glass is solid-not-opaque and joins a pane"
        );
        for irregular in [Block::Cactus, Block::Chest] {
            assert_eq!(east_mask(r, f, east(irregular)), 0, "{irregular:?} opts out");
        }
    }

    #[test]
    fn same_only_and_never_rules() {
        let f = ShapeFamily::Fence;
        let east = |b| move |p| if p == EAST_CELL { b } else { Block::Air };
        assert_eq!(east_mask(ConnectionRule::SameOnly, f, east(Block::OakFence)), EAST);
        assert_eq!(east_mask(ConnectionRule::SameOnly, f, east(Block::Stone)), 0);
        assert_eq!(
            east_mask(ConnectionRule::Never, f, east(Block::OakFence)),
            0,
            "Never is a bare post — not even same family joins"
        );
    }

    #[test]
    fn stair_joins_only_on_its_flat_back_side() {
        let mask_for = |facing| {
            resolved_mask(
                IVec3::ZERO,
                |p| if p == EAST_CELL { Block::OakStairs } else { Block::Air },
                |_| crate::stair::shape(StairState::new(facing, StairHalf::Bottom)),
                |_| false,
                |nb, dir, st, sl| {
                    connects(ConnectionRule::OpaqueOrSame, ShapeFamily::Fence, nb, dir, st, sl)
                },
            )
        };
        assert_eq!(mask_for(Facing::East), EAST, "flat back side joins");
        assert_eq!(mask_for(Facing::West), 0, "open front side does not");
        assert_eq!(mask_for(Facing::North), 0, "stepped side does not");
    }

    #[test]
    fn boxes_span_to_connected_edges_only() {
        static SHAPES: [Shape; 16] = make_shapes(6.0 / 16.0, 10.0 / 16.0);
        for mask in 0..16u8 {
            let boxes = boxes_for_mask(&SHAPES, mask);
            assert!(!boxes.is_empty());
            let reaches = |lo: f32, hi: f32, axis: usize| {
                boxes
                    .iter()
                    .any(|b| b.min[axis] <= lo + 1e-6 && b.max[axis] >= hi - 1e-6)
            };
            assert_eq!(mask & WEST != 0, reaches(0.0, 6.0 / 16.0, 0), "west {mask:04b}");
            assert_eq!(mask & EAST != 0, reaches(10.0 / 16.0, 1.0, 0), "east {mask:04b}");
            assert_eq!(mask & NORTH != 0, reaches(0.0, 6.0 / 16.0, 2), "north {mask:04b}");
            assert_eq!(mask & SOUTH != 0, reaches(10.0 / 16.0, 1.0, 2), "south {mask:04b}");
            for b in boxes {
                for a in 0..3 {
                    assert!(b.min[a] >= 0.0 && b.max[a] <= 1.0 && b.min[a] < b.max[a]);
                }
            }
        }
    }
}
