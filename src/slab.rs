//! Slab shape, stacking state, and material helpers shared by placement,
//! collision, selection, lighting, and meshing.

use crate::block::{Aabb, Block};
use crate::block_state::{SlabSplit, SlabState};
use crate::facing::Facing;
use crate::mathh::IVec3;

const H: f32 = 0.5;
const EMPTY_BOX: Aabb = Aabb {
    min: [0.0, 0.0, 0.0],
    max: [0.0, 0.0, 0.0],
};
const FULL_BOX: Aabb = Aabb {
    min: [0.0, 0.0, 0.0],
    max: [1.0, 1.0, 1.0],
};

/// Every (split, occupancy-mask) shape is a single box: one half-cell for a
/// lone layer, the full cell for a complete stack. The mask-0 entry is never
/// read (`boxes_for_state` returns an empty slice for it).
static SHAPES: [[[Aabb; 1]; 4]; 3] = make_shapes();

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct SlabSlot {
    pub split: SlabSplit,
    pub index: usize,
}

crate::wire_enum::wire_enum! {
    pub enum SlabRotation: u8 {
        Bottom = 0,
        Top = 1,
        Vertical = 2,
    }
    default Bottom with from_index
}

#[inline]
pub fn is_slab(block: Block) -> bool {
    block.is_slab()
}

#[inline]
pub fn default_state(block: Block) -> SlabState {
    SlabState::single(SlabSplit::Y, 0, block)
}

#[inline]
pub fn normalize_state(block: Block, state: SlabState) -> SlabState {
    if state.is_empty() && is_slab(block) {
        default_state(block)
    } else {
        state
    }
}

/// A full stack of the SAME slab material is visually the material's full cube,
/// so the mesher routes it down the ordinary cube path (fast path + greedy merge
/// included). Mixed full stacks keep the per-layer emitter to preserve each
/// layer's texture, but still cull/occlude like a full block.
#[inline]
pub fn is_uniform_full_stack(state: SlabState) -> bool {
    state.is_full() && state.layers[0] == state.layers[1]
}

#[inline]
pub fn state_shape(state: SlabState) -> (SlabSplit, u8) {
    (state.split, state.mask())
}

#[inline]
pub fn boxes_for_state(state: SlabState) -> &'static [Aabb] {
    let (split, mask) = state_shape(state);
    if mask == 0 {
        return &[];
    }
    &SHAPES[split as usize][mask as usize]
}

#[inline]
pub fn default_boxes() -> &'static [Aabb] {
    boxes_for_state(default_state(Block::Dirt))
}

