use super::*;
use crate::entity::{Heading, Stuck};
use petramond_math::math::IVec3;
use petramond_math::world_pos::WorldPos;
use petramond_world::chunk::{Chunk, ChunkPos};
use petramond_world::item::{ItemStack, ItemType};

fn world() -> World {
    let mut world = World::new(1, 1);
    world.clear_world();
    world.insert_chunk_for_test(ChunkPos::new(0, 0), Chunk::new(0, 0));
    world
}

fn spawn(world: &mut World, x: f32) -> u64 {
    let mut item = DroppedItem::new(
        WorldPos::new(f64::from(x), 64.5, 4.5),
        ItemStack::new(ItemType::Dirt, 3),
        1,
    );
    item.vel = Vec3::ZERO;
    item.ticks_lived = 42;
    world.spawn_item(item)
}

#[test]
fn nearest_query_is_bounded_and_breaks_ties_by_identity() {
    let mut world = world();
    let left = spawn(&mut world, 3.5);
    let right = spawn(&mut world, 5.5);
    spawn(&mut world, 8.5);
    spawn(&mut world, 32.5);
    let origin = WorldPos::new(4.5, 64.5, 4.5);
    world.item_entities_mut().reverse();
    let ids = |limit| {
        world
            .nearest_item_entities(origin, 64.0, limit)
            .into_iter()
            .map(|item| item.id)
            .collect::<Vec<_>>()
    };
    assert!(ids(0).is_empty());
    assert_eq!(ids(1), vec![left]);
    assert_eq!(ids(2), vec![left, right]);
    assert_eq!(ids(100).len(), 3, "unloaded terrain is omitted");
}

#[test]
fn impulses_compose_without_stealing_reserved_or_lodged_items() {
    let mut world = world();
    let loose = spawn(&mut world, 4.5);
    let reserved = spawn(&mut world, 5.5);
    let lodged = spawn(&mut world, 6.5);
    let frozen = spawn(&mut world, 32.5);
    world
        .dropped_items_mut()
        .get_mut(reserved)
        .unwrap()
        .request_pickup(crate::player::PlayerId(0));
    world.dropped_items_mut().get_mut(lodged).unwrap().motion = Motion::Stuck(Stuck {
        heading: Heading {
            yaw: 0.0,
            pitch: 0.0,
        },
        anchor: IVec3::new(6, 64, 4),
        verified: true,
    });
    let before = world.dropped_items().get(loose).unwrap().clone();
    let delta = Vec3::new(1.0, 2.0, 0.0);
    assert_eq!(
        world.impulse_item_entities(&[
            (loose, delta),
            (reserved, delta),
            (lodged, delta),
            (frozen, delta),
            (u64::MAX, delta),
            (loose, delta)
        ]),
        vec![true, false, false, false, false, true]
    );
    let after = world.dropped_items().get(loose).unwrap();
    assert_eq!(after.vel, delta * 2.0);
    assert_eq!(after.stack, before.stack);
    assert_eq!(after.ticks_lived, before.ticks_lived);
    assert_eq!(after.motion, before.motion);
    assert_eq!(after.pos, before.pos);
    assert_eq!(
        world.impulse_item_entities(&[(loose, Vec3::new(f32::MAX, 0.0, 0.0))]),
        vec![false]
    );
}
