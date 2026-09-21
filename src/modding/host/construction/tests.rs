use mod_api::{
    ActionRefusal, ActorAction, DigProgress, EntityRef, HostCall, HostRet, PlaceRequest,
};
use petramond_math::math::IVec3;
use petramond_math::world_pos::WorldPos;
use petramond_world::block::Block;
use petramond_world::container::Container;
use petramond_world::item::{ItemStack, ItemType};

use crate::events::tick::TickEvents;
use crate::events::{PostEvent, SimCtx};
use crate::modding::host::{handle_host_call, ModStoreData};
use crate::modding::scope;
use crate::server::game::ServerGame;

/// A server with a stone floor at y 64 and open air above, and one mob
/// carrying `slots` standing at (8.5, 65, 8.5).
fn server_with_worker(slots: Vec<Option<ItemStack>>) -> (ServerGame, u64) {
    let mut server = crate::server::session_build::build_server_inline("", 1, 1);
    server
        .bus
        .queue_mut()
        .want_for_test(crate::events::PostEventKind::ActorActed);
    server.sessions[0].player.pos = WorldPos::new(2.5, 65.0, 2.5);
    for x in 4..=14 {
        for y in 64..=69 {
            for z in 4..=14 {
                let block = if y == 64 { Block::Stone } else { Block::Air };
                server.world.set_block_world(x, y, z, block);
            }
        }
    }
    server.world.mobs_mut().restore([crate::mob::SavedMob {
        kind: crate::mob::Mob::Owl,
        pos: WorldPos::new(8.5, 65.0, 8.5),
        yaw: 0.0,
        tags: Default::default(),
        container: Container { slots },
    }]);
    let id = server.world.mobs().instances()[0].id();
    (server, id)
}

fn call(server: &mut ServerGame, call: HostCall) -> HostRet {
    let mut store = ModStoreData::new("construction_test", 1);
    let mut feed = TickEvents::default();
    let ServerGame {
        world,
        sessions,
        bus,
        ..
    } = server;
    let sess = &mut sessions[0];
    let mut ctx = SimCtx {
        world,
        player: &mut sess.player,
        gui_state: &mut sess.gui_state,
        feed: &mut feed,
        queue: bus.queue_mut(),
    };
    scope::enter(&mut ctx, || handle_host_call(&mut store, call))
}

/// Apply the queued actions and return the `actor_acted` outcomes they posted.
fn drain(server: &mut ServerGame) -> Vec<(ActorAction, Option<ActionRefusal>)> {
    let mut events = TickEvents::default();
    server.apply_deferred_actions(&mut events);
    server
        .bus
        .queue_mut()
        .take_events_for_test()
        .into_iter()
        .filter_map(|post| match post {
            PostEvent::ActorActed {
                action, refusal, ..
            } => Some((action, refusal)),
            _ => None,
        })
        .collect()
}

/// Where the worker would look to work `pos` from where it stands.
fn aim(
    server: &mut ServerGame,
    mob: u64,
    pos: IVec3,
    record: Option<mod_api::BlockRecord>,
) -> Result<[f64; 3], ActionRefusal> {
    let feet = server.world.mobs().instances()[0].pos;
    match call(
        server,
        HostCall::ActorAims {
            actor: EntityRef::Mob(mob),
            from: vec![[feet.x, feet.y, feet.z]],
            pos: pos.to_array(),
            record,
        },
    ) {
        HostRet::Aims(aims) => aims[0],
        other => panic!("unexpected aims answer {other:?}"),
    }
}

/// Turn the worker's eyes onto its work at `pos`.
fn look(server: &mut ServerGame, mob: u64, pos: IVec3, record: Option<mod_api::BlockRecord>) {
    let at = aim(server, mob, pos, record).expect("the work is seen from here");
    let eye = server.world.actor(mob).unwrap().eye;
    let to = WorldPos::new(at[0], at[1], at[2]) - eye;
    let index = server.world.mobs().index_of_id(mob).unwrap();
    server.world.mobs_mut().set_gaze_for_test(
        index,
        (-to.x).atan2(-to.z),
        to.y.atan2((to.x * to.x + to.z * to.z).sqrt()),
    );
}

