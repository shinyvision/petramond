use super::*;
use crate::mob::EntityRef;
use crate::world::ServerWorld;
use petramond_math::world_pos::WorldPos;
use petramond_world::item::ItemType;

const P0: PlayerId = PlayerId(0);

fn anchor(id: PlayerId, pos: WorldPos) -> PlayerAnchor {
    PlayerAnchor {
        id,
        pos,
        ..Default::default()
    }
}

fn drop_at(x: f32, z: f32) -> DroppedItem {
    DroppedItem::new(
        WorldPos::new(f64::from(x), 64.0, f64::from(z)),
        ItemStack::new(ItemType::Dirt, 1),
        1,
    )
}

/// Checks `dropped_reaction` from items.json. Under water, the stack flips in place on its first
/// tick: same count/identity/age, one fx record. Dry ground: nothing. Pack rows need the fixture
/// registry, so we build our own here.
#[test]
fn dropped_reaction_transforms_the_stack_in_water() {
    let root = std::env::temp_dir().join(format!(
        "petramond-fixture-dropped-reaction-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    let pack = root.join("mods").join("testreact");
    std::fs::create_dir_all(&pack).unwrap();
    std::fs::write(
        pack.join("pack.json"),
        r#"{ "name": "Test React", "id": "testreact", "version": "0.0.1" }"#,
    )
    .unwrap();
    std::fs::write(
        pack.join("items.json"),
        r#"{ "items": [
            { "item": "testreact:flour", "key": "testreact:flour", "name": "Test Flour",
              "max_stack_size": 64, "held_pose": { "pitch": 0, "yaw": 0, "roll": 0 },
              "tags": [],
              "dropped_reaction": { "fluid": "petramond:water", "result": "testreact:dough",
                                    "burst": "petramond:water_splash",
                                    "sound": "petramond:water_splash_small" } },
            { "item": "testreact:dough", "key": "testreact:dough", "name": "Test Dough",
              "max_stack_size": 64, "held_pose": { "pitch": 0, "yaw": 0, "roll": 0 },
              "tags": [] }
        ] }"#,
    )
    .unwrap();
    crate::modding::tests::with_fixture_content(&root, dropped_reaction_inner);
    let _ = std::fs::remove_dir_all(&root);
}

fn dropped_reaction_inner() {
    let by_key = |key: &str| {
        ItemType::by_key(key).unwrap_or_else(|| panic!("fixture item '{key}' registered"))
    };
    let (flour, dough) = (by_key("testreact:flour"), by_key("testreact:dough"));
    assert!(flour.dropped_reaction().is_some(), "the row resolved");

    let mut w = ServerWorld::new(1, 4);
    w.clear_world();
    w.insert_chunk_for_test(
        petramond_world::chunk::ChunkPos::new(0, 0),
        petramond_world::chunk::Chunk::new(0, 0),
    );
    w.set_block_world(2, 63, 2, petramond_world::block::Block::Stone);
    w.set_block_world(2, 64, 2, petramond_world::block::Block::Water);
    w.set_block_world(4, 63, 4, petramond_world::block::Block::Stone);

    let mut wet = DroppedItem::new(WorldPos::new(2.5, 64.5, 2.5), ItemStack::new(flour, 7), 1);
    wet.vel = Vec3::ZERO;
    wet.ticks_lived = 123;
    let mut dry = DroppedItem::new(WorldPos::new(4.5, 64.2, 4.5), ItemStack::new(flour, 3), 2);
    dry.vel = Vec3::ZERO;
    w.spawn_item(wet);
    w.spawn_item(dry);
    let wet_id = w.item_entities()[0].id;

    let fx = w.tick_item_physics(0.05, &[]).fx;
    assert_eq!(fx.len(), 1, "one fx per transformed ENTITY, not per item");
    assert!(fx[0].burst.is_some() && fx[0].sound.is_some());
    let wet_now = &w.item_entities()[0];
    assert_eq!(wet_now.stack.item, dough, "the whole stack transformed");
    assert_eq!(wet_now.stack.count, 7, "count preserved 1:1");
    assert_eq!(wet_now.id, wet_id, "entity identity preserved");
    assert_eq!(wet_now.ticks_lived, 123, "age preserved");
    assert_eq!(
        w.item_entities()[1].stack.item,
        flour,
        "dry flour never transforms"
    );

    let fx = w.tick_item_physics(0.05, &[]).fx;
    assert!(fx.is_empty(), "the transform fires exactly once");
    assert_eq!(w.item_entities()[0].stack.item, dough);
}

