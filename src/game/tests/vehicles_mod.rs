//! Coarse lifecycle smoke test for the vehicles pack over the generic engine
//! mechanisms. Focused subsystem tests own riding, collision, animation,
//! authority, and interpolation invariants; this only proves the real WASM
//! and content compose into place → board → drive → dismount → break/drop.

use super::super::tick::TickEvents;
use crate::camera::Camera;
use crate::mathh::{IVec3, Vec3};

fn aim_at(game: &mut super::common::TestGame, point: Vec3) {
    let player = &mut game.server.sessions[0].player;
    let delta = point - player.eye();
    player.yaw = delta.x.atan2(delta.z);
    player.pitch = delta.y.atan2(Vec3::new(delta.x, 0.0, delta.z).length());
}

#[test]
fn boat_places_boards_drives_and_breaks_via_wasm() {
    let Some(root) = crate::modding::tests::stage_mods_fixture("vehicles", &["vehicles"]) else {
        return;
    };
    crate::modding::tests::run_child_test(&root, "game::tests::vehicles_mod::boat_inner");
}

#[test]
fn a_dry_solid_lands_on_a_solid_peer_and_can_drive_next_tick() {
    let Some(root) =
        crate::modding::tests::stage_mods_fixture("vehicles-solid-landing", &["vehicles"])
    else {
        return;
    };
    crate::modding::tests::run_child_test(
        &root,
        "game::tests::vehicles_mod::solid_peer_landing_inner",
    );
}

#[test]
#[ignore = "spawned with the vehicles fixture before registry initialization"]
fn solid_peer_landing_inner() {
    use crate::block::Block;

    let boat = crate::mob::defs()
        .iter()
        .position(|def| def.name == "vehicles:boat")
        .map(|index| crate::mob::Mob(index as u8))
        .expect("vehicles:boat registered from the fixture pack");
    let mut world = crate::world::World::new(0, 1);
    super::common::flat_floor_loaded_air(&mut world, Block::Stone);

    let mut mobs = crate::mob::Mobs::new(0);
    let lower_pos = Vec3::new(8.0, 64.0, 8.0);
    assert!(mobs.spawn(boat, lower_pos, 0.0));
    let lower_id = mobs.instances()[0].id();
    assert!(mobs.spawn(boat, Vec3::new(8.0, 68.0, 8.0), 0.0));
    let upper_id = mobs.instances()[1].id();
    let anchor = crate::mob::PlayerAnchor {
        id: Default::default(),
        pos: Vec3::new(1000.0, 64.0, 1000.0),
        body: None,
        sneaking: false,
    };

    let mut landed_fall = None;
    for _ in 0..160 {
        let tick = mobs.tick(0.05, &world, &[anchor], false);
        if let Some(fall) = tick.falls.iter().find(|fall| fall.mob_id == upper_id) {
            landed_fall = Some(fall.distance);
            break;
        }
    }
    let fall = landed_fall.expect("the upper solid lands on its peer");
    assert!(
        fall > 2.0,
        "fall bookkeeping observes the full dry drop: {fall}"
    );
    let upper_index = mobs
        .index_of_id(upper_id)
        .expect("upper solid remains live");
    assert!(
        mobs.instances()[upper_index].on_ground(),
        "top-face peer contact is authoritative ground"
    );
    let lower_index = mobs
        .index_of_id(lower_id)
        .expect("lower solid remains live");
    let size = crate::mob::def(boat).size;
    let lower = &mobs.instances()[lower_index];
    let upper = &mobs.instances()[upper_index];
    assert!(
        (upper.pos.y - (lower.pos.y + size.height)).abs() < 2e-3,
        "the compound bodies meet at the support face"
    );

    let before = upper.pos;
    let requested_yaw = 0.35;
    assert!(mobs.set_mob_drive(upper_index, 1.0, 0.0, Some(requested_yaw)));
    let next = mobs.tick(0.05, &world, &[anchor], false);
    assert!(
        next.falls.iter().all(|fall| fall.mob_id != upper_id),
        "the landing is reported exactly once"
    );
    let upper = &mobs.instances()[mobs.index_of_id(upper_id).unwrap()];
    assert!(upper.on_ground(), "the supporting peer remains ground");
    assert!(
        upper.pos.x > before.x + 0.01,
        "a dry supported solid can consume drive on the next tick"
    );
    assert!(
        (upper.yaw - requested_yaw).abs() < 1e-4,
        "supported steering applies its requested yaw"
    );
}

