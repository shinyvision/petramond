use super::common::{game, game_on_empty_chunk};
use super::pump_one_tick;
use crate::game::presentation::GamePresentationScratch;
use crate::game::tick::TICK_DT;
use petramond::entity::DroppedItem;
use petramond::mob::Mob;
use petramond::net::protocol::MobStateRow;
use petramond_math::math::Vec3;
use petramond_math::world_pos::WorldPos;
use petramond_world::item::{ItemStack, ItemType};

fn mob_row(id: u64, pos: WorldPos, hurt_timer: f32) -> MobStateRow {
    MobStateRow {
        id,
        kind_id: Mob::Owl.0,
        pos,
        yaw: 0.0,
        tilt: petramond_math::math::Tilt::LEVEL,
        anim_time: 0.0,
        moving: false,
        idle_anim: None,
        head_yaw: 0.0,
        head_pitch: 0.0,
        hurt_timer,
        dead: false,
        shorn: false,
        emitters: Vec::new(),
        conditions: Vec::new(),
        anims: Vec::new(),
        ragdoll: None,
        dig: None,
        held: [None; 2],
        draw: Default::default(),
    }
}

#[test]
fn fire_body_light_composes_and_survives_the_ragdoll_transition() {
    let mut game = game_on_empty_chunk();
    game.set_particles_mode(petramond::save::client::ParticlesMode::Off);
    let mut scratch = GamePresentationScratch::new();
    let view = petramond_render::camera::ViewVolume::unbounded();
    let bundles = ["petramond:burn_great", "petramond:torch_flame"]
        .map(|key| petramond_world::particle_emitters::by_key(key).unwrap());
    let expected = bundles.iter().map(|b| b.body_self_lit).fold(0.0, f32::max);
    let mut row = mob_row(7, WorldPos::new(4.0, 65.0, 4.0), 0.0);
    row.emitters = bundles.iter().map(|b| b.id).collect();
    for dead in [false, true] {
        row.dead = dead;
        row.ragdoll = dead.then(|| vec![([0.0; 3], [0.0, 0.0, 0.0, 1.0])]);
        game.replica
            .entities
            .mobs_mut()
            .apply_snapshot(&[row.clone()]);
        let presentation = scratch.snapshot(&game, 0.0, &view);
        assert!(presentation.particle_emitters.is_empty());
        assert_eq!(presentation.mobs[0].emitter_self_lit, expected);
        assert_eq!(presentation.mobs[0].ragdoll_pose.is_some(), dead);
        row.emitters.reverse();
    }
    row.emitters.clear();
    game.replica.entities.mobs_mut().apply_snapshot(&[row]);
    assert_eq!(
        scratch.snapshot(&game, 0.0, &view).mobs[0].emitter_self_lit,
        0.0
    );
}

#[test]
fn replicated_store_pairs_updates_holds_unmentioned_ids_and_drops_despawns() {
    use petramond::net::protocol::{EntityLane, RowSet};

    let mut store = crate::game::replicated::ReplicatedMobs::default();
    let p1 = WorldPos::new(1.0, 70.0, 1.0);
    let p2 = WorldPos::new(1.5, 69.0, 1.0);

    store.apply(&EntityLane {
        despawned: Vec::new(),
        spawned: vec![mob_row(7, p1, 0.3), mob_row(9, p1, 0.0)].into(),
        updated: RowSet::default(),
    });
    let fresh = store.get(7).expect("stored");
    assert_eq!(fresh.prev.pos, p1, "a spawn interpolates from itself");
    assert_eq!(store.len(), 2);

    store.apply(&vec![mob_row(7, p2, 0.25)].into());
    assert_eq!(store.len(), 2, "id 9 was not despawned: still tracked");
    let paired = store.get(7).expect("id 7 kept");
    assert_eq!(paired.prev.pos, p1, "previous batch became the prev row");
    assert_eq!(paired.curr.pos, p2);
    assert_eq!(paired.prev.hurt_timer, 0.3);
    assert_eq!(paired.curr.hurt_timer, 0.25);

    store.apply(&EntityLane {
        despawned: vec![9],
        spawned: RowSet::default(),
        updated: RowSet::default(),
    });
    assert!(store.get(9).is_none(), "a despawn drops the id");
    let held = store.get(7).expect("id 7 kept");
    assert_eq!(
        (held.prev.pos, held.curr.pos),
        (p2, p2),
        "an unmentioned tracked id holds at its last row"
    );

    store.apply(&EntityLane {
        despawned: Vec::new(),
        spawned: vec![mob_row(7, p1, 0.0)].into(),
        updated: RowSet::default(),
    });
    let reseeded = store.get(7).unwrap();
    assert_eq!((reseeded.prev.pos, reseeded.curr.pos), (p1, p1));
}