fn stone_record() -> mod_api::BlockRecord {
    mod_api::BlockRecord {
        block: "petramond:cobblestone".into(),
        state: Vec::new(),
        refs: Vec::new(),
        data: Vec::new(),
    }
}

#[test]
fn a_dig_accrues_on_consecutive_ticks_and_collects_its_drop() {
    let (mut server, mob) = server_with_worker(vec![None, None]);
    let target = IVec3::new(10, 65, 8);
    server
        .world
        .set_block_world(target.x, target.y, target.z, Block::Dirt);
    look(&mut server, mob, target, None);
    let dig = |server: &mut ServerGame| {
        call(
            server,
            HostCall::ActorDig {
                actor: EntityRef::Mob(mob),
                pos: target.to_array(),
                tool_slot: None,
                collect: true,
            },
        )
    };
    let mut ticks = 0;
    loop {
        server.world.restore_tick(100 + ticks);
        match dig(&mut server) {
            HostRet::Dig(DigProgress::Digging { progress }) => {
                assert!((0.0..1.0).contains(&progress));
                // A second call in the same tick accrues nothing.
                assert_eq!(
                    dig(&mut server),
                    HostRet::Dig(DigProgress::Digging { progress })
                );
            }
            HostRet::Dig(DigProgress::Breaking) => break,
            other => panic!("unexpected dig answer {other:?}"),
        }
        ticks += 1;
        assert!(ticks < 400, "the dig never finished");
    }
    let expected = petramond_world::mining::break_time(Block::Dirt, None);
    assert!(
        (ticks + 1) as f32 * crate::events::tick::TICK_DT + 1.0e-3 >= expected,
        "the break waited out the block's duration"
    );
    assert_eq!(drain(&mut server), vec![(ActorAction::Dig, None)]);
    assert_eq!(
        Block::from_id(server.world.chunk_block(target.x, target.y, target.z)),
        Block::Air
    );
    let carried = server.world.mobs().instances()[0].container().clone();
    assert_eq!(
        carried.count_like(ItemStack::new(ItemType::from_block(Block::Dirt), 1)),
        1,
        "the drop went into the digger's slots"
    );
}

#[test]
fn an_interrupted_dig_starts_over() {
    let (mut server, mob) = server_with_worker(vec![None]);
    let target = IVec3::new(10, 65, 8);
    server
        .world
        .set_block_world(target.x, target.y, target.z, Block::Stone);
    look(&mut server, mob, target, None);
    let progress_at = |server: &mut ServerGame, tick| {
        server.world.restore_tick(tick);
        match call(
            server,
            HostCall::ActorDig {
                actor: EntityRef::Mob(mob),
                pos: target.to_array(),
                tool_slot: None,
                collect: false,
            },
        ) {
            HostRet::Dig(DigProgress::Digging { progress }) => progress,
            other => panic!("unexpected dig answer {other:?}"),
        }
    };
    let first = progress_at(&mut server, 10);
    let second = progress_at(&mut server, 11);
    assert!(second > first);
    assert_eq!(
        progress_at(&mut server, 13),
        first,
        "a missed tick restarts the dig"
    );
}

