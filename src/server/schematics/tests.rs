use super::*;
use crate::events::tick::TickEvents;
use crate::events::{DeferredAction, PostEventKind};
use crate::schematic::share::BlobSender;
use crate::schematic::store::digest;
use crate::schematic::{archive, CellData, Schematic, SchematicCell};
use petramond_math::world_pos::WorldPos;

fn archive_bytes() -> Vec<u8> {
    let png = petramond_world::assets::read_bytes("textures/schematic_wand.png")
        .unwrap()
        .0;
    let cell = |pos| SchematicCell {
        pos,
        data: CellData {
            block: "petramond:stone".into(),
            state: Vec::new(),
            state_ids: Default::default(),
            fluid: 0,
            kv: Default::default(),
            container: None,
            furnace: None,
        },
    };
    let schematic = Schematic::from_cells(
        "Shared".into(),
        [3, 1, 1],
        vec![cell([0, 0, 0]), cell([2, 0, 0])],
    )
    .unwrap();
    archive::encode(&schematic, &png).unwrap()
}

fn server() -> ServerGame {
    let mut server = crate::server::session_build::build_server_inline("", 1, 1);
    server.sessions[0].player.pos = WorldPos::new(8.5, 65.0, 8.5);
    for kind in [
        PostEventKind::SchematicChosen,
        PostEventKind::SchematicPositioned,
    ] {
        server.bus.queue_mut().want_for_test(kind);
    }
    server
}

/// Run ticks of the sharing lane until `done` sees what it waits for.
fn pump_until(server: &mut ServerGame, mut done: impl FnMut(&mut ServerGame) -> bool) {
    let deadline = std::time::Instant::now() + petramond_util::test_time::TEST_HARD_DEADLINE;
    while !done(server) {
        server.tick_schematics();
        assert!(
            std::time::Instant::now() < deadline,
            "the lane never got there"
        );
        std::thread::yield_now();
    }
}

#[test]
fn a_choice_uploads_what_the_world_lacks_then_reports_it() {
    let mut server = server();
    let player = server.sessions[0].id;
    let mut events = TickEvents::default();
    server
        .bus
        .queue_mut()
        .push_action(DeferredAction::SchematicChoose {
            player,
            tag: "fixture:table".into(),
        });
    server.apply_deferred_actions(&mut events);
    assert_eq!(
        server.sessions[0].schematic.take_notices(),
        vec![SchematicNotice::Choose {
            tag: "fixture:table".into()
        }]
    );

    let bytes = archive_bytes();
    let id = digest(&bytes);
    // An answer to a choice nobody opened is ignored.
    server.apply_schematic_request(
        0,
        SchematicRequest::Chosen {
            tag: "fixture:other".into(),
            digest: id,
        },
    );
    assert!(server.sessions[0].schematic.take_notices().is_empty());

    server.apply_schematic_request(
        0,
        SchematicRequest::Chosen {
            tag: "fixture:table".into(),
            digest: id,
        },
    );
    assert_eq!(
        server.sessions[0].schematic.take_notices(),
        vec![SchematicNotice::Want { digest: id }]
    );
    let mut sender = BlobSender::new(id, bytes.into());
    pump_until(&mut server, |server| {
        for packet in sender.packets() {
            server.apply_schematic_request(0, SchematicRequest::Blob(packet));
        }
        for notice in server.sessions[0].schematic.take_notices() {
            if let SchematicNotice::Blob(BlobPacket::Credit { count, .. }) = notice {
                sender.credit(count);
            }
        }
        server
            .bus
            .queue_mut()
            .take_events_for_test()
            .iter()
            .any(|e| matches!(e, PostEvent::SchematicChosen { asset, .. } if *asset == id))
    });
    assert!(server.world.schematics().store.contains(&id));
}

#[test]
fn positioning_reports_only_the_open_request_within_reach() {
    let mut server = server();
    let bytes = archive_bytes();
    let id = digest(&bytes);
    server.world.schematics_mut().store.publish(id, bytes);
    pump_until(&mut server, |server| {
        server.world.schematics().store.contains(&id)
    });
    pump_until(&mut server, |server| {
        matches!(
            server.world.schematics_mut().store.lookup(&id),
            crate::schematic::store::Lookup::Ready(_)
        )
    });
    let player = server.sessions[0].id;
    server.open_schematic_position(player, "fixture:table".into(), id, None, 0);
    let far = SchematicRequest::Positioned {
        tag: "fixture:table".into(),
        digest: id,
        origin: [4000, 65, 8],
        turns: 0,
    };
    server.apply_schematic_request(0, far);
    assert!(server.bus.queue_mut().take_events_for_test().is_empty());
    server.apply_schematic_request(
        0,
        SchematicRequest::Positioned {
            tag: "fixture:table".into(),
            digest: id,
            origin: [9, 65, 8],
            turns: 1,
        },
    );
    assert!(matches!(
        server.bus.queue_mut().take_events_for_test().as_slice(),
        [PostEvent::SchematicPositioned { turns: 1, .. }]
    ));
    server.apply_schematic_request(
        0,
        SchematicRequest::Positioned {
            tag: "fixture:table".into(),
            digest: id,
            origin: [9, 65, 8],
            turns: 1,
        },
    );
    assert!(
        server.bus.queue_mut().take_events_for_test().is_empty(),
        "one positioning answers once"
    );
}

#[test]
fn ghosts_reach_only_their_viewers_and_changes_only_once() {
    let mut server = server();
    let placement = GhostPlacement {
        digest: [3; 32],
        origin: [1, 2, 3],
        turns: 0,
        yields_to_positioning: false,
    };
    let ghost = |viewers| crate::world::schematic::Ghost { placement, viewers };
    server
        .world
        .schematics_mut()
        .ghosts
        .insert("fixture:a".into(), ghost(Vec::new()));
    server
        .world
        .schematics_mut()
        .ghosts
        .insert("fixture:b".into(), ghost(vec![crate::player::PlayerId(99)]));
    server.tick_schematics();
    assert_eq!(
        server.sessions[0].schematic.take_notices(),
        vec![SchematicNotice::Ghost {
            key: "fixture:a".into(),
            placement: Some(placement),
        }]
    );
    server.tick_schematics();
    assert!(server.sessions[0].schematic.take_notices().is_empty());
    server.world.schematics_mut().ghosts.remove("fixture:a");
    server.tick_schematics();
    assert_eq!(
        server.sessions[0].schematic.take_notices(),
        vec![SchematicNotice::Ghost {
            key: "fixture:a".into(),
            placement: None,
        }]
    );
}
