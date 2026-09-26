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
fn pane_connects_to_full_cubes_and_panes_but_not_tagged_irregulars() {
    let mut w = world();
    let p = IVec3::new(8, 64, 8);
    // The probe shape is REAL: masks are refined per-cell state now, so
    // the cell must hold the block whose state the cascade maintains.
    assert!(w.set_block_world(p.x, p.y, p.z, Block::GlassPane));
    assert_eq!(w.data.pane_mask_at(p), 0, "isolated pane is a bare post");

    w.set_block_world(7, 64, 8, Block::Stone);
    w.set_block_world(9, 64, 8, Block::GlassPane);
    assert_eq!(
        w.data.pane_mask_at(p),
        crate::pane::WEST | crate::pane::EAST
    );

    w.set_block_world(8, 64, 7, Block::Chest);
    w.set_block_world(8, 64, 9, Block::Cactus);
    assert_eq!(
        w.data.pane_mask_at(p),
        crate::pane::WEST | crate::pane::EAST,
        "no_pane_connect blocks must not add arms"
    );
}

#[test]
fn pane_connects_to_a_stair_back_but_not_its_open_side() {
    let mut w = world();
    let p = IVec3::new(8, 64, 8);
    // The probe shape is REAL: masks are refined per-cell state now, so
    // the cell must hold the block whose state the cascade maintains.
    assert!(w.set_block_world(p.x, p.y, p.z, Block::GlassPane));
    // Stair east of the pane, facing east: its flat high/back side faces the pane.
    assert!(w.place_stair(
        IVec3::new(9, 64, 8),
        Block::OakStairs,
        StairState::new(Facing::East, StairHalf::Bottom),
    ));
    assert_eq!(w.data.pane_mask_at(p), crate::pane::EAST);

    // Stair west of the pane, also facing east: its open side faces the pane.
    assert!(w.place_stair(
        IVec3::new(7, 64, 8),
        Block::OakStairs,
        StairState::new(Facing::East, StairHalf::Bottom),
    ));
    assert_eq!(w.data.pane_mask_at(p), crate::pane::EAST);
}

#[test]
fn pane_connects_to_a_full_slab_stack_but_not_a_single_slab() {
    let mut w = world();
    let p = IVec3::new(8, 64, 8);
    // The probe shape is REAL: masks are refined per-cell state now, so
    // the cell must hold the block whose state the cascade maintains.
    assert!(w.set_block_world(p.x, p.y, p.z, Block::GlassPane));
    let n = IVec3::new(8, 64, 7);
    let slot = |index| crate::slab::SlabSlot {
        split: SlabSplit::Y,
        index,
    };
    assert!(w.place_slab_layer(n, Block::OakSlab, slot(0)));
    assert_eq!(
        w.data.pane_mask_at(p),
        0,
        "a single slab is not a full face"
    );
    assert!(w.place_slab_layer(n, Block::OakSlab, slot(1)));
    assert_eq!(w.data.pane_mask_at(p), crate::pane::NORTH);
}
