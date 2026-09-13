use petramond::world::World;
use petramond_math::world_pos::WorldPos;
use petramond_world::block::Block;
use petramond_world::chunk::{Chunk, ChunkPos};

use super::footstep_ground;

#[test]
fn a_step_sounds_the_flat_cover_it_presses_but_not_a_plant_it_walks_through() {
    let mut world = World::new(0, 1);
    let mut chunk = Chunk::new(0, 0);
    for (x, cover) in [
        (2, None),
        (6, Some(Block::SnowLayer)),
        (10, Some(Block::ShortGrass)),
    ] {
        chunk.set_block(x, 64, 8, Block::Grass);
        if let Some(cover) = cover {
            chunk.set_block(x, 65, 8, cover);
        }
    }
    world.insert_chunk_for_test(ChunkPos::new(0, 0), chunk);

    let on = |x: f64| footstep_ground(&world, WorldPos::new(x + 0.5, 65.0, 8.5));
    assert_eq!(on(2.0), Some(Block::Grass));
    assert_eq!(on(6.0), Some(Block::SnowLayer));
    assert_eq!(on(10.0), Some(Block::Grass));
}
