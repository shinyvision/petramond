//! Entity interest end to end through the batch builder: what each
//! recipient's lanes carry as entities and players move, leave, and die.

use crate::events::tick::TickEvents;
use crate::mob::Mob;
use crate::net::protocol::{ItemLane, MobLane, PlayerLane, SleepTally};
use crate::player::PlayerId;
use crate::server::game::ServerGame;
use petramond_math::world_pos::WorldPos;

/// A listen server with a second session parked `far` blocks east of the
/// host. View distance floors at four chunks (64 blocks), so 1000 blocks is
/// out of everybody's view.
fn two_sessions(far: f64) -> (ServerGame, usize) {
    let mut server = crate::server::session_build::build_server_inline("", 1, 2);
    server.sessions[0].player.pos = WorldPos::new(0.5, 65.0, 0.5);
    let remote =
        server.add_session_for_test(crate::player::Player::new(WorldPos::new(far, 65.0, 0.5)));
    (server, remote)
}

/// One recipient's entity sections for a window (an absent section is an
/// empty lane).
struct Batch {
    mobs: MobLane,
    items: ItemLane,
    players: PlayerLane,
    sleep_tally: SleepTally,
}

/// One window's batches, indexed like `sessions`.
fn window(server: &mut ServerGame) -> Vec<Batch> {
    let events = TickEvents::default();
    let shared = server.shared_tick_rows(&events);
    (0..server.sessions.len())
        .map(|s| {
            let update = server.build_tick_update(s, &events, &shared);
            Batch {
                mobs: update.mobs().cloned().unwrap_or_default(),
                items: update.items().cloned().unwrap_or_default(),
                players: update.players().cloned().unwrap_or_default(),
                sleep_tally: *update.sleep_tally().expect("the headcount rides every batch"),
            }
        })
        .collect()
}

fn mob_ids(update: &Batch) -> Vec<u64> {
    update.mobs.iter().map(|row| row.id).collect()
}

fn player_ids(update: &Batch) -> Vec<PlayerId> {
    update.players.iter().map(|row| row.id).collect()
}

fn spawn_sheep(server: &mut ServerGame, x: f64) -> u64 {
    server
        .world
        .spawn_mob(Mob::Sheep, WorldPos::new(x, 65.0, 0.5), 0.0)
        .expect("spawned")
}

fn move_mob(server: &mut ServerGame, id: u64, x: f64) {
    let index = server.world.mobs().index_of_id(id).expect("live mob");
    server
        .world
        .mobs_mut()
        .set_mob_kinematic(
            index,
            WorldPos::new(x, 65.0, 0.5),
            0.0,
            petramond_math::math::Tilt::LEVEL,
        )
        .unwrap();
}

/// Far-apart players pay nothing for each other's surroundings: each batch
/// carries its own mobs, items and self row, never the other side's.
#[test]
fn far_apart_sessions_only_receive_what_is_near_them() {
    let (mut server, remote) = two_sessions(1000.0);
    let near_host = spawn_sheep(&mut server, 8.5);
    let near_remote = spawn_sheep(&mut server, 1008.5);
    let item = server.world.spawn_item(crate::entity::DroppedItem::new(
        WorldPos::new(1002.5, 66.0, 0.5),
        petramond_world::item::ItemStack::new(petramond_world::item::ItemType::Dirt, 1),
        1,
    ));
    let batches = window(&mut server);
    let (host, far) = (&batches[0], &batches[remote]);
    assert_eq!(mob_ids(host), [near_host]);
    assert_eq!(mob_ids(far), [near_remote]);
    assert!(host.items.is_empty());
    assert_eq!(far.items.iter().map(|r| r.id).collect::<Vec<_>>(), [item]);
    assert_eq!(player_ids(host), [server.sessions[0].id], "own row only");
    assert_eq!(player_ids(far), [server.sessions[remote].id]);
    assert_eq!(host.sleep_tally.connected, 2, "the headcount is global");
    // First sight is a spawn; the next window an update.
    assert_eq!(host.mobs.spawned.len(), 1);
    let again = window(&mut server);
    assert!(again[0].mobs.spawned.is_empty());
    assert_eq!(mob_ids(&again[0]), [near_host]);
}

/// Entering view spawns, the hysteresis band keeps, leaving it despawns, and
/// a removed entity despawns wherever it stood.
#[test]
fn mobs_spawn_on_entry_hold_through_the_band_and_despawn_on_exit_or_removal() {
    let (mut server, _) = two_sessions(1000.0);
    let id = spawn_sheep(&mut server, 200.5);
    assert!(mob_ids(&window(&mut server)[0]).is_empty(), "out of view");

    move_mob(&mut server, id, 40.5);
    let host = &window(&mut server)[0];
    assert_eq!(
        host.mobs.spawned.iter().map(|r| r.id).collect::<Vec<_>>(),
        [id]
    );

    // Past the 64-block entry radius, inside the 16-block band: kept.
    move_mob(&mut server, id, 75.5);
    let host = &window(&mut server)[0];
    assert_eq!(
        host.mobs.updated.iter().map(|r| r.id).collect::<Vec<_>>(),
        [id]
    );
    assert!(host.mobs.despawned.is_empty());

    move_mob(&mut server, id, 90.5);
    let host = &window(&mut server)[0];
    assert_eq!(host.mobs.despawned, [id], "left the band: despawned");
    assert!(mob_ids(host).is_empty());

    move_mob(&mut server, id, 10.5);
    assert_eq!(
        window(&mut server)[0].mobs.spawned.len(),
        1,
        "re-entry spawns"
    );
    let index = server.world.mobs().index_of_id(id).unwrap();
    assert!(server.world.mobs_mut().remove(index));
    assert_eq!(
        window(&mut server)[0].mobs.despawned,
        [id],
        "removal despawns"
    );
}

/// Players track each other by view distance, both ways.
#[test]
fn players_enter_and_leave_each_others_interest() {
    let (mut server, remote) = two_sessions(1000.0);
    let remote_id = server.sessions[remote].id;
    assert_eq!(player_ids(&window(&mut server)[0]), [server.sessions[0].id]);

    server.sessions[remote].player.pos = WorldPos::new(20.5, 65.0, 0.5);
    let batches = window(&mut server);
    assert!(player_ids(&batches[0]).contains(&remote_id));
    assert!(player_ids(&batches[remote]).contains(&server.sessions[0].id));

    server.sessions[remote].player.pos = WorldPos::new(1000.5, 65.0, 0.5);
    assert_eq!(window(&mut server)[0].players.despawned, [remote_id]);
}

/// A tracked rider brings its mount, however far the mob itself is from the
/// recipient's view (the rider's seat glue needs the mount's rows).
#[test]
fn a_tracked_riders_mount_is_always_replicated() {
    let (mut server, _) = two_sessions(1000.0);
    let mount = spawn_sheep(&mut server, 500.5);
    server.sessions[0].sim.mount = Some(crate::mob::riding::Mount {
        target: crate::mob::riding::MountTarget::Mob(mount),
        seat: 0,
    });
    assert_eq!(mob_ids(&window(&mut server)[0]), [mount]);
    server.sessions[0].sim.mount = None;
    assert_eq!(window(&mut server)[0].mobs.despawned, [mount]);
}
