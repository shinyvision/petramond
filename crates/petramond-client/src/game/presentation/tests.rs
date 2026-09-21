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

/// Where a body's shadow lands, by how its feet sit against the ground.
fn shadow_height(world: &World, feet: WorldPos) -> Option<f64> {
    let mut out = Vec::new();
    super::push_entity_shadow(world, &mut out, feet, 0.4);
    out.first().map(|s| s.center.y)
}

#[test]
fn a_shadow_lands_on_the_surface_feet_rest_on_however_they_settle() {
    let mut world = World::new(0, 1);
    let mut chunk = Chunk::new(0, 0);
    for y in 60..=64 {
        chunk.set_block(2, y, 8, Block::Stone);
    }
    chunk.set_block(6, 64, 8, Block::Stone);
    chunk.set_block(6, 65, 8, Block::OakSlab);
    world.insert_chunk_for_test(ChunkPos::new(0, 0), chunk);

    // A landing settles a hair under the block's top as often as on it.
    for feet in [65.0, 65.0 - 1e-6, 65.0 - 0.02] {
        assert_eq!(
            shadow_height(&world, WorldPos::new(2.5, feet, 8.5)),
            Some(65.0),
            "feet at {feet}"
        );
    }
    // Standing on a slab in the feet's own cell.
    assert_eq!(
        shadow_height(&world, WorldPos::new(6.5, 65.5, 8.5)),
        Some(65.5)
    );
    // In the air over it: the same surface, from above.
    assert_eq!(
        shadow_height(&world, WorldPos::new(2.5, 67.3, 8.5)),
        Some(65.0)
    );
}
