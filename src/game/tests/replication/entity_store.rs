//! Replicated entity-row stores: interpolation pairs, staged windows,
//! overflow resync, and the presentation rows they feed.

use super::common::{game, game_on_empty_chunk};
use super::pump_one_tick;
use crate::entity::DroppedItem;
use crate::game::presentation::GamePresentationScratch;
use crate::game::tick::TICK_DT;
use crate::item::{ItemStack, ItemType};
use crate::mathh::Vec3;
use crate::mob::Mob;
use crate::net::protocol::MobStateRow;

fn mob_row(id: u64, pos: Vec3, hurt_timer: f32) -> MobStateRow {
    MobStateRow {
        id,
        kind_id: Mob::Owl.0,
        pos,
        yaw: 0.0,
        anim_time: 0.0,
        moving: false,
        idle_anim: None,
        head_yaw: 0.0,
        head_pitch: 0.0,
        hurt_timer,
        dead: false,
        shorn: false,
        emitters: Vec::new(),
        anims: Vec::new(),
        ragdoll: None,
    }
}

/// Store semantics: a fresh id starts with prev == curr, a repeated id shifts
/// curr→prev, and an id absent from a batch is dropped.
#[test]
fn replicated_store_pairs_consecutive_batches_and_drops_absent_ids() {
    let mut store = crate::game::replicated::ReplicatedMobs::default();
    let p1 = Vec3::new(1.0, 70.0, 1.0);
    let p2 = Vec3::new(1.5, 69.0, 1.0);

    store.apply(vec![mob_row(7, p1, 0.3), mob_row(9, p1, 0.0)]);
    let fresh = store.iter().find(|e| e.curr.id == 7).expect("stored");
    assert_eq!(fresh.prev.pos, p1, "a fresh id interpolates from itself");
    assert_eq!(store.len(), 2);

    store.apply(vec![mob_row(7, p2, 0.25)]);
    assert_eq!(store.len(), 1, "id 9 was absent from the batch: dropped");
    let paired = store.iter().next().expect("id 7 kept");
    assert_eq!(paired.prev.pos, p1, "previous batch became the prev row");
    assert_eq!(paired.curr.pos, p2);
    assert_eq!(paired.prev.hurt_timer, 0.3);
    assert_eq!(paired.curr.hurt_timer, 0.25);
}

/// A receive burst must fill the FIFO without turning the interpolation
/// window early. Bootstrap is the sole immediate adoption; every later row
/// shifts prev/curr at one crossed render-time boundary, in FIFO order.
#[test]
fn burst_before_a_boundary_does_not_shift_the_committed_pair() {
    use crate::net::protocol::TickUpdate;

    let mut game = game();
    let update = |tick: u64, x: f32| TickUpdate {
        tick,
        mobs: vec![mob_row(7, Vec3::new(x, 70.0, 0.0), 0.0)],
        ..Default::default()
    };

    game.game.apply_tick_update(Box::new(update(1, 1.0)));
    let mob = game.game.replicated_mobs.get(7).expect("bootstrapped");
    assert_eq!(
        (mob.prev.pos.x, mob.curr.pos.x),
        (1.0, 1.0),
        "the first batch seeds both pair slots"
    );

    game.game.replica_clock.advance(TICK_DT * 0.4);
    game.game.apply_tick_update(Box::new(update(2, 2.0)));
    game.game.apply_tick_update(Box::new(update(3, 3.0)));
    assert_eq!(game.game.staged_rows.len(), 2, "the burst queues FIFO");
    let mob = game.game.replicated_mobs.get(7).expect("still committed");
    assert_eq!(
        (mob.prev.pos.x, mob.curr.pos.x),
        (1.0, 1.0),
        "arrivals alone never turn the live interpolation window"
    );

    game.game.replica_clock.advance(TICK_DT * 0.59);
    game.game.advance_interp_window();
    assert_eq!(
        game.game.replicated_mobs.get(7).unwrap().curr.pos.x,
        1.0,
        "the pair stays fixed immediately before the boundary"
    );

    game.game.replica_clock.advance(TICK_DT * 0.02);
    game.game.advance_interp_window();
    let mob = game.game.replicated_mobs.get(7).unwrap();
    assert_eq!((mob.prev.pos.x, mob.curr.pos.x), (1.0, 2.0));
    assert_eq!(
        game.game.staged_rows.len(),
        1,
        "one crossed boundary consumes exactly one queued batch"
    );

    game.game.replica_clock.advance(TICK_DT);
    game.game.advance_interp_window();
    let mob = game.game.replicated_mobs.get(7).unwrap();
    assert_eq!((mob.prev.pos.x, mob.curr.pos.x), (2.0, 3.0));
    assert!(game.game.staged_rows.is_empty());
}