fn open_world() -> ServerWorld {
    let mut w = ServerWorld::new(1, 4);
    w.clear_world();
    w.insert_chunk_for_test(
        petramond_world::chunk::ChunkPos::new(0, 0),
        petramond_world::chunk::Chunk::new(0, 0),
    );
    w
}

fn launched(pos: WorldPos, vel: Vec3, owner: Option<EntityRef>) -> DroppedItem {
    DroppedItem::launched(pos, ItemStack::new(ItemType::Dirt, 1), vel, owner)
}

#[test]
fn a_flight_strikes_the_first_body_on_its_path_and_stops_there() {
    let mut w = open_world();
    assert!(w
        .mobs_mut()
        .spawn(crate::mob::Mob::Owl, WorldPos::new(5.5, 64.0, 5.5), 0.0));
    let owl = w.mobs().instances()[0].id();
    w.spawn_item(launched(
        WorldPos::new(0.5, 64.2, 5.5),
        Vec3::new(200.0, 0.0, 0.0),
        None,
    ));

    let step = w.tick_item_physics(0.05, &[]);
    assert_eq!(step.impacts.len(), 1);
    assert_eq!(step.impacts[0].target, ImpactTarget::Mob(owl));
    let it = &w.item_entities()[0];
    assert!(
        it.pos.x < 5.5,
        "seated at the body, not flown through: {:?}",
        it.pos
    );
    assert!(
        matches!(it.motion, Motion::Flight(_)),
        "resolution is the server's"
    );
    assert!(!it.collectable(), "an item in the air is nobody's pickup");
}

#[test]
fn a_flight_stops_at_collidable_terrain_and_reports_the_face() {
    let mut w = open_world();
    w.set_block_world(5, 64, 5, petramond_world::block::Block::Stone);
    w.spawn_item(launched(
        WorldPos::new(0.5, 64.5, 5.5),
        Vec3::new(200.0, 0.0, 0.0),
        None,
    ));

    let step = w.tick_item_physics(0.05, &[]);
    assert_eq!(
        step.impacts[0].target,
        ImpactTarget::Block {
            cell: IVec3::new(5, 64, 5),
            face: IVec3::new(-1, 0, 0),
        }
    );
    assert!(w.item_entities()[0].pos.x < 5.0);
}

#[test]
fn a_flight_spares_its_launcher_only_until_it_has_left_the_body() {
    let mut w = open_world();
    let me = PlayerId(3);
    let feet = WorldPos::new(2.5, 64.0, 2.5);
    let launcher = PlayerAnchor {
        id: me,
        pos: feet + Vec3::Y,
        body: Some(petramond_world::body::Body::new(feet, 0.3, 1.8)),
        ..Default::default()
    };
    w.spawn_item(launched(
        feet + Vec3::new(0.0, 1.6, 0.0),
        Vec3::new(0.0, 0.0, 10.0),
        Some(EntityRef::Player(me)),
    ));
    let step = w.tick_item_physics(0.05, &[launcher]);
    assert!(step.impacts.is_empty(), "leaving the launcher's own body");
    let Motion::Flight(f) = w.item_entities()[0].motion else {
        panic!("still flying");
    };
    assert!(!f.left_owner, "still inside the launcher");
    w.tick_item_physics(0.05, &[launcher]);
    let Motion::Flight(f) = w.item_entities()[0].motion else {
        panic!("still flying");
    };
    assert!(f.left_owner, "outside the launcher's box now");

    w.item_entities_mut().clear();
    w.spawn_item(launched(
        feet + Vec3::new(0.0, 1.6, 0.0),
        Vec3::new(0.0, 0.0, 4.0),
        Some(EntityRef::Player(me)),
    ));
    for tick in 0..6 {
        let walked = feet + Vec3::new(0.0, 0.0, 6.0 * 0.05 * tick as f32);
        let walking = PlayerAnchor {
            pos: walked + Vec3::Y,
            body: Some(petramond_world::body::Body::new(walked, 0.3, 1.8)),
            ..launcher
        };
        let step = w.tick_item_physics(0.05, &[walking]);
        assert!(step.impacts.is_empty(), "tick {tick}: walked into own shot");
        let Motion::Flight(f) = w.item_entities()[0].motion else {
            panic!("still flying");
        };
        assert!(!f.left_owner, "tick {tick}: the body kept meeting it");
    }

    w.item_entities_mut().clear();
    w.spawn_item(launched(
        feet + Vec3::new(0.0, 5.0, 0.0),
        Vec3::new(0.0, -40.0, 0.0),
        Some(EntityRef::Player(me)),
    ));
    let step = w.tick_item_physics(0.05, &[launcher]);
    assert!(
        step.impacts.is_empty(),
        "the first tick is clear of the body"
    );
    let step = w.tick_item_physics(0.05, &[launcher]);
    assert_eq!(
        step.impacts.len(),
        1,
        "a returning launch strikes its launcher"
    );
    assert_eq!(step.impacts[0].target, ImpactTarget::Player(me));
}