/// Whether the face of `state` with outward normal `dir` is a complete
/// surface: a full stack always, else every half-cell on that face occupied.
/// The slab family's answer to the cross-family `full_face` question.
pub fn face_full(state: SlabState, dir: crate::mathh::IVec3) -> bool {
    if state.is_full() {
        return true;
    }
    let occ = |ix, iy, iz| half_cell_occupied(state, ix, iy, iz);
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
pub fn visual_aabb(state: SlabState) -> Option<([f32; 3], [f32; 3])> {
    let boxes = boxes_for_state(state);
    if boxes.is_empty() {
        return None;
    }
    let mut mn = [f32::INFINITY; 3];
    let mut mx = [f32::NEG_INFINITY; 3];
    for b in boxes {
        for axis in 0..3 {
            mn[axis] = mn[axis].min(b.min[axis]);
            mx[axis] = mx[axis].max(b.max[axis]);
        }
    }
    if mn == [0.0; 3] && mx == [1.0; 3] {
        None
    } else {
        Some((mn, mx))
    }
}

#[inline]
pub fn layer_slots(state: SlabState) -> impl Iterator<Item = (SlabSlot, Block)> {
    [0usize, 1usize].into_iter().filter_map(move |index| {
        state.block_in_slot(index).map(|block| {
            (
                SlabSlot {
                    split: state.split,
                    index,
                },
                block,
            )
        })
    })
}

#[inline]
pub fn half_cell_occupied(state: SlabState, ix: usize, iy: usize, iz: usize) -> bool {
    half_cell_block(state, ix, iy, iz).is_some()
}

#[inline]
pub fn half_cell_block(state: SlabState, ix: usize, iy: usize, iz: usize) -> Option<Block> {
    let slot = match state.split {
        SlabSplit::X => ix,
        SlabSplit::Y => iy,
        SlabSplit::Z => iz,
    };
    state.block_in_slot(slot)
}

#[inline]
pub fn slot_for_rotation(rotation: SlabRotation, normal: IVec3, facing: Facing) -> SlabSlot {
    match rotation {
        SlabRotation::Bottom => SlabSlot {
            split: SlabSplit::Y,
            index: 0,
        },
        SlabRotation::Top => SlabSlot {
            split: SlabSplit::Y,
            index: 1,
        },
        SlabRotation::Vertical => vertical_slot(normal, facing),
    }
}

/// The slot a click stacks into the HIT slab cell, or `None` when the clicked
/// face cannot stack at all. A face stacks only when its normal runs along the
/// candidate slot's split axis — i.e. the player clicked the face fronting the
/// half the layer would fill (the top face of a bottom slab, the mid face of a
/// vertical slab, …). Side clicks never stack; they build into the adjacent
/// cell like any other placement.
#[inline]
pub fn stack_slot(rotation: SlabRotation, normal: IVec3, facing: Facing) -> Option<SlabSlot> {
    let slot = match rotation {
        SlabRotation::Bottom if normal.y > 0 => SlabSlot {
            split: SlabSplit::Y,
            index: 1,
        },
        SlabRotation::Top if normal.y < 0 => SlabSlot {
            split: SlabSplit::Y,
            index: 0,
        },
        SlabRotation::Vertical if normal.x > 0 => SlabSlot {
            split: SlabSplit::X,
            index: 1,
        },
        SlabRotation::Vertical if normal.x < 0 => SlabSlot {
            split: SlabSplit::X,
            index: 0,
        },
        SlabRotation::Vertical if normal.z > 0 => SlabSlot {
            split: SlabSplit::Z,
            index: 1,
        },
        SlabRotation::Vertical if normal.z < 0 => SlabSlot {
            split: SlabSplit::Z,
            index: 0,
        },
        _ => slot_for_rotation(rotation, normal, facing),
    };
    (normal_split_axis(normal) == Some(slot.split)).then_some(slot)
}

#[inline]
fn normal_split_axis(normal: IVec3) -> Option<SlabSplit> {
    if normal.x != 0 {
        Some(SlabSplit::X)
    } else if normal.y != 0 {
        Some(SlabSplit::Y)
    } else if normal.z != 0 {
        Some(SlabSplit::Z)
    } else {
        None
    }
}

#[inline]
fn vertical_slot(normal: IVec3, facing: Facing) -> SlabSlot {
    if normal.x > 0 {
        return SlabSlot {
            split: SlabSplit::X,
            index: 0,
        };
    }
    if normal.x < 0 {
        return SlabSlot {
            split: SlabSplit::X,
            index: 1,
        };
    }
    if normal.z > 0 {
        return SlabSlot {
            split: SlabSplit::Z,
            index: 0,
        };
    }
    if normal.z < 0 {
        return SlabSlot {
            split: SlabSplit::Z,
            index: 1,
        };
    }
    match facing {
        Facing::West => SlabSlot {
            split: SlabSplit::X,
            index: 0,
        },
        Facing::East => SlabSlot {
            split: SlabSplit::X,
            index: 1,
        },
        Facing::North => SlabSlot {
            split: SlabSplit::Z,
            index: 0,
        },
        Facing::South => SlabSlot {
            split: SlabSplit::Z,
            index: 1,
        },
    }
}

#[inline]
pub fn can_add_layer(state: SlabState, slot: SlabSlot) -> bool {
    state.split == slot.split && state.block_in_slot(slot.index).is_none()
}

#[inline]
pub fn add_layer(state: SlabState, slot: SlabSlot, block: Block) -> Option<SlabState> {
    if state.is_empty() {
        return Some(SlabState::single(slot.split, slot.index, block));
    }
    if state.split != slot.split {
        return None;
    }
    state.with_slot(slot.index, block)
}

pub fn representative_block(state: SlabState) -> Block {
    layer_slots(state)
        .map(|(_, block)| block)
        .max_by(|a, b| {
            a.harvest_tier()
                .cmp(&b.harvest_tier())
                .then_with(|| a.hardness().total_cmp(&b.hardness()))
                .then_with(|| a.id().cmp(&b.id()))
        })
        .unwrap_or(Block::Air)
}

const fn make_shapes() -> [[[Aabb; 1]; 4]; 3] {
    [
        make_split_shapes(SlabSplit::X),
        make_split_shapes(SlabSplit::Y),
        make_split_shapes(SlabSplit::Z),
    ]
}

const fn make_split_shapes(split: SlabSplit) -> [[Aabb; 1]; 4] {
    [
        [EMPTY_BOX],
        [slot_box(split, 0)],
        [slot_box(split, 1)],
        [FULL_BOX],
    ]
}

const fn slot_box(split: SlabSplit, slot: usize) -> Aabb {
    match (split, slot) {
        (SlabSplit::X, 0) => Aabb {
            min: [0.0, 0.0, 0.0],
            max: [H, 1.0, 1.0],
        },
        (SlabSplit::X, _) => Aabb {
            min: [H, 0.0, 0.0],
            max: [1.0, 1.0, 1.0],
        },
        (SlabSplit::Y, 0) => Aabb {
            min: [0.0, 0.0, 0.0],
            max: [1.0, H, 1.0],
        },
        (SlabSplit::Y, _) => Aabb {
            min: [0.0, H, 0.0],
            max: [1.0, 1.0, 1.0],
        },
        (SlabSplit::Z, 0) => Aabb {
            min: [0.0, 0.0, 0.0],
            max: [1.0, 1.0, H],
        },
        (SlabSplit::Z, _) => Aabb {
            min: [0.0, 0.0, H],
            max: [1.0, 1.0, 1.0],
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slab_state_boxes_stay_inside_one_cell() {
        for split in [SlabSplit::X, SlabSplit::Y, SlabSplit::Z] {
            for mask in 1..=3 {
                let state = SlabState {
                    split,
                    layers: [
                        if mask & 1 != 0 {
                            Block::DirtSlab
                        } else {
                            Block::Air
                        },
                        if mask & 2 != 0 {
                            Block::StoneSlab
                        } else {
                            Block::Air
                        },
                    ],
                };
                for b in boxes_for_state(state) {
                    for axis in 0..3 {
                        assert!(b.min[axis] >= 0.0);
                        assert!(b.max[axis] <= 1.0);
                        assert!(b.min[axis] < b.max[axis]);
                    }
                }
            }
        }
    }
}
