use super::*;
use crate::chunk::Chunk;
use crate::world::test_world::TestWorld;

fn world_with_chunk() -> TestWorld {
    TestWorld::with_chunk(1, Chunk::new(0, 0))
}

#[test]
fn grass_at_radius_is_found() {
    let mut w = world_with_chunk();
    let p = IVec3::new(8, 70, 8);
    w.set_block_world(p.x + SPREAD_RADIUS, p.y, p.z, Block::Grass);
    assert!(grass_within(&w.data, p, SPREAD_RADIUS));
}

#[test]
fn grass_one_past_radius_is_not_found() {
    let mut w = world_with_chunk();
    let p = IVec3::new(8, 70, 8);
    w.set_block_world(p.x + SPREAD_RADIUS + 1, p.y, p.z, Block::Grass);
    assert!(!grass_within(&w.data, p, SPREAD_RADIUS));
}

#[test]
fn diagonal_grass_within_radius_is_found() {
    let mut w = world_with_chunk();
    let p = IVec3::new(8, 70, 8);
    let c = p + IVec3::new(SPREAD_RADIUS, SPREAD_RADIUS, SPREAD_RADIUS);
    w.set_block_world(c.x, c.y, c.z, Block::Grass);
    assert!(grass_within(&w.data, p, SPREAD_RADIUS));
}

#[test]
fn dirt_with_grass_in_range_greens_over() {
    let mut w = world_with_chunk();
    let p = IVec3::new(8, 70, 8);
    w.set_block_world(p.x, p.y, p.z, Block::Dirt);
    w.set_block_world(p.x + 1, p.y, p.z, Block::Grass);
    DIRT.random_tick(&mut w, p);
    assert_eq!(w.data.block_if_loaded(p.x, p.y, p.z), Some(Block::Grass));
}

#[test]
fn dirt_with_no_grass_in_range_stays_dirt() {
    let mut w = world_with_chunk();
    let p = IVec3::new(8, 70, 8);
    w.set_block_world(p.x, p.y, p.z, Block::Dirt);
    DIRT.random_tick(&mut w, p);
    assert_eq!(w.data.block_if_loaded(p.x, p.y, p.z), Some(Block::Dirt));
}

#[test]
fn covered_dirt_does_not_green_even_with_grass_in_range() {
    let mut w = world_with_chunk();
    let p = IVec3::new(8, 70, 8);
    w.set_block_world(p.x, p.y, p.z, Block::Dirt);
    w.set_block_world(p.x + 1, p.y, p.z, Block::Grass);
    w.set_block_world(p.x, p.y + 1, p.z, Block::Stone);
    DIRT.random_tick(&mut w, p);
    assert_eq!(w.data.block_if_loaded(p.x, p.y, p.z), Some(Block::Dirt));
}

#[test]
fn dirt_under_no_grass_decay_cover_greens() {
    let mut w = world_with_chunk();
    let p = IVec3::new(8, 70, 8);
    w.set_block_world(p.x, p.y, p.z, Block::Dirt);
    w.set_block_world(p.x + 1, p.y, p.z, Block::Grass);
    w.set_block_world(p.x, p.y + 1, p.z, Block::OakLeaves);
    DIRT.random_tick(&mut w, p);
    assert_eq!(w.data.block_if_loaded(p.x, p.y, p.z), Some(Block::Grass));
}

#[test]
fn submerged_dirt_does_not_green_even_with_grass_in_range() {
    let mut w = world_with_chunk();
    let p = IVec3::new(8, 70, 8);
    w.set_block_world(p.x, p.y, p.z, Block::Dirt);
    w.set_block_world(p.x + 1, p.y, p.z, Block::Grass);
    w.set_block_world(p.x, p.y + 1, p.z, Block::Water);
    DIRT.random_tick(&mut w, p);
    assert_eq!(w.data.block_if_loaded(p.x, p.y, p.z), Some(Block::Dirt));
}
