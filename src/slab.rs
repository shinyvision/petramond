//! Slab shape, stacking state, and material helpers shared by placement,
//! collision, selection, lighting, meshing, and item drops.

use crate::block::{Aabb, Block};
use crate::block_state::{SlabSplit, SlabState};
use crate::facing::Facing;
use crate::item::{ItemStack, ItemType};
use crate::mathh::{IVec3, Vec3, MAX_SELECTION_BOXES};

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

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum SlabRotation {
    #[default]
    Bottom,
    Top,
    Vertical,
}

impl SlabRotation {
    #[inline]
    pub fn from_index(index: u8) -> Self {
        match index % 3 {
            1 => Self::Top,
            2 => Self::Vertical,
            _ => Self::Bottom,
        }
    }
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
pub fn half_cell_bounds(ix: usize, iy: usize, iz: usize) -> ([f32; 3], [f32; 3]) {
    debug_assert!(ix < 2 && iy < 2 && iz < 2);
    let min = [ix as f32 * H, iy as f32 * H, iz as f32 * H];
    let max = [min[0] + H, min[1] + H, min[2] + H];
    (min, max)
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

pub fn drop_stacks(state: SlabState) -> Vec<ItemStack> {
    let mut stacks: Vec<ItemStack> = Vec::new();
    for (_, block) in layer_slots(state) {
        let item = ItemType::from_block(block);
        if item == ItemType::Air {
            continue;
        }
        if let Some(stack) = stacks.iter_mut().find(|s| s.item == item) {
            stack.count = stack
                .count
                .saturating_add(1)
                .min(stack.item.max_stack_size());
        } else {
            stacks.push(ItemStack::new(item, 1));
        }
    }
    stacks
}

/// A 2x2 mask of open half-face quadrants on a slab boundary.
#[inline]
pub fn light_side_mask(state: SlabState, dx: i32, dy: i32, dz: i32) -> u8 {
    let dir = [dx, dy, dz];
    let Some(axis) = dir.iter().position(|&d| d != 0) else {
        return 0;
    };
    let layer = usize::from(dir[axis] > 0);
    let mut open = 0u8;
    for iy in 0..2 {
        for iz in 0..2 {
            for ix in 0..2 {
                let idx = [ix, iy, iz];
                if idx[axis] == layer && !half_cell_occupied(state, ix, iy, iz) {
                    open |= aperture_bit(axis, ix, iy, iz);
                }
            }
        }
    }
    open
}

#[inline]
fn aperture_bit(axis: usize, ix: usize, iy: usize, iz: usize) -> u8 {
    match axis {
        0 => quadrant_bit(iz, iy),
        1 => quadrant_bit(ix, iz),
        _ => quadrant_bit(ix, iy),
    }
}

#[inline]
fn quadrant_bit(a: usize, b: usize) -> u8 {
    match (a, b) {
        (0, 0) => 0b0001,
        (1, 0) => 0b0010,
        (0, 1) => 0b0100,
        _ => 0b1000,
    }
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

    #[test]
    fn slab_light_masks_match_the_open_half() {
        let bottom = SlabState::single(SlabSplit::Y, 0, Block::DirtSlab);
        assert_eq!(
            light_side_mask(bottom, 0, -1, 0),
            0,
            "bottom slab blocks light from below"
        );
        assert_eq!(
            light_side_mask(bottom, 0, 1, 0),
            0b1111,
            "bottom slab leaves the full top boundary open"
        );
        assert_eq!(
            light_side_mask(bottom, 1, 0, 0),
            0b1100,
            "side boundaries leave only the upper quadrants open"
        );

        let full = SlabState {
            split: SlabSplit::Y,
            layers: [Block::DirtSlab, Block::StoneSlab],
        };
        assert_eq!(light_side_mask(full, 0, 1, 0), 0);
        assert_eq!(light_side_mask(full, 1, 0, 0), 0);
    }
}