/// If the bounded FIFO overflows, the newest state becomes a declared resync
/// while EVERY queued player action keeps arrival order. Neither the collapse
/// nor the snap may touch committed rows until a boundary; after that, enough
/// crossed boundaries catch up ordinary consecutive snapshots one-for-one.
#[test]
fn staged_overflow_resyncs_at_a_boundary_and_catch_up_stays_one_per_segment() {
    use crate::net::protocol::{ItemStateRow, PlayerActionKind, PlayerStateRow, TickUpdate};
    use crate::server::player::PlayerId;

    let mut game = game();
    let remote_id = PlayerId(7);
    let update = |tick: u64, action: Option<PlayerActionKind>| {
        let x = tick as f32;
        TickUpdate {
            tick,
            mobs: vec![mob_row(7, Vec3::new(x, 70.0, 0.0), 0.0)],
            items: vec![ItemStateRow {
                id: 9,
                item_id: ItemType::Dirt.0,
                count: 1,
                pos: Vec3::new(x, 69.0, 0.0),
                spin: 0.0,
            }],
            players: vec![PlayerStateRow {
                id: remote_id,
                transform: crate::net::protocol::Transform {
                    pos: Vec3::new(x, 68.0, 0.0),
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
                mining: None,
                eating: false,
                hurt_recent: false,
                snap: false,
                mount: None,
            }],
            player_actions: action.into_iter().map(|kind| (remote_id, kind)).collect(),
            ..Default::default()
        }
    };

    game.game.apply_tick_update(Box::new(update(1, None)));
    let action_kinds = [
        PlayerActionKind::Swung,
        PlayerActionKind::Broke,
        PlayerActionKind::Placed,
        PlayerActionKind::ThrewItem,
        PlayerActionKind::UsedItem,
        PlayerActionKind::Interacted,
        PlayerActionKind::AteFinished,
        PlayerActionKind::Died,
        PlayerActionKind::Respawned,
    ];
    // Two full queue depths plus one arrival forces TWO collapses; carrying
    // one distinct enum value per tick also proves every action kind survives
    // a collapse that already contains actions from an earlier collapse.
    let burst_len = crate::game::replicated::MAX_STAGED_ROW_BATCHES * 2 + 1;
    let mut expected_actions = Vec::new();
    for i in 0..burst_len {
        let kind = action_kinds[i % action_kinds.len()];
        expected_actions.push((remote_id, kind));
        game.game
            .apply_tick_update(Box::new(update(i as u64 + 2, Some(kind))));
    }

    assert_eq!(
        game.game.staged_rows.len(),
        1,
        "overflow collapses the pending backlog to its newest snapshot"
    );
    assert_eq!(
        game.game.staged_rows.front().unwrap().actions,
        expected_actions,
        "actions from every collapsed batch survive in arrival order"
    );
    let resync_tick = burst_len as u64 + 1;
    let resync_x = resync_tick as f32;
    assert_eq!(
        game.game.staged_rows.front().unwrap().mobs[0].pos.x,
        resync_x,
        "the retained state is the newest arrival"
    );
    assert_eq!(
        game.game.replicated_mobs.get(7).unwrap().curr.pos.x,
        1.0,
        "overflow itself does not mutate the live pair"
    );

    game.game.replica_clock.advance(TICK_DT * 0.99);
    game.game.advance_interp_window();
    assert_eq!(game.game.replicated_mobs.get(7).unwrap().curr.pos.x, 1.0);
    game.game.replica_clock.advance(TICK_DT * 0.02);
    game.game.advance_interp_window();

    let mob = game.game.replicated_mobs.get(7).unwrap();
    assert_eq!((mob.prev.pos.x, mob.curr.pos.x), (resync_x, resync_x));
    let item = game.game.replicated_items.iter().next().unwrap();
    assert_eq!((item.prev.pos.x, item.curr.pos.x), (resync_x, resync_x));
    let remote = game.game.remote_players.iter().next().unwrap();
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
    game.game.replica_clock.advance(TICK_DT * 2.0);
    game.game.advance_interp_window();
    let mob = game.game.replicated_mobs.get(7).unwrap();
    assert_eq!(
        (mob.prev.pos.x, mob.curr.pos.x),
        (next_tick as f32, (next_tick + 1) as f32),
        "two crossed boundaries catch up two consecutive queued snapshots"
    );
    assert!(game.game.staged_rows.is_empty());
}

/// The riding rubber-band regression (2026-07-15): batch arrivals reach the
/// client quantized to FRAME boundaries, aliasing against the fixed tick —
/// the old arrival-anchored clock turned that into a stall/lurch cycle,
/// violent with the camera glued to a fast mount. The STAGED interpolation
/// window must render an entity moving at constant server velocity with
/// uniform per-frame steps: the committed pair under the render only shifts
/// when render time crosses the segment (see `ReplicaClock`).
#[test]
fn staged_window_renders_uniform_motion_across_frame_aliased_batches() {
    use crate::net::protocol::TickUpdate;
    let mut game = game();

    // A mob gliding +X at exactly 0.2 blocks/tick, one batch per tick, the
    // arrivals quantized UP to a 56 Hz frame grid (56/20 = 2.8 frames per
    // tick — the gaps alias 3/3/2, the worst case for an arrival clock).
    let frame = 1.0 / 56.0;
    let speed = 0.2f32;
    let mut applied = 0u64;
    let sample = |game: &super::common::TestGame| {
        game.game.replicated_mobs.get(7).map(|entry| {
            entry
                .prev
                .pos
                .lerp(entry.curr.pos, game.game.tick_alpha())
                .x
        })
    };
    // Two sample points per frame, mirroring production: the SEND half (what
    // the rider's camera slaves to — the window must turn there too, or the
    // camera stalls one frame every tick) and the RECEIVE half (what
    // presentation renders after the batches drained).
    let mut camera = Vec::new();
    let mut presentation = Vec::new();
    for f in 1..=120u64 {
        let now = f as f32 * frame;
        // Send half: render time advances, the window turns, the slave samples.
        game.game.replica_clock.advance(frame);
        game.game.advance_interp_window();
        camera.extend(sample(&game));
        // Receive half: due batches drain, the window turns again,
        // presentation samples.
        while (applied as f32 + 1.0) * TICK_DT <= now {
            applied += 1;
            let update = TickUpdate {
                tick: applied,
                mobs: vec![mob_row(
                    7,
                    Vec3::new(applied as f32 * speed, 70.0, 0.0),
                    0.0,
                )],
                ..Default::default()
            };
            game.game.apply_tick_update(Box::new(update));
        }
        game.game.advance_interp_window();
        presentation.extend(sample(&game));
    }
    let nominal = speed * frame / TICK_DT;
    // Past the ratchet warm-up, every frame advances BOTH sample sequences by
    // exactly one frame's worth of server motion — no stalls, no snaps.
    for (name, positions) in [("camera", camera), ("presentation", presentation)] {
        let steps: Vec<f32> = positions.windows(2).map(|w| w[1] - w[0]).collect();
        for (i, s) in steps.iter().enumerate().skip(20) {
            assert!(
                (*s - nominal).abs() < nominal * 0.05,
                "uniform {name} velocity (step {i}: {s} vs {nominal})"
            );
        }
    }
}

/// Two pumped batches feed the presentation path: `collect_mobs` reads the
/// REPLICATED store and yields prev/curr rows matching the two batches (the
/// interpolation source the renderer blends), with the replicated kind.
#[test]
fn pumped_mob_batches_become_interpolated_presentation_rows() {
    let mut game = game_on_empty_chunk();
    game.server.sessions[0].player.pos = Vec3::new(8.5, 64.0, 8.5);
    // An owl in free fall right above the player: gravity guarantees its
    // position differs between consecutive ticks.
    assert!(game
        .server
        .world
        .spawn_mob(Mob::Owl, Vec3::new(8.5, 70.0, 8.5), 0.0));
    let id = game.server.world.mobs().instances()[0].id();

    let batch1 = pump_one_tick(&mut game);
    let row1 = batch1
        .mobs
        .iter()
        .find(|m| m.id == id)
        .cloned()
        .expect("the owl replicates");
    game.apply_tick_update(batch1);
    game.commit_replication_window_for_test();

    let batch2 = pump_one_tick(&mut game);
    let row2 = batch2
        .mobs
        .iter()
        .find(|m| m.id == id)
        .cloned()
        .expect("still replicating");
    assert_ne!(row1.pos, row2.pos, "the falling owl moved between ticks");
    game.apply_tick_update(batch2);
    game.commit_replication_window_for_test();

    let mut scratch = GamePresentationScratch::new();
    let presentation = scratch.snapshot(&game, 0.0);
    let row = presentation
        .mobs
        .iter()
        .find(|m| m.id == id)
        .expect("presentation reads the replicated store");
    assert_eq!(row.kind, Mob::Owl);
    assert_eq!(row.prev_pos, row1.pos, "prev = previous batch state");
    assert_eq!(row.pos, row2.pos, "curr = latest batch state");
}

/// A killed/despawned mob vanishes from the next batch, so its id drops from
/// the store and the presentation rows.
#[test]
fn a_despawned_mob_drops_from_the_store_on_the_next_batch() {
    let mut game = game_on_empty_chunk();
    game.server.sessions[0].player.pos = Vec3::new(8.5, 64.0, 8.5);
    assert!(game
        .server
        .world
        .spawn_mob(Mob::Owl, Vec3::new(8.5, 70.0, 8.5), 0.0));
    let id = game.server.world.mobs().instances()[0].id();

    let batch = pump_one_tick(&mut game);
    game.apply_tick_update(batch);
    game.commit_replication_window_for_test();
    assert!(game.replicated_mobs.iter().any(|e| e.curr.id == id));

    let index = game
        .server
        .world
        .mobs()
        .index_of_id(id)
        .expect("still alive server-side");
    assert!(game.server.world.mobs_mut().remove(index));
    let batch = pump_one_tick(&mut game);
    game.apply_tick_update(batch);
    game.commit_replication_window_for_test();

    assert!(
        !game.replicated_mobs.iter().any(|e| e.curr.id == id),
        "an id absent from the batch drops from the store"
    );
    let mut scratch = GamePresentationScratch::new();
    let presentation = scratch.snapshot(&game, 0.0);
    assert!(
        !presentation.mobs.iter().any(|m| m.id == id),
        "and from the presentation rows"
    );
}

/// Dropped items replicate through the same path: batch rows carry the stable
/// per-spawn id, and presentation reads the replicated store.
#[test]
fn dropped_items_replicate_with_stable_ids_into_presentation() {
    let mut game = game_on_empty_chunk();
    // Far from the player so no pickup interferes; above the floor so it moves
    // (falls) between ticks.
    let mut drop = DroppedItem::new(
        Vec3::new(2.5, 70.0, 2.5),
        ItemStack::new(ItemType::Dirt, 3),
        1,
    );
    drop.vel = Vec3::ZERO;
    game.server.world.spawn_item(drop);
    let id = game.server.world.item_entities()[0].id;
    assert_ne!(id, 0, "entering the active set assigns a stable id");

    let batch1 = pump_one_tick(&mut game);
    let row1 = *batch1
        .items
        .iter()
        .find(|i| i.id == id)
        .expect("the drop replicates");
    assert_eq!(row1.item_id, ItemType::Dirt.0);
    assert_eq!(row1.count, 3);
    game.apply_tick_update(batch1);
    game.commit_replication_window_for_test();

    let batch2 = pump_one_tick(&mut game);
    let row2 = *batch2
        .items
        .iter()
        .find(|i| i.id == id)
        .expect("still replicating");
    game.apply_tick_update(batch2);
    game.commit_replication_window_for_test();

    let mut scratch = GamePresentationScratch::new();
    let presentation = scratch.snapshot(&game, 0.0);
    let row = presentation
        .item_entities
        .iter()
        .find(|i| i.item == ItemType::Dirt)
        .expect("presentation reads the replicated store");
    assert_eq!(row.prev_pos, row1.pos, "prev = previous batch state");
    assert_eq!(row.pos, row2.pos, "curr = latest batch state");
    assert_eq!(row.count, 3);
}