#[test]
fn burst_before_a_boundary_does_not_shift_the_committed_pair() {
    use petramond::net::protocol::TickUpdate;

    let mut game = game();
    let update = |tick: u64, x: f32| TickUpdate {
        tick,
        clock: 0,
        sections: vec![petramond::net::protocol::TickSection::Mobs(
            vec![mob_row(7, WorldPos::new(f64::from(x), 70.0, 0.0), 0.0)].into(),
        )],
    };

    game.game.apply_tick_update(Box::new(update(1, 1.0)));
    let mob = game
        .game
        .replica
        .entities
        .mobs()
        .get(7)
        .expect("bootstrapped");
    assert_eq!(
        (mob.prev.pos.x, mob.curr.pos.x),
        (1.0, 1.0),
        "the first batch seeds both pair slots"
    );

    game.game
        .replica
        .entities
        .clock_mut()
        .advance(TICK_DT * 0.4);
    game.game.apply_tick_update(Box::new(update(2, 2.0)));
    game.game.apply_tick_update(Box::new(update(3, 3.0)));
    assert_eq!(
        game.game.replica.entities.staged().len(),
        2,
        "the burst queues FIFO"
    );
    let mob = game
        .game
        .replica
        .entities
        .mobs()
        .get(7)
        .expect("still committed");
    assert_eq!(
        (mob.prev.pos.x, mob.curr.pos.x),
        (1.0, 1.0),
        "arrivals alone never turn the live interpolation window"
    );

    game.game
        .replica
        .entities
        .clock_mut()
        .advance(TICK_DT * 0.59);
    game.game.advance_interp_window();
    assert_eq!(
        game.game.replica.entities.mobs().get(7).unwrap().curr.pos.x,
        1.0,
        "the pair stays fixed immediately before the boundary"
    );

    game.game
        .replica
        .entities
        .clock_mut()
        .advance(TICK_DT * 0.02);
    game.game.advance_interp_window();
    let mob = game.game.replica.entities.mobs().get(7).unwrap();
    assert_eq!((mob.prev.pos.x, mob.curr.pos.x), (1.0, 2.0));
    assert_eq!(
        game.game.replica.entities.staged().len(),
        1,
        "one crossed boundary consumes exactly one queued batch"
    );

    game.game.replica.entities.clock_mut().advance(TICK_DT);
    game.game.advance_interp_window();
    let mob = game.game.replica.entities.mobs().get(7).unwrap();
    assert_eq!((mob.prev.pos.x, mob.curr.pos.x), (2.0, 3.0));
    assert!(game.game.replica.entities.staged().is_empty());
}

