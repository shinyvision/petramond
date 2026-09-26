use super::*;
use crate::chunk::Chunk;
use crate::world::test_world::TestWorld;

fn world_with_chunk() -> TestWorld {
    TestWorld::with_chunk(1, Chunk::new(0, 0))
}

#[test]
fn grass_under_solid_dies_to_dirt() {
    let mut w = world_with_chunk();
    let p = IVec3::new(8, 70, 8);
    w.set_block_world(p.x, p.y, p.z, Block::Grass);
    w.set_block_world(p.x, p.y + 1, p.z, Block::Stone);
    GRASS.random_tick(&mut w, p);
    assert_eq!(w.data.block_if_loaded(p.x, p.y, p.z), Some(Block::Dirt));
}

#[test]
fn uncovered_grass_survives() {
    let mut w = world_with_chunk();
    let p = IVec3::new(8, 70, 8);
    w.set_block_world(p.x, p.y, p.z, Block::Grass);
    GRASS.random_tick(&mut w, p);
    assert_eq!(w.data.block_if_loaded(p.x, p.y, p.z), Some(Block::Grass));
}

#[test]
fn grass_under_no_grass_decay_cover_survives() {
    // A solid cover tagged NoGrassDecay (leaves being the canonical carrier) does
    // not smother the grass below: it stays grass instead of dying back to dirt.
    let mut w = world_with_chunk();
    let p = IVec3::new(8, 70, 8);
    w.set_block_world(p.x, p.y, p.z, Block::Grass);
    w.set_block_world(p.x, p.y + 1, p.z, Block::OakLeaves);
    GRASS.random_tick(&mut w, p);
    assert_eq!(w.data.block_if_loaded(p.x, p.y, p.z), Some(Block::Grass));
}

#[test]
fn flooded_grass_dies_to_dirt() {
    // Water directly overhead drowns grass — it reverts to dirt, so the spread
    // can never leave grass sitting under water.
    let mut w = world_with_chunk();
    let p = IVec3::new(8, 70, 8);
    w.set_block_world(p.x, p.y, p.z, Block::Grass);
    w.set_block_world(p.x, p.y + 1, p.z, Block::Water);
    GRASS.random_tick(&mut w, p);
    assert_eq!(w.data.block_if_loaded(p.x, p.y, p.z), Some(Block::Dirt));
}
