use super::*;
use crate::mob::SpawnRule;
use petramond_world::chunk::{Chunk, SectionPos};

fn rule() -> SpawnRule {
    SpawnRule {
        biomes: &[Biome::Plains],
        underground: &[],
        y: Some([18, 28]),
        space: None,
        chance: 1.0,
        chances: &[],
        ground: &[Block::Grass],
    }
}

fn cave() -> World {
    let mut world = World::new(0, 1);
    let mut chunk = Chunk::new(0, 0);
    for x in 0..16 {
        for z in 0..16 {
            chunk.set_biome(x, z, Biome::Plains.id());
            chunk.set_block(x, 20, z, Block::Grass);
            chunk.set_block(x, 64, z, Block::Grass);
        }
    }
    world.insert_chunk_for_test(ChunkPos::new(0, 0), chunk);
    world
}

#[test]
fn bounded_search_finds_a_cave_floor_below_the_exposed_surface() {
    let world = cave();
    let rule = rule();
    assert_eq!(
        find(&world, Mob::Owl, &rule, 8, 8, [18, 28]),
        Some(Vec3::new(8.5, 21.0, 8.5))
    );
    assert!(find(&world, Mob::Owl, &rule, 8, 8, [22, 28]).is_none());
}

#[test]
fn cave_sites_require_dry_clearance_and_final_terrain() {
    let mut world = cave();
    let rule = rule();
    world.set_block_world(8, 21, 8, Block::Water);
    assert!(find(&world, Mob::Owl, &rule, 8, 8, [18, 28]).is_none());
    world.set_block_world(8, 21, 8, Block::Stone);
    assert!(find(&world, Mob::Owl, &rule, 8, 8, [18, 28]).is_none());
    world.set_block_world(8, 21, 8, Block::Air);
    world.mark_overlay_in_flight_for_test(SectionPos::new(0, 1, 0));
    assert!(find(&world, Mob::Owl, &rule, 8, 8, [18, 28]).is_none());
}

#[test]
fn territory_constraints_are_independent_of_surface_climate() {
    let world = cave();
    let territory = petramond_worldgen::underground_biomes_at(0, &[[8, 21, 8]])[0];
    let rule = SpawnRule {
        underground: Box::leak(vec![territory.wrapping_add(1)].into_boxed_slice()),
        ..rule()
    };
    assert!(find(&world, Mob::Owl, &rule, 8, 8, [18, 28]).is_none());
    let rule = SpawnRule {
        underground: Box::leak(vec![territory].into_boxed_slice()),
        biomes: &[],
        ..rule
    };
    assert!(find(&world, Mob::Owl, &rule, 8, 8, [18, 28]).is_some());
}

#[test]
fn volume_spawns_need_the_declared_medium_without_inventing_a_floor() {
    let mut world = cave();
    world.set_block_world(8, 24, 8, Block::Water);
    let rule = SpawnRule {
        ground: &[],
        space: Some(&[Block::Water]),
        ..rule()
    };
    assert!(rule.is_spawnable());
    assert_eq!(
        find(&world, Mob::Owl, &rule, 8, 8, [24, 24]),
        Some(Vec3::new(8.5, 24.0, 8.5))
    );
    world.set_block_world(8, 24, 8, Block::Air);
    assert!(find(&world, Mob::Owl, &rule, 8, 8, [24, 24]).is_none());
}