#[test]
fn staged_overflow_resyncs_at_a_boundary_and_catch_up_stays_one_per_segment() {
    use petramond::net::protocol::{ItemStateRow, PlayerActionKind, PlayerStateRow, TickUpdate};
    use petramond::player::PlayerId;

    let mut game = game();
    let remote_id = PlayerId(7);
    let update = |tick: u64, action: Option<PlayerActionKind>| {
        let x = tick as f32;
        TickUpdate {
            tick,
            clock: 0,
            sections: vec![
                petramond::net::protocol::TickSection::Mobs(
                    vec![mob_row(7, WorldPos::new(f64::from(x), 70.0, 0.0), 0.0)].into(),
                ),
                petramond::net::protocol::TickSection::Items(
                    vec![ItemStateRow {
                        id: 9,
                        item_id: ItemType::Dirt.0,
                        count: 1,
                        data: None,
                        pos: WorldPos::new(f64::from(x), 69.0, 0.0),
                        spin: 0.0,
                        flight: None,
                    }]
                    .into(),
                ),
                petramond::net::protocol::TickSection::Players(
                    vec![PlayerStateRow {
                        conditions: Vec::new(),
                        id: remote_id,
                        transform: petramond::net::protocol::Transform {
                            pos: WorldPos::new(f64::from(x), 68.0, 0.0),
                            vel: Vec3::ZERO,
                            yaw: 0.0,
                            pitch: 0.0,
                        },
                        on_ground: true,
                        sneaking: false,
                        sleeping: false,
                        sleep_yaw: None,
                        alive: true,
                        visible: true,
                        held_item: None,
                        held_data: None,
                        off_hand_item: None,
                        off_hand_data: None,
                        mining: None,
                        eating: false,
                        eating_off_hand: false,
                        held_pose_main: None,
                        held_pose_off: None,
                        held_display: [None; 2],
                        bone_poses: Vec::new(),
                        animator: Default::default(),
                        hurt_recent: false,
                        snap: false,
                        mount: None,
                    }]
                    .into(),
                ),
                petramond::net::protocol::TickSection::PlayerActions(
                    action.into_iter().map(|kind| (remote_id, kind)).collect(),
                ),
            ],
        }
    };

    game.game.apply_tick_update(Box::new(update(1, None)));
    let action_kinds = [
        PlayerActionKind::Animator {
            rig: petramond::player::RigId(0),
            event: 0,
        },
        PlayerActionKind::Animator {
            rig: petramond::player::RigId(0),
            event: 1,
        },
        PlayerActionKind::Animator {
            rig: petramond::player::RigId(1),
            event: 0,
        },
        PlayerActionKind::Died,
        PlayerActionKind::Respawned,
    ];
    let burst_len = crate::game::replicated::MAX_STAGED_ROW_BATCHES * 2 + 1;
    let mut expected_actions = Vec::new();
    for i in 0..burst_len {
        let kind = action_kinds[i % action_kinds.len()];
        expected_actions.push((remote_id, kind));
        game.game
            .apply_tick_update(Box::new(update(i as u64 + 2, Some(kind))));
    }

    assert_eq!(
        game.game.replica.entities.staged().len(),
        1,
        "overflow collapses the pending backlog to its newest snapshot"
    );
    let collapsed = game.game.replica.entities.staged().front().unwrap();
    let actions: Vec<_> = collapsed
        .windows()
        .flat_map(|window| window.actions.iter().copied())
        .collect();
    assert_eq!(
        actions, expected_actions,
        "actions from every collapsed batch survive in arrival order"
    );
    let resync_tick = burst_len as u64 + 1;
    let resync_x = resync_tick as f64;
    let newest = collapsed.windows().last().unwrap();
    assert_eq!(
        newest.mobs.iter().next().unwrap().pos.x,
        resync_x,
        "the retained state is the newest arrival"
    );
    assert_eq!(
        game.game.replica.entities.mobs().get(7).unwrap().curr.pos.x,
        1.0,
        "overflow itself does not mutate the live pair"
    );

    game.game
        .replica
        .entities
        .clock_mut()
        .advance(TICK_DT * 0.99);
    game.game.advance_interp_window();
    assert_eq!(
        game.game.replica.entities.mobs().get(7).unwrap().curr.pos.x,
        1.0
    );
    game.game
        .replica
        .entities
        .clock_mut()
        .advance(TICK_DT * 0.02);
    game.game.advance_interp_window();

    let mob = game.game.replica.entities.mobs().get(7).unwrap();
    assert_eq!((mob.prev.pos.x, mob.curr.pos.x), (resync_x, resync_x));
    let item = game.game.replica.entities.items().iter().next().unwrap();
    assert_eq!((item.prev.pos.x, item.curr.pos.x), (resync_x, resync_x));
    let remote = game.game.replica.entities.players().iter().next().unwrap();
    assert_eq!(
        (remote.prev.transform.pos.x, remote.curr.transform.pos.x),
        (resync_x, resync_x),
        "the boundary-only resync snaps every replicated row kind"
    );

    let next_tick = resync_tick + 1;
    game.game
        .apply_tick_update(Box::new(update(next_tick, None)));
    game.game
        .apply_tick_update(Box::new(update(next_tick + 1, None)));
    game.game
        .replica
        .entities
        .clock_mut()
        .advance(TICK_DT * 2.0);
    game.game.advance_interp_window();
    let mob = game.game.replica.entities.mobs().get(7).unwrap();
    assert_eq!(
        (mob.prev.pos.x, mob.curr.pos.x),
        (next_tick as f64, (next_tick + 1) as f64),
        "two crossed boundaries catch up two consecutive queued snapshots"
    );
    assert!(game.game.replica.entities.staged().is_empty());
}

