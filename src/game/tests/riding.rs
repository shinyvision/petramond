//! Cross-subsystem riding boundaries that need the full server fixture.

use super::common::{game, game_on_empty_chunk};
use crate::block::Block;
use crate::mathh::Vec3;
use crate::mob::Mob;

#[test]
fn mounted_autosave_expands_past_blocked_dismount_probes_without_moving_the_rider() {
    let mut game = game_on_empty_chunk();
    let seat = Vec3::new(8.0, 80.0, 8.0);
    assert!(game.server.world.mobs_mut().spawn(Mob::Owl, seat, 0.0));
    let mob_id = game.server.world.mobs().instances()[0].id();
    let player_id = game.server.sessions[0].id.0;
    game.server.sessions[0].player.teleport(seat);
    assert!(game.server.world.riding_mut().mount(player_id, mob_id, 0));
    game.server.sessions[0].mount = game.server.world.riding().mount_of(player_id);

    // Block the seat itself and every ordinary right/left/behind/ahead probe
    // at both heights. The persistence search must expand; falling back to the
    // unchanged seat would restore the detached player inside solid geometry.
    let probes = std::cell::RefCell::new(Vec::new());
    assert_eq!(
        crate::mob::riding::dismount_spot(
            seat,
            0.0,
            |feet| {
                probes.borrow_mut().push(feet);
                false
            },
            |_| true,
        ),
        None
    );
    let ordinary = probes.into_inner();
    assert_eq!(ordinary.len(), 8);
    for feet in ordinary.iter().copied().chain(std::iter::once(seat)) {
        let c = crate::mathh::voxel_at(feet);
        assert!(game
            .server
            .world
            .set_block_world(c.x, c.y, c.z, Block::Stone));
    }
    let obstacles = game.server.world.mobs().solid_obstacles();
    assert!(
        crate::mob::riding::dismount_spot(
            seat,
            0.0,
            |feet| crate::mob::riding::player_body_free(&game.server.world, feet, &obstacles,),
            |_| true,
        )
        .is_none(),
        "the fixture must obstruct all ordinary dismount probes"
    );
    assert!(
        !crate::mob::riding::player_body_free(&game.server.world, seat, &obstacles),
        "the transient seat transform is deliberately unsafe to reload detached"
    );

    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "petramond-mounted-autosave-{}-{nonce}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    let opened = crate::save::open_at(dir.clone()).expect("temp save opens");
    game.server.world.attach_save(opened.save);
    let name = game.server.sessions[0].name.clone();

    game.server.maybe_autosave(30.0);

    let saved = {
        let save = game.server.world.save_mut().expect("save stays attached");
        save.shutdown();
        save.load_player(&name).expect("autosave wrote the player")
    };
    let restored = crate::save::player::decode(&saved)
        .expect("saved player decodes")
        .restore();
    let obstacles = game.server.world.mobs().solid_obstacles();
    assert!(
        crate::mob::riding::player_body_free(&game.server.world, restored.pos, &obstacles),
        "the persisted copy stands clear of the mount: {:?}",
        restored.pos
    );
    assert_ne!(
        restored.pos, seat,
        "the transient seat transform is not saved"
    );
    assert!(
        ordinary.iter().all(|&p| restored.pos != p),
        "the saved copy came from the expanding fallback, not an obstructed ordinary probe"
    );
    assert_eq!(
        game.server.sessions[0].player.pos, seat,
        "autosave never moves the live rider"
    );
    assert!(game.server.world.riding().mount_of(player_id).is_some());
    assert!(game.server.sessions[0].mount.is_some());

    drop(game);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn mounted_snapshot_defers_when_no_terrain_state_is_known() {
    let mut game = game();
    game.server.world.clear_world();
    let seat = Vec3::new(8.0, 80.0, 8.0);
    let player_id = game.server.sessions[0].id.0;
    game.server.sessions[0].player.teleport(seat);
    assert!(game.server.world.riding_mut().mount(player_id, 77, 0));
    game.server.sessions[0].mount = game.server.world.riding().mount_of(player_id);

    assert!(
        game.server.player_snapshot_for_save(0, &[]).is_none(),
        "unloaded or unresolved terrain must defer instead of masquerading as safe air"
    );
    assert_eq!(game.server.sessions[0].player.pos, seat);
    assert!(game.server.world.riding().mount_of(player_id).is_some());
}
