use petramond_math::math::Vec3;
use petramond_math::world_pos::WorldPos;
use petramond_world::block::Block;
use petramond_world::chunk::SectionPos;

use crate::events::PostEvent;
use crate::mob::MobTagValue;
use crate::server::game::ServerGame;

const POST_MARKER: &str = "monsters:camp_post";

#[test]
fn a_camp_post_is_manned_out_of_sight_and_its_guard_fights() {
    let Some(root) =
        crate::modding::tests::stage_mods_fixture("camp-guard", &["combat", "monsters"])
    else {
        return;
    };
    crate::modding::tests::run_child_test(
        &root,
        "server::game::tests::camp_guard::a_camp_post_is_manned_out_of_sight_and_its_guard_fights_inner",
    );
}

fn stand(server: &mut ServerGame, at: WorldPos) {
    let sess = &mut server.sessions[0];
    sess.player.pos = at;
    sess.player.vel = Vec3::ZERO;
    sess.input.claim_pos = at;
}

fn tick(server: &mut ServerGame) {
    server.pump_tagged(0.05, &mut Vec::new(), &[]);
}

#[test]
#[ignore = "spawned by a_camp_post_is_manned_out_of_sight_and_its_guard_fights with the packs staged"]
fn a_camp_post_is_manned_out_of_sight_and_its_guard_fights_inner() {
    let mut server = crate::server::session_build::build_server_inline("", 1, 3);
    for _ in 0..60 {
        server.pump_tagged(0.06, &mut Vec::new(), &[]);
    }
    let feet = server.sessions[0].player.pos;
    let (bx, by, bz) = (
        feet.x.floor() as i32,
        feet.y.floor() as i32,
        feet.z.floor() as i32,
    );
    let post = [bx + 26, by - 1, bz];
    let final_at = |server: &ServerGame| {
        server
            .world
            .section_stream_final_at(post[0], post[1] + 1, post[2])
    };
    for _ in 0..400 {
        if final_at(&server) {
            break;
        }
        tick(&mut server);
    }
    assert!(
        final_at(&server),
        "test setup: the post's section streamed in"
    );
    for dx in -4..=32 {
        for dz in -6..=6 {
            let (x, z) = (bx + dx, bz + dz);
            server.world.set_block_world(x, by - 1, z, Block::Stone);
            for dy in 0..=4 {
                let wall = dx == 22;
                let block = if wall { Block::Stone } else { Block::Air };
                server.world.set_block_world(x, by + dy, z, block);
            }
        }
    }
    stand(
        &mut server,
        WorldPos::new(bx as f64 + 0.5, by as f64, bz as f64 + 0.5),
    );
    // `[role, yaw]`, then the camp's box: min and max corners as little-endian i32s.
    let mut marker = vec![0, 0xFF];
    for v in [-4, -2, -6, 4, 6, 6]
        .iter()
        .zip(post.iter().cycle())
        .map(|(d, p)| p + d)
    {
        marker.extend(v.to_le_bytes());
    }
    assert!(server
        .world
        .cell_kv_set(post[0], post[1], post[2], POST_MARKER.into(), marker));
    let section = SectionPos::from_world(post[0], post[1], post[2]).expect("in the world");
    server.mods.emit(PostEvent::SectionLoaded { pos: section });

    let skeleton = crate::mob::by_key("monsters:skeleton").expect("the monsters pack loaded");
    let guard = (0..300)
        .find_map(|_| {
            tick(&mut server);
            server
                .world
                .mobs()
                .instances()
                .iter()
                .find(|m| m.kind == skeleton)
                .map(|m| m.id())
        })
        .expect("the empty post was manned while the player stood behind the wall");
    let man = server.world.mobs().get(guard).expect("alive");
    assert!(
        (man.pos.x - (f64::from(post[0]) + 0.5)).abs() < 0.01
            && (man.pos.z - (f64::from(post[2]) + 0.5)).abs() < 0.01,
        "the guard appeared on its post, at {:?}",
        man.pos
    );
    assert!(
        matches!(man.tags().get("monsters:loadout"), Some(MobTagValue::String(name)) if !name.is_empty()),
        "the guard carries a loadout: {:?}",
        man.tags()
    );
    assert!(man.tags().contains_key("monsters:post"));

    let at = man.pos;
    stand(&mut server, WorldPos::new(at.x + 1.1, at.y, at.z));
    let mut last = server.sessions[0].player.health();
    let mut hits = 0;
    for _ in 0..300 {
        tick(&mut server);
        let health = server.sessions[0].player.health();
        if health < last {
            hits += 1;
            stand(&mut server, WorldPos::new(at.x + 1.1, at.y, at.z));
        }
        last = health;
        if hits == 2 {
            break;
        }
    }
    assert_eq!(
        hits, 2,
        "the guard kept fighting the player standing beside it"
    );
}