#[test]
fn a_restored_lodged_item_reverifies_its_anchor_once_loaded() {
    let mut w = open_world();
    let restored = |x: f32, anchor: IVec3| {
        let mut it = launched(
            WorldPos::new(f64::from(x), 64.5, 5.5),
            Vec3::new(10.0, 0.0, 0.0),
            None,
        );
        it.lodge(anchor);
        it.vel = Vec3::ZERO;
        let Motion::Stuck(stuck) = &mut it.motion else {
            unreachable!()
        };
        stuck.verified = false;
        it
    };
    w.set_block_world(5, 64, 5, petramond_world::block::Block::Stone);
    w.spawn_item(restored(4.8, IVec3::new(5, 64, 5)));
    w.spawn_item(restored(7.8, IVec3::new(8, 64, 5)));
    w.spawn_item(restored(15.8, IVec3::new(16, 64, 5)));

    w.tick_item_physics(0.05, &[]);
    let items = w.item_entities();
    assert!(
        matches!(items[0].motion, Motion::Stuck(s) if s.verified),
        "held, and verified against live collision: {:?}",
        items[0].motion
    );
    assert!(
        matches!(items[1].motion, Motion::Flight(_)),
        "flying again: the anchor is air"
    );
    assert!(
        matches!(items[2].motion, Motion::Stuck(s) if !s.verified),
        "an unloaded column proves nothing: {:?}",
        items[2].motion
    );
}

#[test]
fn a_lodged_item_holds_until_its_block_goes_then_resumes_with_horizontal_drift() {
    let mut w = open_world();
    w.set_block_world(5, 64, 5, petramond_world::block::Block::Stone);
    let approach = Vec3::new(10.0, -4.0, -6.0);
    let mut it = launched(WorldPos::new(4.8, 64.5, 5.5), approach, None);
    it.lodge(IVec3::new(5, 64, 5));
    assert!(matches!(it.motion, Motion::Stuck(_)));
    let id = w.spawn_item(it);

    w.tick_item_physics(0.05, &[]);
    let held = &w.item_entities()[0];
    assert!(matches!(held.motion, Motion::Stuck(_)));
    assert_eq!(
        held.pos,
        WorldPos::new(4.8, 64.5, 5.5),
        "held exactly where it lodged"
    );
    assert!(held.collectable(), "a lodged item can be pulled out");

    w.set_block_world(5, 64, 5, petramond_world::block::Block::Air);
    w.tick_item_physics(0.05, &[]);
    let released = w.dropped_items().get(id).unwrap();
    assert!(matches!(released.motion, Motion::Flight(_)));
    assert_eq!(
        released.vel.y, 0.0,
        "only gravity supplies vertical velocity"
    );
    let forward = Vec3::new(approach.x, 0.0, approach.z).normalize();
    assert!(released.vel.dot(forward) > 0.0);
    assert!(released.vel.normalize().abs_diff_eq(forward, 1e-5));
    assert!(
        released.vel.length() < approach.length(),
        "a nudge, not the original launch speed"
    );
    assert_eq!(released.pos, WorldPos::new(4.8, 64.5, 5.5));
    assert!(!released.collectable());
    w.tick_item_physics(0.05, &[]);
    let falling = w.dropped_items().get(id).unwrap();
    assert!(falling.pos.y < 64.5 && falling.vel.y < 0.0);
    assert!(falling.pos.x > 4.8 && falling.pos.z < 5.5);
    let pitch = falling.heading().unwrap().pitch;
    assert!(
        pitch > -std::f32::consts::FRAC_PI_2 && pitch < 0.0,
        "the first falling pose remains slanted"
    );
}

