use crate::slab::SlabSlot;

pub fn slot_box(slot: SlabSlot) -> ([f32; 3], [f32; 3]) {
    let mut min = [0.0f32; 3];
    let mut max = [1.0f32; 3];
    let axis = match slot.split {
        crate::block_state::SlabSplit::X => 0,
        crate::block_state::SlabSplit::Y => 1,
        crate::block_state::SlabSplit::Z => 2,
    };
    min[axis] = slot.index as f32 * 0.5;
    max[axis] = min[axis] + 0.5;
    (min, max)
}
