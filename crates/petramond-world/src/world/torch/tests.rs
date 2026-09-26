use super::*;
use crate::block::Block;
use crate::block_state::{SlabSplit, StairHalf, StairState};
use crate::chunk::Chunk;
use crate::facing::Facing;
use crate::world::test_world::TestWorld;

fn world() -> TestWorld {
    TestWorld::with_chunk(0, Chunk::new(0, 0))
}

#[test]
fn stair_flat_back_supports_a_wall_torch() {
    let mut w = world();
    let stair = IVec3::new(8, 64, 8);
    assert!(w.place_stair(
        stair,
        Block::OakStairs,
        StairState::new(Facing::East, StairHalf::Bottom)
    ));

    let torch = stair - IVec3::new(1, 0, 0);
    assert!(
        w.data.torch_supported_at(torch, TorchPlacement::West),
        "the full-height back face of a stair should hold a wall torch"
    );
}

#[test]
fn single_slab_side_does_not_support_a_wall_torch() {
    let mut w = world();
    let slab = IVec3::new(8, 64, 8);
    assert!(w.place_slab_layer(
        slab,
        Block::DirtSlab,
        crate::slab::SlabSlot {
            split: SlabSplit::Y,
            index: 0,
        }
    ));

    let torch = slab + IVec3::new(1, 0, 0);
    assert!(
        !w.data.torch_supported_at(torch, TorchPlacement::East),
        "a single slab side is not a complete wall face"
    );
}

#[test]
fn stair_open_side_does_not_support_a_wall_torch() {
    let mut w = world();
    let stair = IVec3::new(8, 64, 8);
    assert!(w.place_stair(
        stair,
        Block::OakStairs,
        StairState::new(Facing::East, StairHalf::Bottom)
    ));

    let torch = stair + IVec3::new(1, 0, 0);
    assert!(
        !w.data.torch_supported_at(torch, TorchPlacement::East),
        "the open side of a stair is not a complete wall face"
    );
}

#[test]
fn fence_post_top_supports_a_floor_torch_but_its_sides_hold_no_wall_torch() {
    let mut w = world();
    w.set_block_world(8, 64, 8, Block::OakFence);

    let floor_torch = IVec3::new(8, 65, 8);
    assert!(
        w.data
            .torch_supported_at(floor_torch, TorchPlacement::Floor),
        "a fence's post top should hold a floor torch"
    );

    let fence = IVec3::new(8, 64, 8);
    for (torch, placement) in [
        (fence + IVec3::new(1, 0, 0), TorchPlacement::East),
        (fence + IVec3::new(-1, 0, 0), TorchPlacement::West),
        (fence + IVec3::new(0, 0, 1), TorchPlacement::South),
        (fence + IVec3::new(0, 0, -1), TorchPlacement::North),
    ] {
        assert!(
            !w.data.torch_supported_at(torch, placement),
            "{placement:?} must not mount on a fence side"
        );
    }
}

#[test]
fn full_slab_stacks_support_torches_like_full_blocks() {
    let mut w = world();
    let slab = IVec3::new(8, 64, 8);
    for (block, index) in [(Block::DirtSlab, 0), (Block::CobblestoneSlab, 1)] {
        assert!(w.place_slab_layer(
            slab,
            block,
            crate::slab::SlabSlot {
                split: SlabSplit::Y,
                index,
            }
        ));
    }

    for (torch, placement) in [
        (slab + IVec3::new(0, 1, 0), TorchPlacement::Floor),
        (slab + IVec3::new(1, 0, 0), TorchPlacement::East),
        (slab + IVec3::new(-1, 0, 0), TorchPlacement::West),
        (slab + IVec3::new(0, 0, 1), TorchPlacement::South),
        (slab + IVec3::new(0, 0, -1), TorchPlacement::North),
    ] {
        assert!(
            w.data.torch_supported_at(torch, placement),
            "{placement:?} should be supported by a full slab stack"
        );
    }
}