#[test]
fn dislodged_projectiles_use_the_same_body_sweep_and_keep_their_launcher() {
    for player in [true, false] {
        let mut w = open_world();
        let feet = WorldPos::new(5.5, 64.0, 5.5);
        let (target, owner, anchors) = if player {
            let id = PlayerId(3);
            (
                ImpactTarget::Player(id),
                EntityRef::Player(id),
                vec![PlayerAnchor {
                    id,
                    pos: feet + Vec3::Y,
                    body: Some(petramond_world::body::Body::new(feet, 0.3, 1.8)),
                    ..Default::default()
                }],
            )
        } else {
            assert!(w.mobs_mut().spawn(crate::mob::Mob::Owl, feet, 0.0));
            let id = w.mobs().instances()[0].id();
            (ImpactTarget::Mob(id), EntityRef::Mob(id), vec![])
        };
        let block = IVec3::new(5, 69, 5);
        w.set_block_world(
            block.x,
            block.y,
            block.z,
            petramond_world::block::Block::Stone,
        );
        let mut shot = launched(WorldPos::new(5.5, 68.8, 5.5), Vec3::Y * 50.0, Some(owner));
        if let Motion::Flight(f) = &mut shot.motion {
            f.left_owner = true;
        }
        let flight = shot.motion.projectile().unwrap();
        let stack = shot.stack;
        shot.lodge(block);
        let id = w.spawn_item(shot);
        w.tick_item_physics(0.05, &anchors);
        w.set_block_world(
            block.x,
            block.y,
            block.z,
            petramond_world::block::Block::Air,
        );
        assert!(w.tick_item_physics(0.05, &anchors).impacts.is_empty());
        let resumed = w.dropped_items().get(id).unwrap();
        assert_eq!(resumed.vel, Vec3::ZERO);
        assert_eq!(resumed.motion, Motion::Flight(flight));
        assert_eq!(resumed.stack, stack);
        let impact = (0..40)
            .find_map(|_| {
                w.tick_item_physics(0.05, &anchors)
                    .impacts
                    .into_iter()
                    .next()
            })
            .expect("the falling projectile hits the body below");
        assert_eq!(impact.id, id, "it is the same projectile");
        assert_eq!(
            impact.target, target,
            "even its original launcher can be struck"
        );
        assert!(impact.vel.y < -1.0 && impact.vel.x == 0.0 && impact.vel.z == 0.0);
        assert_eq!(
            w.dropped_items()
                .get(id)
                .unwrap()
                .motion
                .projectile()
                .unwrap()
                .owner,
            Some(owner)
        );
    }
}

#[test]
fn lifetime_advances_and_despawns_at_the_limit() {
    let mut w = ServerWorld::new(0, 0);
    let mut item = drop_at(0.5, 0.5);
    item.ticks_lived = ITEM_LIFETIME_TICKS - 2;
    w.spawn_item(item);
    w.tick_item_lifetime();
    assert_eq!(w.item_entities()[0].ticks_lived, ITEM_LIFETIME_TICKS - 1);
    w.tick_item_lifetime();
    assert!(
        w.item_entities().is_empty(),
        "despawns once it reaches the lifetime limit"
    );
}

