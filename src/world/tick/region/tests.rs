use super::*;
use petramond_world::chunk::ChunkPos;

#[test]
fn region_updates_include_non_air_once_across_sections_and_skip_outside_cells() {
    let mut world = World::new_with_pool(
        0,
        1,
        crate::world::WorldRole::ServerHeadless,
        std::sync::Arc::new(crate::worker::JobPool::inline()),
    );
    for cx in -1..=0 {
        for cz in 0..=1 {
            world.insert_empty_column_for_test(ChunkPos::new(cx, cz));
        }
    }
    let inside = [
        (IVec3::new(-2, -1, 14), Block::Stone),
        (IVec3::new(1, 16, 17), Block::ShortGrass),
        (IVec3::new(-1, 0, 15), Block::PebblesSmall),
        (IVec3::new(0, 15, 16), Block::OakPlanks),
    ];
    let outside = [
        IVec3::new(-3, 0, 15),
        IVec3::new(0, 17, 16),
        IVec3::new(1, 16, 18),
    ];
    for (p, b) in inside.into_iter().chain(outside.map(|p| (p, Block::Stone))) {
        world.set_block_world(p.x, p.y, p.z, b);
    }
    world.sim.update_queue.clear();
    world.sim.update_set.clear();
    let sections = world.sections.len();
    for _ in 0..2 {
        world.queue_non_air_updates_in_box(IVec3::new(-2, -1, 14), IVec3::new(1, 16, 17));
    }
    assert_eq!(world.sections.len(), sections);
    assert_eq!(world.sim.update_queue.len(), inside.len());
    for (p, _) in inside {
        assert!(world.sim.update_set.contains(&p));
    }
    for p in outside {
        assert!(!world.sim.update_set.contains(&p));
    }
    let queued = world.sim.update_queue.clone();
    world.sim.update_queue.clear();
    world.sim.update_set.clear();
    world.queue_non_air_updates_in_box(IVec3::new(-2, -1, 14), IVec3::new(1, 16, 17));
    assert_eq!(
        world.sim.update_queue, queued,
        "dispatch order must be deterministic"
    );
}

#[test]
fn region_updates_clip_to_world_bounds_without_materializing_empty_sections() {
    let mut world = World::new_with_pool(
        0,
        1,
        crate::world::WorldRole::ServerHeadless,
        std::sync::Arc::new(crate::worker::JobPool::inline()),
    );
    let p = IVec3::new(WORLD_BORDER - 1, WORLD_MIN_Y, -WORLD_BORDER);
    let sp = SectionPos::from_world(p.x, p.y, p.z).unwrap();
    world.insert_empty_column_for_test(sp.chunk_pos());
    world.set_block_world(p.x, p.y, p.z, Block::Stone);
    world.sim.update_queue.clear();
    world.sim.update_set.clear();
    let sections = world.sections.len();
    world.queue_non_air_updates_in_box(p - IVec3::ONE, p + IVec3::ONE);
    assert_eq!(
        world.sim.update_queue.iter().copied().collect::<Vec<_>>(),
        vec![p]
    );
    world.queue_non_air_updates_in_box(IVec3::splat(1000), IVec3::splat(1001));
    world.queue_non_air_updates_in_box(IVec3::splat(-1000), IVec3::splat(-999));
    assert_eq!(world.sim.update_queue.len(), 1);
    assert_eq!(world.sections.len(), sections);
}
