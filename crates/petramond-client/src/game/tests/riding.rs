//! Cross-subsystem riding boundaries that need the full server fixture.

use super::common::{game, game_on_empty_chunk};
use petramond::mob::Mob;
use petramond_math::world_pos::WorldPos;
use petramond_world::block::Block;

#[test]
fn mounted_autosave_expands_past_blocked_dismount_probes_without_moving_the_rider() {
    let dir = petramond_util::test_dirs::TestScratchDir::new("mounted-autosave");
    let mut game = game_on_empty_chunk();
    let seat = WorldPos::new(8.0, 80.0, 8.0);
    assert!(game
        .server_world_mut()
        .mobs_mut()
        .spawn(Mob::Owl, seat, 0.0));
    let mob_id = game.server_world().mobs().instances()[0].id();
    let player_id = game.session().id().0;
    game.server_player_mut().teleport(seat);
    assert!(game.server_world_mut().riding_mut().mount(
        player_id,
        petramond::mob::riding::MountTarget::Mob(mob_id),
        0
    ));
    let mount = game.server_world().riding().mount_of(player_id);
    game.session_mut().sim_mut().mount = mount;

    // Block the seat itself and every ordinary right/left/behind/ahead probe
    // at both heights. The persistence search must expand; falling back to the
    // unchanged seat would restore the detached player inside solid geometry.
    let probes = std::cell::RefCell::new(Vec::new());
    assert_eq!(
        petramond::mob::riding::dismount_spot(
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
        let c = feet.block();
        assert!(game
            .server_world_mut()
            .set_block_world(c.x, c.y, c.z, Block::Stone));
    }
    let obstacles = game.server_world().mobs().solid_obstacles();
    assert!(
        petramond::mob::riding::dismount_spot(
            seat,
            0.0,
            |feet| petramond::mob::riding::player_body_free(
                game.server_world().data(),
                feet,
                &obstacles,
            ),
            |_| true,
        )
        .is_none(),
        "the fixture must obstruct all ordinary dismount probes"
    );
    assert!(
        !petramond::mob::riding::player_body_free(game.server_world().data(), seat, &obstacles),
        "the transient seat transform is deliberately unsafe to reload detached"
    );

    let opened = petramond::save::open_at(dir.to_path_buf()).expect("temp save opens");
    game.server_world_mut()
        .attach_save(opened.save, opened.saved);
    let key = game.session().key();

    game.sim_mut().maybe_autosave(30.0);

    let saved = {
        let save = game
            .server_world_mut()
            .save_mut()
            .expect("save stays attached");
        save.shutdown();
        save.load_player(&key)
            .expect("saved player decodes")
            .expect("autosave wrote the player")
    };
    let restored = saved.restore();
    let obstacles = game.server_world().mobs().solid_obstacles();
    assert!(
        petramond::mob::riding::player_body_free(
            game.server_world().data(),
            restored.pos,
            &obstacles
        ),
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
        game.server_player().pos,
        seat,
        "autosave never moves the live rider"
    );
    assert!(game.server_world().riding().mount_of(player_id).is_some());
    assert!(game.session().mount().is_some());

    drop(game);
}

#[test]
fn mounted_snapshot_defers_when_no_terrain_state_is_known() {
    let mut game = game();
    game.server_world_mut().clear_world();
    let seat = WorldPos::new(8.0, 80.0, 8.0);
    let player_id = game.session().id().0;
    game.server_player_mut().teleport(seat);
    assert!(game.server_world_mut().riding_mut().mount(
        player_id,
        petramond::mob::riding::MountTarget::Mob(77),
        0
    ));
    let mount = game.server_world().riding().mount_of(player_id);
    game.session_mut().sim_mut().mount = mount;

    assert!(
        game.sim().player_snapshot_for_save(0, &[]).is_none(),
        "unloaded or unresolved terrain must defer instead of masquerading as safe air"
    );
    assert_eq!(game.server_player().pos, seat);
    assert!(game.server_world().riding().mount_of(player_id).is_some());
}
