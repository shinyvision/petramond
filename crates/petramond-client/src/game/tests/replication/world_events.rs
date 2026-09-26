//! World-event broadcast on the batch: the initiator echo strips for own
//! place/break, the client suppress belt, and multi-batch accumulation.

use super::common::{filled_inventory, game};
use crate::game::tick::TICK_DT;
use petramond_math::world_pos::WorldPos;

/// An UNPREDICTED placement (oriented model, replace-in-place, slab stack,
/// frozen ledger) never presented client-side, so the initiator's own
/// `BlockPlaced` must FLOW — stripping it (the pre-flag behavior) left the
/// place with no hand jab and no sound for the placer.
#[test]
fn unpredicted_placement_keeps_the_initiators_world_event() {
    use petramond::net::protocol::WorldEventMsg;
    use petramond_math::math::IVec3;
    use petramond_world::block::Block;

    let mut game = super::common::game_on_empty_chunk();
    game.server_player_mut().pos = WorldPos::new(3.5, 64.0, 5.5);
    let floor = IVec3::new(3, 63, 3);
    game.server_world_mut()
        .set_block_world(floor.x, floor.y, floor.z, Block::Stone);
    game.sim_mut()
        .mark_section_sent_for_test(0, floor + IVec3::Y);
    game.server_player_mut().inventory = filled_inventory();
    game.session_mut().input_mut().look = Some(super::common::hit(floor, IVec3::Y));
    game.sim_mut().queue_place_click_for_test(0);
    game.session_mut()
        .input_mut()
        .pending_use_click
        .as_mut()
        .expect("click queued")
        .predicted = false; // e.g. a model-block click

    let mut inbox = Vec::new();
    let out = game.sim_mut().pump(TICK_DT, &mut inbox);
    let placed_at = floor + IVec3::Y;
    let initiator = out
        .msgs
        .iter()
        .find_map(|msg| match msg {
            petramond::net::protocol::ServerToClient::Tick(u) => Some(u.as_ref()),
            _ => None,
        })
        .expect("local session batch");
    assert!(
        initiator
            .events()
            .map(Vec::as_slice)
            .unwrap_or(&[])
            .iter()
            .any(|e| matches!(
                e,
                WorldEventMsg::BlockPlaced { pos, .. } if *pos == placed_at
            )),
        "an unpredicted place must keep the initiator's BlockPlaced, got {:?}",
        initiator.events().map(Vec::as_slice).unwrap_or(&[])
    );
}

/// Player block placement and (mined) breaks broadcast position-carrying
/// `WorldEventMsg`s. The initiator's own batch omits their PREDICTED place
/// presentation (echo rule), while a hold-path break they never presented
/// still reaches them; a second session receives both either way.
#[test]
fn placement_and_mined_breaks_broadcast_world_events_with_positions() {
    use petramond::net::protocol::WorldEventMsg;
    use petramond_math::math::IVec3;
    use petramond_world::block::Block;

    let mut game = super::common::game_on_empty_chunk();
    game.server_player_mut().pos = WorldPos::new(3.5, 64.0, 5.5);
    let floor = IVec3::new(3, 63, 3);
    game.server_world_mut()
        .set_block_world(floor.x, floor.y, floor.z, Block::Stone);
    game.server_player_mut().inventory = filled_inventory(); // Dirt in slot 0
    let observer = game
        .sim_mut()
        .add_session_for_test(petramond::player::Player::new(WorldPos::new(
            2.5, 64.0, 2.5,
        )));
    game.sim_mut()
        .mark_section_sent_for_test(0, floor + IVec3::Y);
    game.sim_mut()
        .mark_section_sent_for_test(observer, floor + IVec3::Y);

    // Place: a latched use click against the floor's top face.
    game.session_mut().input_mut().look = Some(super::common::hit(floor, IVec3::Y));
    game.sim_mut().queue_place_click_for_test(0);
    let mut inbox = Vec::new();
    let out = game.sim_mut().pump(TICK_DT, &mut inbox);
    let placed_at = floor + IVec3::Y;
    let initiator = out
        .msgs
        .iter()
        .find_map(|msg| match msg {
            petramond::net::protocol::ServerToClient::Tick(u) => Some(u.as_ref()),
            _ => None,
        })
        .expect("local session batch");
    assert!(
        initiator
            .events()
            .map(Vec::as_slice)
            .unwrap_or(&[])
            .iter()
            .all(|e| !matches!(
                e,
                WorldEventMsg::BlockPlaced { pos, .. } if *pos == placed_at
            )),
        "initiator must not re-hear their own BlockPlaced, got {:?}",
        initiator.events().map(Vec::as_slice).unwrap_or(&[])
    );
    let observer_id = game.session_at(observer).id();
    let observer_batch = out
        .remote
        .iter()
        .find(|(id, _)| *id == observer_id)
        .and_then(|(_, msgs)| {
            msgs.iter().find_map(|msg| match msg {
                petramond::net::protocol::ServerToClient::Tick(u) => Some(u.as_ref()),
                _ => None,
            })
        })
        .expect("observer batch");
    assert!(
        observer_batch
            .events()
            .map(Vec::as_slice)
            .unwrap_or(&[])
            .iter()
            .any(|e| matches!(
                e,
                WorldEventMsg::BlockPlaced { pos, block_id }
                    if *pos == placed_at && *block_id == Block::Dirt.0
            )),
        "observers still receive the placement, got {:?}",
        observer_batch.events().map(Vec::as_slice).unwrap_or(&[])
    );

    // Break: a PURE hold-path finish — no BreakFinished was ever sent, so the
    // client never presented (its timer reset on a sub-tick target flicker,
    // or the break delta cancelled its mining). The initiator MUST receive
    // BlockBroken; stripping it here was the silent-break bug. A predicted
    // finish merely in flight presents once regardless: the client's own
    // suppress belt (`PredictionLedger::mark_presented`) drops the wire copy.
    game.session_mut().input_mut().look = Some(super::common::hit(placed_at, IVec3::Y));
    game.session_mut().input_mut().intent_gameplay = true;
    game.session_mut().input_mut().intent_break_held = true;
    let mut initiator_heard = false;
    let mut observer_heard = false;
    for _ in 0..200 {
        let mut inbox = Vec::new();
        let out = game.sim_mut().pump(TICK_DT, &mut inbox);
        let local = out.msgs.iter().find_map(|msg| match msg {
            petramond::net::protocol::ServerToClient::Tick(u) => Some(u.as_ref()),
            _ => None,
        });
        if let Some(u) = local {
            if u.events()
                .map(Vec::as_slice)
                .unwrap_or(&[])
                .iter()
                .any(|e| {
                    matches!(
                        e,
                        WorldEventMsg::BlockBroken { pos, .. } if *pos == placed_at
                    )
                })
            {
                initiator_heard = true;
            }
            if Block::from_id(game.server_world().data().chunk_block(
                placed_at.x,
                placed_at.y,
                placed_at.z,
            )) == Block::Air
            {
                let obs = out
                    .remote
                    .iter()
                    .find(|(id, _)| *id == observer_id)
                    .and_then(|(_, msgs)| {
                        msgs.iter().find_map(|msg| match msg {
                            petramond::net::protocol::ServerToClient::Tick(u) => Some(u.as_ref()),
                            _ => None,
                        })
                    });
                if let Some(u) = obs {
                    observer_heard = u
                        .events()
                        .map(Vec::as_slice)
                        .unwrap_or(&[])
                        .iter()
                        .any(|e| {
                            matches!(
                                e,
                                WorldEventMsg::BlockBroken { pos, block_id, .. }
                                    if *pos == placed_at && *block_id == Block::Dirt.0
                            )
                        });
                }
                break;
            }
        }
    }
    assert!(
        initiator_heard,
        "a never-presented hold-path break must reach the initiator"
    );
    assert!(
        observer_heard,
        "observers still receive the hold-path break"
    );
}