#[test]
fn a_placement_pays_once_from_the_actors_slots_and_needs_a_face() {
    let (mut server, mob) = server_with_worker(vec![Some(ItemStack::new(
        ItemType::from_block(Block::Cobblestone),
        2,
    ))]);
    let place = |server: &mut ServerGame, at: IVec3| {
        call(
            server,
            HostCall::ActorPlace {
                actor: EntityRef::Mob(mob),
                pos: at.to_array(),
                record: stone_record(),
                pay: true,
            },
        )
    };
    let on_floor = IVec3::new(10, 65, 9);
    assert_eq!(
        place(&mut server, on_floor),
        HostRet::Place(PlaceRequest::Refused(ActionRefusal::NotAimed)),
        "a block goes in only where its builder is looking"
    );
    look(&mut server, mob, on_floor, Some(stone_record()));
    assert_eq!(
        place(&mut server, on_floor),
        HostRet::Place(PlaceRequest::Queued)
    );
    assert_eq!(drain(&mut server), vec![(ActorAction::Place, None)]);
    assert_eq!(
        Block::from_id(server.world.chunk_block(on_floor.x, on_floor.y, on_floor.z)),
        Block::Cobblestone
    );
    let carried = |server: &ServerGame| {
        server.world.mobs().instances()[0]
            .container()
            .count_like(ItemStack::new(ItemType::from_block(Block::Cobblestone), 1))
    };
    assert_eq!(carried(&server), 1, "exactly one item paid");
    assert_eq!(
        place(&mut server, on_floor),
        HostRet::Place(PlaceRequest::Satisfied),
        "a built record costs nothing again"
    );
    assert_eq!(
        place(&mut server, IVec3::new(8, 68, 11)),
        HostRet::Place(PlaceRequest::Refused(ActionRefusal::NoFace)),
        "nothing to place against in open air"
    );
    assert_eq!(carried(&server), 1);

    // Requested while valid, then emptied before its turn: nothing commits.
    let second = IVec3::new(8, 65, 11);
    look(&mut server, mob, second, Some(stone_record()));
    assert_eq!(
        place(&mut server, second),
        HostRet::Place(PlaceRequest::Queued)
    );
    let index = server.world.mobs().index_of_id(mob).unwrap();
    server.world.mobs_mut().container_mut(index).unwrap().slots[0] = None;
    assert_eq!(
        drain(&mut server),
        vec![(ActorAction::Place, Some(ActionRefusal::MissingItems))]
    );
    assert_eq!(
        Block::from_id(server.world.chunk_block(second.x, second.y, second.z)),
        Block::Air
    );
}

#[test]
fn reach_is_measured_from_the_actors_eye() {
    let (mut server, mob) = server_with_worker(vec![Some(ItemStack::new(
        ItemType::from_block(Block::Cobblestone),
        1,
    ))]);
    let reply = call(
        &mut server,
        HostCall::ActorPlace {
            actor: EntityRef::Mob(mob),
            pos: [14, 65, 14],
            record: stone_record(),
            pay: true,
        },
    );
    assert_eq!(
        reply,
        HostRet::Place(PlaceRequest::Refused(ActionRefusal::OutOfReach))
    );
    assert!(matches!(
        call(
            &mut server,
            HostCall::ActorDig {
                actor: EntityRef::Mob(mob + 1000),
                pos: [9, 64, 8],
                tool_slot: None,
                collect: false,
            },
        ),
        HostRet::Dig(DigProgress::Refused(ActionRefusal::NoActor))
    ));
}

#[test]
fn a_face_turned_away_from_the_eye_is_no_face_to_build_on() {
    let (mut server, mob) = server_with_worker(vec![]);
    // A block overhead, off to one side: the cell on top of it is in plain
    // view, and the only face it could be placed against is the block's top.
    server.world.set_block_world(10, 67, 8, Block::Stone);
    let above = IVec3::new(10, 68, 8);
    assert_eq!(
        aim(&mut server, mob, above, Some(stone_record())),
        Err(ActionRefusal::NoLineOfSight)
    );
    let beside = IVec3::new(9, 67, 8);
    let at = aim(&mut server, mob, beside, Some(stone_record())).expect("its west face is seen");
    assert!(
        (at[0] - 10.0).abs() < 0.05,
        "the look rests on that face, not the cell: {at:?}"
    );
}