#[test]
fn item_rules_prevent_collection_and_keep_their_own_expiry_after_lodging() {
    use petramond_world::item::variant;
    let rules = [(
        "petramond:item_entity".into(),
        br#"{"pickup":false,"lifetime_ticks":37}"#.to_vec(),
    )]
    .into();
    let stack = ItemStack::with_variant(ItemType::Stone, 1, variant::intern(&rules).unwrap());
    let at = WorldPos::new(0.0, 64.0, 0.0);
    let mut shot = DroppedItem::launched(at, stack, Vec3::Z, Some(EntityRef::Mob(42)));
    assert!(!shot.collectable());
    shot.lodge(IVec3::new(0, 64, 1));
    assert!(!shot.collectable());
    shot.release();
    assert!(
        !shot.collectable(),
        "breaking the anchor cannot enable pickup"
    );
    shot.ticks_lived = 35;
    let world = ServerWorld::new(1, 4);
    let mut drops = DroppedItems::default();
    let old = drops.spawn(shot);
    let fresh = drops.spawn(DroppedItem::new(at, stack, 1));
    drops.merge_nearby();
    assert_eq!(
        drops.items.len(),
        2,
        "merging cannot extend a custom expiry"
    );
    drops.request_pickups(P0, at, |s| s.count);
    assert!(drops.items.iter().all(|it| it.pickup_requested.is_none()));
    drops.tick_lifetime(&world, false);
    assert!(drops.get(old).is_some());
    drops.tick_lifetime(&world, false);
    assert!(drops.get(old).is_none());
    assert!(drops.get(fresh).is_some());
}

#[test]
fn pickup_waits_out_the_delay_then_collects() {
    let mut w = ServerWorld::new(0, 0);
    let player = WorldPos::new(0.5, 64.0, 0.5);
    w.spawn_item(drop_at(0.5, 0.5));
    let mut collected = 0u32;
    w.dropped_items_mut()
        .request_pickups(P0, player, |s| s.count);
    w.dropped_items_mut()
        .collect_requested_pickups(P0, player, |s| {
            collected += s.count as u32;
            None
        });
    assert_eq!(collected, 0, "the pickup delay blocks collection");
    assert_eq!(w.item_entities().len(), 1);
    assert!(
        w.item_entities()[0].pickup_requested.is_none(),
        "delayed drops are not requested"
    );

    w.item_entities_mut()[0].ticks_lived = ITEM_PICKUP_DELAY_TICKS;
    w.dropped_items_mut()
        .request_pickups(P0, player, |s| s.count);
    w.dropped_items_mut()
        .collect_requested_pickups(P0, player, |s| {
            collected += s.count as u32;
            None
        });
    assert_eq!(collected, 1, "collected once past the delay");
    assert!(w.item_entities().is_empty());
}

#[test]
fn pickup_splits_off_only_the_part_that_fits() {
    let mut w = ServerWorld::new(0, 0);
    let player = WorldPos::new(0.5, 64.0, 0.5);
    let mut item = DroppedItem::new(player, ItemStack::new(ItemType::Dirt, 10), 1);
    item.ticks_lived = 1234;
    let origin_pos = item.pos;
    let origin_vel = item.vel;
    w.spawn_item(item);
    w.dropped_items_mut().request_pickups(P0, player, |_| 6);

    assert_eq!(w.item_entities().len(), 2);
    let original = w
        .item_entities()
        .iter()
        .find(|d| d.pickup_requested.is_none())
        .expect("original kept, despawn timer untouched");
    assert_eq!(
        original.stack.count, 4,
        "original reduced by the part that fit"
    );
    assert_eq!(
        original.ticks_lived, 1234,
        "original despawn timer untouched"
    );
    let split = w
        .item_entities()
        .iter()
        .find(|d| d.pickup_requested == Some(P0))
        .expect("split drop requested by the planning player");
    assert_eq!(
        split.stack.count, 6,
        "split carries exactly the part that fit"
    );
    assert_eq!(split.stack.item, ItemType::Dirt);
    assert_eq!(split.ticks_lived, 1234, "split keeps the source lifetime");
    assert_eq!(
        split.pos, origin_pos,
        "split spawns exactly where the original is"
    );
    assert_eq!(
        split.vel, origin_vel,
        "split inherits the original's velocity"
    );
}

