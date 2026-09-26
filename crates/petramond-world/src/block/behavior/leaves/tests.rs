use super::*;
use crate::block::Block;
use crate::chunk::Chunk;
use crate::world::test_world::TestWorld;

fn world_with_chunk() -> TestWorld {
    TestWorld::with_chunk(1, Chunk::new(0, 0))
}

/// Lay a straight +x run of `len` leaves from `start`, then a log — so the log
/// sits exactly `len` face-steps from `start` through leaves. Stays inside the
/// 16-wide chunk for `start.x + len <= 15`.
fn leaf_run_to_log(w: &mut TestWorld, start: IVec3, len: i32) {
    for i in 0..len {
        w.set_block_world(start.x + i, start.y, start.z, Block::OakLeaves);
    }
    w.set_block_world(start.x + len, start.y, start.z, Block::OakLog);
}

#[test]
fn log_at_max_distance_supports() {
    let mut w = world_with_chunk();
    let p = IVec3::new(2, 70, 8);
    leaf_run_to_log(&mut w, p, MAX_LOG_DISTANCE);
    assert!(leaf_supported(&w.data, p));
}

#[test]
fn log_one_step_past_max_does_not_support() {
    let mut w = world_with_chunk();
    let p = IVec3::new(2, 70, 8);
    leaf_run_to_log(&mut w, p, MAX_LOG_DISTANCE + 1);
    assert!(!leaf_supported(&w.data, p));
}

#[test]
fn adjacent_log_supports() {
    let mut w = world_with_chunk();
    let p = IVec3::new(8, 70, 8);
    w.set_block_world(p.x, p.y, p.z, Block::OakLeaves);
    w.set_block_world(p.x + 1, p.y, p.z, Block::OakLog);
    assert!(leaf_supported(&w.data, p));
}

#[test]
fn isolated_leaf_is_unsupported() {
    let mut w = world_with_chunk();
    let p = IVec3::new(8, 70, 8);
    w.set_block_world(p.x, p.y, p.z, Block::OakLeaves);
    assert!(!leaf_supported(&w.data, p));
}

#[test]
fn a_decaying_leaf_breaks_naturally_so_its_drop_can_roll() {
    // A leaf cut off from wood doesn't vanish silently: it breaks as a NATURAL
    // break, so `Game` plays the burst and rolls the leaf's drop table — the 10%
    // sapling. Here we assert the decay is recorded as a natural break (the drop
    // hand-off), independent of the probabilistic roll itself.
    let mut w = world_with_chunk();
    let p = IVec3::new(8, 70, 8);
    w.set_block_world(p.x, p.y, p.z, Block::OakLeaves); // isolated → unsupported
    LEAVES.random_tick(&mut w, p);
    assert_eq!(
        w.data.block_if_loaded(p.x, p.y, p.z),
        Some(Block::Air),
        "the leaf decayed"
    );
    let breaks = w.take_natural_breaks();
    assert!(
        breaks
            .iter()
            .any(|&(bp, b)| bp == p && b == Block::OakLeaves),
        "a decayed leaf is recorded as a natural break so its sapling drop rolls",
    );
}