#[test]
fn a_turned_block_goes_in_only_from_where_a_click_turns_it_so() {
    use petramond_world::block::CellCodec;
    use petramond_world::block_state::{StairHalf, StairState};
    let (mut server, mob) = server_with_worker(vec![]);
    let stair = |facing| {
        super::record_out(&petramond_world::construction::Record {
            block: Block::OakStairs,
            state: StairState::new(facing, StairHalf::Bottom).to_cell(),
            data: Default::default(),
        })
    };
    // East of the worker on the floor: whatever face it clicks, it looks east.
    let cell = IVec3::new(10, 65, 8);
    let looking = crate::server::placement::facing_from_forward(petramond_math::math::Vec3::new(
        1.0, 0.0, 0.0,
    ));
    let away = crate::server::placement::facing_from_forward(petramond_math::math::Vec3::new(
        -1.0, 0.0, 0.0,
    ));
    assert!(aim(&mut server, mob, cell, Some(stair(looking))).is_ok());
    assert_eq!(
        aim(&mut server, mob, cell, Some(stair(away))),
        Err(ActionRefusal::Misaligned)
    );
}

#[test]
fn a_mob_stepping_aside_under_its_own_power_reads_as_walking() {
    // Through the whole mob tick, with no route to walk: the brain is idle,
    // and an idle route must not take a driven step for a walk that arrived.
    let (mut server, mob) = server_with_worker(vec![]);
    let anchors = [crate::mob::PlayerAnchor {
        pos: WorldPos::new(2.5, 65.0, 2.5),
        ..Default::default()
    }];
    server.world.tick_mobs(0.05, &anchors);
    let stepped = |server: &mut ServerGame, gait| {
        let reply = call(
            server,
            HostCall::MobDrive {
                mob_id: mob,
                horizontal: Some([0.8, 0.0]),
                vertical: None,
                yaw: None,
                while_walking: false,
                gait,
            },
        );
        assert_eq!(reply, HostRet::Bool(true));
        let before = server.world.mobs().instances()[0].pos;
        server.world.tick_mobs(0.05, &anchors);
        let after = &server.world.mobs().instances()[0];
        assert!(after.pos.x > before.x, "the drive moved the body");
        after.moving
    };
    assert!(stepped(&mut server, true), "a step aside plays the walk");
    assert!(
        !stepped(&mut server, false),
        "a body carried along does not"
    );
}

#[test]
fn a_cell_of_two_parts_goes_in_a_click_at_a_time_each_paid_with_its_own_item() {
    use petramond_world::block::CellCodec;
    use petramond_world::block_state::{SlabSplit, SlabState};
    let slab = |block| ItemStack::new(ItemType::from_block(block), 1);
    let (mut server, mob) = server_with_worker(vec![
        Some(slab(Block::DirtSlab)),
        Some(slab(Block::StoneSlab)),
    ]);
    let both = SlabState::single(SlabSplit::Y, 0, Block::DirtSlab)
        .with_slot(1, Block::StoneSlab)
        .unwrap();
    let record = super::record_out(&petramond_world::construction::Record {
        block: petramond_world::slab::representative_block(both),
        state: both.to_cell(),
        data: Default::default(),
    });
    let cell = IVec3::new(10, 65, 8);
    let place = |server: &mut ServerGame| {
        look(server, mob, cell, Some(record.clone()));
        let answer = call(
            server,
            HostCall::ActorPlace {
                actor: EntityRef::Mob(mob),
                pos: cell.to_array(),
                record: record.clone(),
                pay: true,
            },
        );
        assert_eq!(answer, HostRet::Place(PlaceRequest::Queued));
        assert_eq!(drain(server), vec![(ActorAction::Place, None)]);
    };
    let carried = |server: &ServerGame, block| {
        server.world.mobs().instances()[0]
            .container()
            .count_like(slab(block))
    };
    place(&mut server);
    assert_eq!(
        (
            carried(&server, Block::DirtSlab),
            carried(&server, Block::StoneSlab)
        ),
        (0, 1),
        "the first click lays the layer the floor holds up, with that layer's item"
    );
    place(&mut server);
    assert_eq!(carried(&server, Block::StoneSlab), 0);
    assert_eq!(
        server.world.slab_state_at(cell.x, cell.y, cell.z),
        both,
        "two clicks leave the cell as recorded"
    );
}