#[test]
fn pickup_replans_existing_request_before_splitting_more() {
    let mut w = ServerWorld::new(0, 0);
    let player = WorldPos::new(0.5, 64.0, 0.5);
    let mut item = DroppedItem::new(player, ItemStack::new(ItemType::Dirt, 10), 1);
    item.ticks_lived = ITEM_PICKUP_DELAY_TICKS;
    w.spawn_item(item);

    let mut remaining = 6;
    w.dropped_items_mut().request_pickups(P0, player, |s| {
        let count = remaining.min(s.count);
        remaining -= count;
        count
    });
    assert_eq!(w.item_entities().len(), 2);

    let mut remaining = 6;
    w.dropped_items_mut().request_pickups(P0, player, |s| {
        let count = remaining.min(s.count);
        remaining -= count;
        count
    });

    assert_eq!(w.item_entities().len(), 2, "no duplicate split-off");
    let requested: u32 = w
        .item_entities()
        .iter()
        .filter(|d| d.pickup_requested.is_some())
        .map(|d| d.stack.count as u32)
        .sum();
    let unrequested: u32 = w
        .item_entities()
        .iter()
        .filter(|d| d.pickup_requested.is_none())
        .map(|d| d.stack.count as u32)
        .sum();
    assert_eq!(requested, 6);
    assert_eq!(unrequested, 4);
}

#[test]
fn a_split_drop_tracks_the_original_instead_of_drifting() {
    let mut w = ServerWorld::new(0, 0);
    let mut item = DroppedItem::new(
        WorldPos::new(0.5, 80.0, 0.5),
        ItemStack::new(ItemType::Dirt, 10),
        7,
    );
    item.ticks_lived = ITEM_PICKUP_DELAY_TICKS;
    item.vel = Vec3::new(3.0, 0.0, 1.0);
    let player = WorldPos::new(0.5, 80.0, 0.5);
    w.spawn_item(item);
    w.dropped_items_mut().request_pickups(P0, player, |_| 6);
    assert_eq!(w.item_entities().len(), 2);

    let far = WorldPos::new(1000.0, 80.0, 0.5);
    for _ in 0..30 {
        w.tick_item_physics(1.0 / 60.0, &[anchor(P0, far)]);
    }
    let p0 = w.item_entities()[0].pos;
    let p1 = w.item_entities()[1].pos;
    assert_eq!(p0, p1, "split and original stay co-located, not nearby");
}

#[test]
fn pickup_leaves_a_drop_with_no_room() {
    let mut w = ServerWorld::new(0, 0);
    let player = WorldPos::new(0.5, 64.0, 0.5);
    let mut item = DroppedItem::new(player, ItemStack::new(ItemType::Dirt, 10), 1);
    item.ticks_lived = ITEM_PICKUP_DELAY_TICKS;
    w.spawn_item(item);
    w.dropped_items_mut().request_pickups(P0, player, |_| 0);
    w.dropped_items_mut()
        .collect_requested_pickups(P0, player, |_| None);
    assert_eq!(
        w.item_entities().len(),
        1,
        "a full inventory leaves the drop"
    );
    assert_eq!(w.item_entities()[0].stack.count, 10, "untouched");
    assert!(
        w.item_entities()[0].pickup_requested.is_none(),
        "unrequested drops are left alone"
    );
}

#[test]
fn magnet_skips_a_drop_that_was_not_requested() {
    let mut w = ServerWorld::new(0, 0);
    let target = WorldPos::new(0.5, 65.0, 0.5);
    let mut item = drop_at(0.5, 0.5);
    item.pos = WorldPos::new(0.5, 64.5, 0.5);
    item.vel = Vec3::ZERO;
    item.ticks_lived = ITEM_PICKUP_DELAY_TICKS;
    w.spawn_item(item);

    let before_y = w.item_entities()[0].pos.y;
    w.tick_item_physics(1.0 / 60.0, &[anchor(P0, target)]);
    let after_y = w.item_entities()[0].pos.y;
    assert!(
        after_y < before_y,
        "an unrequested drop should fall, not rise toward the player: {before_y} -> {after_y}"
    );
}