/// Batch arrivals reach the client quantized to frame boundaries, aliasing
/// against the fixed tick. The staged interpolation
/// window must render an entity moving at constant server velocity with
/// uniform per-frame steps: the committed pair under the render only shifts
/// when render time crosses the segment (see `ReplicaClock`).
#[test]
fn staged_window_renders_uniform_motion_across_frame_aliased_batches() {
    use petramond::net::protocol::TickUpdate;
    let mut game = game();

    // A mob gliding +X at exactly 0.2 blocks/tick, one batch per tick, the
    // arrivals quantized UP to a 56 Hz frame grid (56/20 = 2.8 frames per
    // tick — the gaps alias 3/3/2, the worst case for an arrival clock).
    let frame = 1.0 / 56.0;
    let speed = 0.2f32;
    let mut applied = 0u64;
    let sample = |game: &super::common::TestGame| {
        game.game.replica.entities.mobs().get(7).map(|entry| {
            entry
                .prev
                .pos
                .lerp(entry.curr.pos, game.game.tick_alpha())
                .x
        })
    };
    let mut camera = Vec::new();
    let mut presentation = Vec::new();
    for f in 1..=120u64 {
        let now = f as f32 * frame;
        game.game.replica.entities.clock_mut().advance(frame);
        game.game.advance_interp_window();
        camera.extend(sample(&game));
        while (applied as f32 + 1.0) * TICK_DT <= now {
            applied += 1;
            let update = TickUpdate {
                tick: applied,
                clock: 0,
                sections: vec![petramond::net::protocol::TickSection::Mobs(
                    vec![mob_row(
                        7,
                        WorldPos::new(f64::from(applied as f32 * speed), 70.0, 0.0),
                        0.0,
                    )]
                    .into(),
                )],
            };
            game.game.apply_tick_update(Box::new(update));
        }
        game.game.advance_interp_window();
        presentation.extend(sample(&game));
    }
    let nominal = speed * frame / TICK_DT;
    for (name, positions) in [("camera", camera), ("presentation", presentation)] {
        let steps: Vec<f32> = positions.windows(2).map(|w| (w[1] - w[0]) as f32).collect();
        for (i, s) in steps.iter().enumerate().skip(20) {
            assert!(
                (*s - nominal).abs() < nominal * 0.05,
                "uniform {name} velocity (step {i}: {s} vs {nominal})"
            );
        }
    }
}

#[test]
fn pumped_mob_batches_become_interpolated_presentation_rows() {
    let mut game = game_on_empty_chunk();
    game.server_player_mut().pos = WorldPos::new(8.5, 64.0, 8.5);
    assert!(game
        .server_world_mut()
        .spawn_mob(Mob::Owl, WorldPos::new(8.5, 70.0, 8.5), 0.0)
        .is_some());
    let id = game.server_world().mobs().instances()[0].id();

    let batch1 = pump_one_tick(&mut game);
    let row1 = batch1
        .mobs()
        .expect("mobs section")
        .iter()
        .find(|m| m.id == id)
        .cloned()
        .expect("the owl replicates");
    game.apply_tick_update(batch1);
    game.commit_replication_window_for_test();

    let batch2 = pump_one_tick(&mut game);
    let row2 = batch2
        .mobs()
        .expect("mobs section")
        .iter()
        .find(|m| m.id == id)
        .cloned()
        .expect("still replicating");
    assert_ne!(row1.pos, row2.pos, "the falling owl moved between ticks");
    game.apply_tick_update(batch2);
    game.commit_replication_window_for_test();

    let mut scratch = GamePresentationScratch::new();
    let presentation = scratch.snapshot(
        &game,
        0.0,
        &petramond_render::camera::ViewVolume::unbounded(),
    );
    let row = presentation
        .mobs
        .iter()
        .find(|m| m.id == id)
        .expect("presentation reads the replicated store");
    assert_eq!(row.kind, Mob::Owl);
    assert_eq!(row.prev_pos, row1.pos, "prev = previous batch state");
    assert_eq!(row.pos, row2.pos, "curr = latest batch state");
}