/// The client-side suppress belt (`PredictionLedger::mark_presented`): with the
/// hold-path no longer stripping on assumption (a never-presented break must
/// flow — the test above), this belt is what keeps a predicted finish whose
/// request is still IN FLIGHT from presenting twice. A wire `BlockBroken`
/// for a cell this client already presented is dropped until its request
/// resolves; any other cell's event assembles normally.
#[test]
fn wire_break_for_a_presented_cell_is_suppressed_while_its_request_is_pending() {
    use petramond::net::protocol::{ServerToClient, TickUpdate, WorldEventMsg};
    use petramond_math::math::IVec3;
    use petramond_world::block::Block;

    let mut game = game();
    let presented = IVec3::new(8, 64, 8);
    let other = IVec3::new(3, 64, 3);
    game.game.prediction.mark_presented(presented);

    let update = TickUpdate {
        tick: 0,
        clock: 0,
        sections: vec![petramond::net::protocol::TickSection::Events(vec![
            WorldEventMsg::BlockBroken {
                pos: presented,
                block_id: Block::Stone.0,
                normal: None,
                tint: None,
            },
            WorldEventMsg::BlockBroken {
                pos: other,
                block_id: Block::Stone.0,
                normal: None,
                tint: None,
            },
        ])],
    };
    game.send_server_message(ServerToClient::Tick(Box::new(update)));

    let events = game.game.tick_receive(TICK_DT);
    let broken: Vec<IVec3> = events
        .world_events
        .iter()
        .filter_map(|e| match e {
            crate::game::tick::WorldEvent::BlockBroken { pos, .. } => Some(*pos),
            _ => None,
        })
        .collect();
    assert_eq!(
        broken,
        vec![other],
        "the presented cell's wire copy is suppressed; the un-presented one flows"
    );
}

/// The self-clocked server thread can outpace a slow frame, so several
/// `TickUpdate`s may drain in ONE client frame. The buffered `ClientEvents`
/// must ACCUMULATE across them — one-shot booleans OR, event queues append in
/// order — never keep only the last batch.
#[test]
fn multiple_tick_updates_in_one_frame_accumulate_not_overwrite() {
    use petramond::net::protocol::{TickUpdate, WorldEventMsg};

    let mut game = super::common::game();

    let first = TickUpdate::new(10, 0)
        .with(petramond::net::protocol::SelfEvents {
            picked_up_item: true,
            ..Default::default()
        })
        .with(vec![WorldEventMsg::ChestOpened {
            pos: petramond_math::math::IVec3::new(1, 65, 1),
        }]);

    let second = TickUpdate::new(11, 0)
        .with(petramond::net::protocol::SelfEvents {
            player_damaged: true,
            ..Default::default()
        })
        .with(vec![WorldEventMsg::ChestClosed {
            pos: petramond_math::math::IVec3::new(1, 65, 1),
        }]);

    game.apply_tick_update(Box::new(first));
    game.apply_tick_update(Box::new(second));

    let ev = &game.game.replica.events;
    assert!(
        ev.self_events.picked_up_item && ev.self_events.player_damaged,
        "one-shots from BOTH batches survive (OR, not overwrite)"
    );
    assert_eq!(
        ev.world,
        vec![
            crate::game::WorldEvent::ChestOpened {
                pos: petramond_math::math::IVec3::new(1, 65, 1)
            },
            crate::game::WorldEvent::ChestClosed {
                pos: petramond_math::math::IVec3::new(1, 65, 1)
            },
        ],
        "world events append in arrival order"
    );
    assert_eq!(game.current_tick(), 11, "latest state wins for the clock");
}