#[test]
fn magnet_pulls_a_requested_drop() {
    let mut w = ServerWorld::new(0, 0);
    let target = WorldPos::new(0.5, 65.0, 0.5);
    let mut item = drop_at(0.5, 0.5);
    item.pos = WorldPos::new(0.5, 64.5, 0.5);
    item.vel = Vec3::ZERO;
    item.ticks_lived = ITEM_PICKUP_DELAY_TICKS;
    w.spawn_item(item);
    w.dropped_items_mut()
        .request_pickups(P0, target, |s| s.count);
    assert_eq!(w.item_entities()[0].pickup_requested, Some(P0));

    let before_y = w.item_entities()[0].pos.y;
    w.tick_item_physics(1.0 / 60.0, &[anchor(P0, target)]);
    let after_y = w.item_entities()[0].pos.y;
    assert!(
        after_y > before_y,
        "a requested drop should be pulled up toward the player: {before_y} -> {after_y}"
    );
}

#[test]
fn magnet_pulls_toward_the_requester_not_the_nearest_player() {
    let p1 = PlayerId(1);
    let mut w = ServerWorld::new(0, 0);
    let p0_pos = WorldPos::new(1.2, 64.0, 0.5);
    let p1_pos = WorldPos::new(0.1, 64.0, 0.5);
    let mut item = drop_at(0.5, 0.5);
    item.pos = WorldPos::new(0.5, 64.0, 0.5);
    item.vel = Vec3::ZERO;
    item.ticks_lived = ITEM_PICKUP_DELAY_TICKS;
    w.spawn_item(item);
    w.dropped_items_mut()
        .request_pickups(P0, p0_pos, |s| s.count);
    assert_eq!(w.item_entities()[0].pickup_requested, Some(P0));

    let before_x = w.item_entities()[0].pos.x;
    w.tick_item_physics(1.0 / 60.0, &[anchor(P0, p0_pos), anchor(p1, p1_pos)]);
    let after_x = w.item_entities()[0].pos.x;
    assert!(
        after_x > before_x,
        "the drop flies toward its requester (+X), not the nearer player: {before_x} -> {after_x}"
    );
}

#[test]
fn stale_requests_release_when_the_requester_is_gone() {
    let mut w = ServerWorld::new(0, 0);
    let player = WorldPos::new(0.5, 64.0, 0.5);
    let mut item = drop_at(0.5, 0.5);
    item.ticks_lived = ITEM_PICKUP_DELAY_TICKS;
    w.spawn_item(item);
    w.dropped_items_mut()
        .request_pickups(P0, player, |s| s.count);
    assert_eq!(w.item_entities()[0].pickup_requested, Some(P0));

    w.dropped_items_mut()
        .release_requests_not_from(|id| id != P0);
    assert!(
        w.item_entities()[0].pickup_requested.is_none(),
        "the leaver's reservation is released"
    );
}

#[test]
fn a_drop_waits_for_the_section_under_it_to_arrive() {
    use petramond_world::chunk::ChunkPos;
    let dir = petramond_util::test_dirs::TestScratchDir::new("drop-freeze");
    let mut w = ServerWorld::new(0, 0);
    let opened = crate::save::open_at(dir.to_path_buf()).expect("temp save opens");
    w.attach_save(opened.save, opened.saved);
    let column = ChunkPos::new(0, 0);
    w.data.ensure_column(column);
    w.insert_empty_column_for_test(column);
    let floor = SectionPos::new(0, 3, 0);
    w.side.gen.awaited_overlays.insert(floor);
    w.note_stream_nonfinal(floor);
    let start = drop_at(2.5, 2.5).pos;
    w.spawn_item(drop_at(2.5, 2.5));
    for _ in 0..40 {
        w.tick_item_physics(0.05, &[]);
    }
    assert_eq!(
        w.item_entities()[0].pos,
        start,
        "held still over terrain that has not arrived"
    );
    w.side.gen.awaited_overlays.remove(&floor);
    w.settle_stream_nonfinal(floor);
    for _ in 0..10 {
        w.tick_item_physics(0.05, &[]);
    }
    assert!(
        w.item_entities()[0].pos.y < start.y,
        "simulates again once the floor section is final"
    );
}