#[test]
#[ignore = "spawned with the vehicles fixture before registry initialization"]
fn boat_inner() {
    use crate::block::Block;
    use crate::chunk::{Chunk, ChunkPos, CHUNK_SX, CHUNK_SZ};
    use crate::item::ItemStack;
    use crate::net::protocol::{ClientToServer, PlayerAction, TargetRef};

    let boat_mob = crate::mob::defs()
        .iter()
        .position(|def| def.name == "vehicles:boat")
        .map(|index| crate::mob::Mob(index as u8))
        .expect("vehicles:boat registered from the fixture pack");
    let boat_item = *crate::item::ItemType::all()
        .iter()
        .find(|item| item.key() == "vehicles:boat")
        .expect("vehicles:boat item registered");

    let mut game =
        super::common::game_with_camera(Camera::new(Vec3::new(5.0, 66.0, 8.0), 16.0 / 9.0));
    assert_eq!(game.mods_for_test().loaded(), 1, "vehicles WASM loaded");

    game.server.world.clear_world();
    game.server
        .world
        .insert_empty_column_for_test(ChunkPos::new(0, 0));
    let mut chunk = Chunk::new(0, 0);
    for z in 0..CHUNK_SZ {
        for x in 0..CHUNK_SX {
            chunk.set_block(x, 62, z, Block::Stone);
            if (4..14).contains(&x) && (2..14).contains(&z) {
                chunk.set_water(x, 63, z, Block::Water, 0);
            } else {
                chunk.set_block(x, 63, z, Block::Stone);
            }
        }
    }
    game.server
        .world
        .insert_chunk_for_test(ChunkPos::new(0, 0), chunk);

    let session = &mut game.server.sessions[0];
    session.player.pos = Vec3::new(3.5, 64.0, 8.5);
    session.player.vel = Vec3::ZERO;
    session.player.on_ground = true;
    session.player.yaw = std::f32::consts::FRAC_PI_2;
    session.claim_pos = session.player.pos;
    session.intent_gameplay = true;
    session
        .player
        .inventory
        .add(ItemStack::new(crate::item::ItemType::Stone, 1));
    session.player.inventory.add(ItemStack::new(boat_item, 1));

    let mut events = TickEvents::default();
    let water_cell = IVec3::new(5, 63, 8);
    let water_target = Vec3::new(5.5, 63.99, 8.5);
    aim_at(&mut game, water_target);
    let use_click = |block| {
        ClientToServer::Action(PlayerAction::UseClick {
            mob: None,
            target: Some(TargetRef {
                block,
                normal: IVec3::Y,
            }),
            request_id: None,
            predicted: false,
            jabbed: false,
        })
    };

    // Receipt-time targeting and tick-time use must be one held-item
    // transaction. Without the selection guard, a normal-ray click can name
    // an arbitrary in-reach water cell, switch to the boat before Placement,
    // and make the boat handler consume a target that was never water-ray
    // validated.
    game.server.apply_message(
        0,
        ClientToServer::Action(PlayerAction::UseClick {
            mob: None,
            target: Some(TargetRef {
                block: water_cell,
                normal: IVec3::Y,
            }),
            request_id: Some(700),
            predicted: true,
            jabbed: true,
        }),
    );
    game.server.sessions[0].player.inventory.set_active(1);
    game.server.game_tick_step(&mut events);
    assert!(
        !game
            .server
            .world
            .mobs()
            .instances()
            .iter()
            .any(|mob| mob.kind == boat_mob),
        "switching from a normal-ray item to the boat cannot reuse the click"
    );
    assert_eq!(
        game.server.sessions[0]
            .player
            .inventory
            .slot(0)
            .map(|stack| (stack.item, stack.count)),
        Some((crate::item::ItemType::Stone, 1)),
        "the click-time item stays untouched"
    );
    assert_eq!(
        game.server.sessions[0]
            .player
            .inventory
            .selected()
            .map(|stack| (stack.item, stack.count)),
        Some((boat_item, 1)),
        "the newly selected boat is not consumed"
    );
    let outcome = game.server.sessions[0]
        .pending_action_outcomes
        .iter()
        .find(|outcome| outcome.id == 700)
        .expect("the invalidated prediction is answered");
    assert!(!outcome.accepted);
    assert_eq!(
        outcome.reason,
        Some(crate::net::protocol::ActionDenyReason::Denied)
    );
    assert!(
        game.server.sessions[0]
            .pending_corrective_cells
            .contains(&water_cell),
        "selection invalidation keeps the ordinary corrective-cell contract"
    );
    game.server.game_tick_step(&mut events);
    assert!(
        !game
            .server
            .world
            .mobs()
            .instances()
            .iter()
            .any(|mob| mob.kind == boat_mob),
        "the invalidated click leaves no stale intent for a later tick"
    );
    assert_eq!(
        game.server.sessions[0]
            .player
            .inventory
            .selected()
            .map(|stack| stack.item),
        Some(boat_item)
    );

    // The water ray is server authority. A merely in-reach cell cannot be
    // named through a wall, nor can a farther water cell replace the first
    // water hit along the current view ray.
    for wall in [IVec3::new(4, 64, 8), IVec3::new(4, 65, 8)] {
        game.server
            .world
            .set_block_world(wall.x, wall.y, wall.z, Block::Stone);
    }
    game.server.apply_message(0, use_click(water_cell));
    game.server.game_tick_step(&mut events);
    assert!(
        !game
            .server
            .world
            .mobs()
            .instances()
            .iter()
            .any(|mob| mob.kind == boat_mob),
        "an occluded client claim cannot launch a boat"
    );
    for wall in [IVec3::new(4, 64, 8), IVec3::new(4, 65, 8)] {
        game.server
            .world
            .set_block_world(wall.x, wall.y, wall.z, Block::Air);
    }

    game.server
        .apply_message(0, use_click(water_cell + IVec3::X));
    game.server.game_tick_step(&mut events);
    assert!(
        !game
            .server
            .world
            .mobs()
            .instances()
            .iter()
            .any(|mob| mob.kind == boat_mob),
        "a non-first water-cell claim cannot launch a boat"
    );
    assert_eq!(
        game.server.sessions[0]
            .player
            .inventory
            .selected()
            .map(|stack| stack.item),
        Some(boat_item),
        "rejected claims keep the item"
    );

    // Checked spawning validates the COMPLETE segmented hull, not just the
    // target cell. Shore touching only the bow refuses transactionally and
    // refunds the consumed item.
    let shore = IVec3::new(6, 64, 8);
    game.server
        .world
        .set_block_world(shore.x, shore.y, shore.z, Block::Stone);
    game.server.apply_message(0, use_click(water_cell));
    game.server.game_tick_step(&mut events);
    assert!(
        !game
            .server
            .world
            .mobs()
            .instances()
            .iter()
            .any(|mob| mob.kind == boat_mob),
        "a hull cannot spawn with its bow through adjacent shore"
    );
    assert_eq!(
        game.server.sessions[0]
            .player
            .inventory
            .selected()
            .map(|stack| (stack.item, stack.count)),
        Some((boat_item, 1)),
        "a blocked checked spawn refunds the boat"
    );
    game.server
        .world
        .set_block_world(shore.x, shore.y, shore.z, Block::Air);

    // The engine commits simultaneously driven rigid bodies from one shared
    // time of impact. Exercise the complete real-pack path here: registry row,
    // drive intent, mob tick proposal, pair solve, and committed compound hull.
    let pair_y = 63.9;
    let pair_z = 5.5;
    let pair_a_yaw = -std::f32::consts::FRAC_PI_2;
    let pair_b_yaw = std::f32::consts::FRAC_PI_2;
    assert!(game
        .server
        .world
        .spawn_mob(boat_mob, Vec3::new(6.0, pair_y, pair_z), pair_a_yaw,));
    let pair_a_id = game
        .server
        .world
        .mobs()
        .instances()
        .last()
        .expect("first driven hull was appended")
        .id();
    assert!(game
        .server
        .world
        .spawn_mob(boat_mob, Vec3::new(11.0, pair_y, pair_z), pair_b_yaw,));
    let pair_b_id = game
        .server
        .world
        .mobs()
        .instances()
        .last()
        .expect("second driven hull was appended")
        .id();
    let pair_size = crate::mob::def(boat_mob).size;
    let initial_gap = 5.0;
    let mut final_gap = initial_gap;
    for _ in 0..6 {
        let pair_a = game
            .server
            .world
            .mobs()
            .index_of_id(pair_a_id)
            .expect("first driven hull remains live");
        let pair_b = game
            .server
            .world
            .mobs()
            .index_of_id(pair_b_id)
            .expect("second driven hull remains live");
        assert!(game
            .server
            .world
            .mobs_mut()
            .set_mob_drive(pair_a, 20.0, 0.0, Some(pair_a_yaw)));
        assert!(game
            .server
            .world
            .mobs_mut()
            .set_mob_drive(pair_b, -20.0, 0.0, Some(pair_b_yaw)));
        game.server.game_tick_step(&mut events);

        let mobs = game.server.world.mobs();
        let a = &mobs.instances()[mobs.index_of_id(pair_a_id).unwrap()];
        let b = &mobs.instances()[mobs.index_of_id(pair_b_id).unwrap()];
        final_gap = b.pos.x - a.pos.x;
        assert!(a.pos.x < b.pos.x, "stable hull identities cannot cross");
        assert!(
            crate::mob::body_separation(a.pos, a.yaw, pair_size, b.pos, b.yaw, pair_size).is_none(),
            "committed compound hulls cannot overlap: {:?} {:?}",
            a.pos,
            b.pos,
        );
    }
    let pair_outer_reach = pair_size.half_length.unwrap_or(pair_size.half_width) * 2.0;
    assert!(
        final_gap < initial_gap,
        "both drive intents must actually move the hulls toward each other"
    );
    assert!(
        final_gap <= pair_outer_reach + 1e-3,
        "the repeated drive reaches compound-hull contact: {final_gap}"
    );
    for id in [pair_a_id, pair_b_id] {
        let index = game
            .server
            .world
            .mobs()
            .index_of_id(id)
            .expect("driven test hull remains removable");
        assert!(game.server.world.mobs_mut().remove(index));
    }

    // A second live solid hull is part of the same atomic fit check.
    let spawn_yaw = game.server.sessions[0].player.yaw + std::f32::consts::PI;
    let spawn_pos = Vec3::new(5.5, 63.9, 8.5);
    assert!(game.server.world.spawn_mob(boat_mob, spawn_pos, spawn_yaw));
    let blocker_id = game.server.world.mobs().instances()[0].id();
    game.server.apply_message(0, use_click(water_cell));
    game.server.game_tick_step(&mut events);
    assert_eq!(
        game.server
            .world
            .mobs()
            .instances()
            .iter()
            .filter(|mob| mob.kind == boat_mob)
            .count(),
        1,
        "checked spawn refuses overlap with another solid hull"
    );
    assert_eq!(
        game.server.sessions[0]
            .player
            .inventory
            .selected()
            .map(|stack| (stack.item, stack.count)),
        Some((boat_item, 1)),
        "solid-body rejection also refunds"
    );
    let blocker = game
        .server
        .world
        .mobs()
        .index_of_id(blocker_id)
        .expect("blocker remains until the test removes it");
    assert!(game.server.world.mobs_mut().remove(blocker));

    game.server.apply_message(0, use_click(water_cell));
    game.server.game_tick_step(&mut events);

    let boat_id = game
        .server
        .world
        .mobs()
        .instances()
        .iter()
        .find(|mob| mob.kind == boat_mob && !mob.is_dead())
        .map(|mob| mob.id())
        .expect("a water use launched the boat");
    assert!(
        game.server.sessions[0]
            .player
            .inventory
            .selected()
            .is_none(),
        "placing consumed the held boat item"
    );

    let hull_pose = |game: &super::common::TestGame| {
        let mobs = game.server.world.mobs();
        let mob = &mobs.instances()[mobs.index_of_id(boat_id).expect("boat alive")];
        (mob.pos, mob.yaw)
    };
    let (hull_pos, _) = hull_pose(&game);
    let hull_height = crate::mob::def(boat_mob).size.height;

    // Server placement occupancy and the client prediction mirror both use
    // every hull segment. This thin far-half shape touches the bow but not the
    // former centre square, making the regression precise despite cell
    // alignment.
    let bow_cell = IVec3::new(6, 64, 8);
    let bow_half = [crate::block::Aabb {
        min: [0.5, 0.0, 0.0],
        max: [1.0, 1.0, 1.0],
    }];
    let size = crate::mob::def(boat_mob).size;
    assert!(
        !crate::body::Body::new(hull_pos, size.half_width, size.height)
            .overlaps_block_boxes(bow_cell, &bow_half),
        "the legacy centre square misses this bow-only shape"
    );
    assert!(
        game.server
            .world
            .mobs()
            .any_overlapping_boxes(bow_cell, &bow_half),
        "server placement sees the bow segment"
    );
    let (hull_id, hull_kind, replicated_pos, replicated_yaw) = {
        let mobs = game.server.world.mobs();
        let hull = &mobs.instances()[mobs.index_of_id(boat_id).unwrap()];
        (hull.id(), hull.kind.0, hull.pos, hull.yaw)
    };
    game.game
        .replicated_mobs
        .apply(vec![crate::net::protocol::MobStateRow {
            id: hull_id,
            kind_id: hull_kind,
            pos: replicated_pos,
            yaw: replicated_yaw,
            anim_time: 0.0,
            moving: false,
            idle_anim: None,
            head_yaw: 0.0,
            head_pitch: 0.0,
            hurt_timer: 0.0,
            dead: false,
            shorn: false,
            emitters: Vec::new(),
            anims: Vec::new(),
            ragdoll: None,
        }]);
    assert!(
        game.placement_blocked_by_body(bow_cell, &bow_half),
        "prediction sees the same bow segment"
    );

    aim_at(&mut game, hull_pos + Vec3::new(0.0, hull_height * 0.5, 0.0));
    game.server.apply_message(
        0,
        ClientToServer::Action(PlayerAction::UseClick {
            mob: Some(boat_id),
            target: None,
            request_id: None,
            predicted: false,
            jabbed: true,
        }),
    );
    game.server.game_tick_step(&mut events);

    let player_id = game.server.sessions[0].id.0;
    let mount = game
        .server
        .world
        .riding()
        .mount_of(player_id)
        .expect("an in-reach mob interaction boarded the boat");
    assert_eq!(mount.mob_id, boat_id);

    let start = hull_pose(&game).0;
    game.server.sessions[0].move_wishdir = Vec3::X;
    for _ in 0..16 {
        game.server.game_tick_step(&mut events);
    }
    let (moved, yaw) = hull_pose(&game);
    assert!(moved.x > start.x + 0.1, "driver input moved the hull");

    let seat = crate::mob::def(boat_mob).seats[mount.seat as usize];
    let seat_pos = crate::mob::riding::seat_world_pos(moved, yaw, seat);
    assert!(
        (game.server.sessions[0].player.pos - seat_pos).length() < 1e-3,
        "the rider remained slaved to the declared seat"
    );
    let animated = {
        let mobs = game.server.world.mobs();
        let mob = &mobs.instances()[mobs.index_of_id(boat_id).unwrap()];
        mob.active_anims().iter().any(|layer| layer.phase != 0.0)
    };
    assert!(
        animated,
        "rowing drove at least one authored animation layer"
    );

    game.server.sessions[0].intent_sneak = true;
    game.server.game_tick_step(&mut events);
    assert_eq!(game.server.world.riding().mount_of(player_id), None);
    game.server.sessions[0].intent_sneak = false;
    game.server.sessions[0].move_wishdir = Vec3::ZERO;

    for _ in 0..300 {
        let Some(index) = game.server.world.mobs().index_of_id(boat_id) else {
            break;
        };
        if game.server.world.mobs().instances()[index].is_dead() {
            break;
        }
        let hull = game.server.world.mobs().instances()[index].pos;
        game.server.sessions[0]
            .player
            .teleport(hull + Vec3::new(-2.0, 0.0, 0.0));
        game.server.sessions[0].claim_pos = game.server.sessions[0].player.pos;
        aim_at(&mut game, hull + Vec3::new(0.0, hull_height * 0.5, 0.0));
        game.server.sessions[0].pending_attack = true;
        game.server.sessions[0].pending_attack_mob = Some(boat_id);
        game.server.game_tick_step(&mut events);
    }

    let broken = game
        .server
        .world
        .mobs()
        .index_of_id(boat_id)
        .is_none_or(|index| game.server.world.mobs().instances()[index].is_dead());
    assert!(broken, "ordinary attacks broke the boat");
    assert!(
        game.server
            .world
            .item_entities()
            .iter()
            .any(|drop| drop.stack.item == boat_item),
        "the boat's ordinary loot path returned its item"
    );

    let (disabled, _, _) = game.mods_for_test().probe(0);
    assert!(
        !disabled,
        "vehicles mod stayed healthy through the lifecycle"
    );
}