#[test]
fn a_despawned_mob_drops_from_the_store_on_the_next_batch() {
    let mut game = game_on_empty_chunk();
    game.server_player_mut().pos = WorldPos::new(8.5, 64.0, 8.5);
    assert!(game
        .server_world_mut()
        .spawn_mob(Mob::Owl, WorldPos::new(8.5, 70.0, 8.5), 0.0)
        .is_some());
    let id = game.server_world().mobs().instances()[0].id();

    let batch = pump_one_tick(&mut game);
    game.apply_tick_update(batch);
    game.commit_replication_window_for_test();
    assert!(game.replica.entities.mobs().iter().any(|e| e.curr.id == id));

    assert!(game.server_world_mut().mobs_mut().remove(id));
    let batch = pump_one_tick(&mut game);
    game.apply_tick_update(batch);
    game.commit_replication_window_for_test();

    assert!(
        !game.replica.entities.mobs().iter().any(|e| e.curr.id == id),
        "a removed mob despawns from the store"
    );
    let mut scratch = GamePresentationScratch::new();
    let presentation = scratch.snapshot(
        &game,
        0.0,
        &petramond_render::camera::ViewVolume::unbounded(),
    );
    assert!(
        !presentation.mobs.iter().any(|m| m.id == id),
        "and from the presentation rows"
    );
}

#[test]
fn dropped_items_replicate_with_stable_ids_into_presentation() {
    let mut game = game_on_empty_chunk();
    let mut drop = DroppedItem::new(
        WorldPos::new(2.5, 70.0, 2.5),
        ItemStack::new(ItemType::Dirt, 3),
        1,
    );
    drop.vel = Vec3::ZERO;
    game.server_world_mut().spawn_item(drop);
    let id = game.server_world().item_entities()[0].id;
    assert_ne!(id, 0, "entering the active set assigns a stable id");

    let batch1 = pump_one_tick(&mut game);
    let row1 = batch1
        .items()
        .expect("items section")
        .iter()
        .find(|i| i.id == id)
        .expect("the drop replicates")
        .clone();
    assert_eq!(row1.item_id, ItemType::Dirt.0);
    assert_eq!(row1.count, 3);
    game.apply_tick_update(batch1);
    game.commit_replication_window_for_test();

    let batch2 = pump_one_tick(&mut game);
    let row2 = batch2
        .items()
        .expect("items section")
        .iter()
        .find(|i| i.id == id)
        .expect("still replicating")
        .clone();
    game.apply_tick_update(batch2);
    game.commit_replication_window_for_test();

    let mut scratch = GamePresentationScratch::new();
    let presentation = scratch.snapshot(
        &game,
        0.0,
        &petramond_render::camera::ViewVolume::unbounded(),
    );
    let row = presentation
        .item_entities
        .iter()
        .find(|i| i.item == ItemType::Dirt)
        .expect("presentation reads the replicated store");
    assert_eq!(row.prev_pos, row1.pos, "prev = previous batch state");
    assert_eq!(row.pos, row2.pos, "curr = latest batch state");
    assert_eq!(row.count, 3);
}

#[test]
fn a_mob_draw_set_follows_the_interpolated_body_and_clears() {
    let mut game = game_on_empty_chunk();
    let mut scratch = GamePresentationScratch::new();
    let view = petramond_render::camera::ViewVolume::unbounded();
    let worn = petramond::world::draw::BodyDraw {
        prims: vec![mod_api::DrawPrim::Sprite {
            at: [0.0, 2.0, 0.0],
            scale: 0.5,
            yaw: 0.0,
            pitch: 0.0,
            spin: 0.0,
            bob: [0.1, 4.0],
            faces_viewer: true,
            tile: "stone".into(),
            tint: [255; 3],
            emissive: true,
        }]
        .into(),
        turns: false,
    };
    let mut row = mob_row(7, WorldPos::new(4.25, 65.0, 4.0), 0.0);
    row.draw = worn.clone();
    game.replica
        .entities
        .mobs_mut()
        .apply_snapshot(&[row.clone()]);
    row.pos = WorldPos::new(5.25, 65.0, 4.0);
    game.replica
        .entities
        .mobs_mut()
        .apply_snapshot(&[row.clone()]);

    let presentation = scratch.snapshot(&game, 0.5, &view);
    let [draw] = presentation.block_draws else {
        panic!("one worn set, got {}", presentation.block_draws.len());
    };
    let feet = draw.frame.to_world(Vec3::ZERO);
    let (body, _) = game
        .replica
        .entities
        .mobs()
        .iter()
        .next()
        .unwrap()
        .interpolated_pose(game.tick_alpha());
    assert!((feet.x - body.x).abs() < 1e-4, "{} vs {}", feet.x, body.x);

    row.draw = Default::default();
    game.replica.entities.mobs_mut().apply_snapshot(&[row]);
    assert!(scratch.snapshot(&game, 0.5, &view).block_draws.is_empty());
}