#[test]
fn unloading_a_section_harvests_only_its_items() {
    let mut w = ServerWorld::new(0, 0);
    w.spawn_item(drop_at(2.5, 2.5));
    w.spawn_item(drop_at(20.5, 2.5));
    let taken = w
        .dropped_items_mut()
        .take_items_in_section(SectionPos::new(0, 4, 0));
    assert_eq!(taken.len(), 1, "only the (0,4,0) drop is harvested");
    assert_eq!(w.item_entities().len(), 1, "the (1,4,0) drop stays active");
    assert!(w.item_entities()[0].pos.x > 16.0);
}

#[test]
fn items_group_by_owning_section_for_flush() {
    let mut w = ServerWorld::new(0, 0);
    w.spawn_item(drop_at(2.5, 2.5));
    w.spawn_item(drop_at(5.5, 9.5));
    w.spawn_item(drop_at(20.5, 2.5));
    let map = w.dropped_items_mut().items_by_section();
    assert_eq!(map[&SectionPos::new(0, 4, 0)].len(), 2);
    assert_eq!(map[&SectionPos::new(1, 4, 0)].len(), 1);
}

#[test]
fn nearby_compatible_stacks_merge_into_one_entity() {
    let mut w = ServerWorld::new(0, 0);
    w.spawn_item(drop_at(0.5, 0.5));
    w.spawn_item(drop_at(1.2, 0.5));
    w.dropped_items_mut().merge_nearby();
    assert_eq!(w.item_entities().len(), 1);
    assert_eq!(w.item_entities()[0].stack.count, 2);
    assert_eq!(w.item_entities()[0].stack.item, ItemType::Dirt);
}

#[test]
fn merging_respects_the_max_stack_size() {
    let mut w = ServerWorld::new(0, 0);
    for x in [0.5, 1.1] {
        let mut item = drop_at(x, 0.5);
        item.stack = ItemStack::new(ItemType::Dirt, 40);
        w.spawn_item(item);
    }
    w.dropped_items_mut().merge_nearby();
    assert_eq!(w.item_entities().len(), 2, "64 + remainder");
    let mut counts: Vec<u8> = w
        .item_entities()
        .iter()
        .map(|d| d.stack.count)
        .collect::<Vec<_>>();
    counts.sort();
    assert_eq!(counts, vec![16, 64]);
}

#[test]
fn different_items_do_not_merge() {
    let mut w = ServerWorld::new(0, 0);
    w.spawn_item(drop_at(0.5, 0.5));
    let mut other = drop_at(0.6, 0.6);
    other.stack.item = ItemType::Stone;
    w.spawn_item(other);
    w.dropped_items_mut().merge_nearby();
    assert_eq!(w.item_entities().len(), 2);
}

#[test]
fn drops_beyond_the_radius_do_not_merge() {
    let mut w = ServerWorld::new(0, 0);
    w.spawn_item(drop_at(0.5, 0.5));
    w.spawn_item(drop_at(1.8, 0.5));
    w.dropped_items_mut().merge_nearby();
    assert_eq!(w.item_entities().len(), 2);
}

#[test]
fn requested_drops_never_merge() {
    let mut w = ServerWorld::new(0, 0);
    let mut flying = drop_at(0.5, 0.5);
    flying.pickup_requested = Some(P0);
    w.spawn_item(flying);
    w.spawn_item(drop_at(0.7, 0.5));
    w.dropped_items_mut().merge_nearby();
    assert_eq!(
        w.item_entities().len(),
        2,
        "the reserved drop neither absorbs nor is absorbed"
    );
    assert!(w.item_entities()[0].pickup_requested.is_some());
}

#[test]
fn merged_pile_keeps_the_most_remaining_lifetime() {
    let mut w = ServerWorld::new(0, 0);
    let mut old = drop_at(0.5, 0.5);
    old.ticks_lived = ITEM_LIFETIME_TICKS - 200;
    w.spawn_item(old);
    w.spawn_item(drop_at(1.2, 0.5));
    w.dropped_items_mut().merge_nearby();
    assert_eq!(w.item_entities().len(), 1);
    assert_eq!(
        w.item_entities()[0].ticks_lived,
        0,
        "the pile lives as long as its youngest member"
    );
}
