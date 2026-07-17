//! World-event broadcast on the batch: the initiator echo strips for own
//! place/break, the client suppress belt, and multi-batch accumulation.

use super::common::{filled_inventory, game};
use crate::game::tick::TICK_DT;
use crate::mathh::Vec3;

/// An UNPREDICTED placement (oriented model, replace-in-place, slab stack,
/// frozen ledger) never presented client-side, so the initiator's own
/// `BlockPlaced` must FLOW — stripping it (the pre-flag behavior) left the
/// place with no hand jab and no sound for the placer.
#[test]
fn unpredicted_placement_keeps_the_initiators_world_event() {
    use crate::block::Block;
    use crate::mathh::IVec3;
    use crate::net::protocol::WorldEventMsg;

    let mut game = super::common::game_on_empty_chunk();
    game.server.sessions[0].player.pos = Vec3::new(8.5, 64.0, 8.5);
    let floor = IVec3::new(3, 63, 3);
    game.server
        .world
        .set_block_world(floor.x, floor.y, floor.z, Block::Stone);
    game.server.sessions[0].player.inventory = filled_inventory();
    game.server.sessions[0].look = Some(super::common::hit(floor, IVec3::Y));
    game.server.queue_place_click_for_test(0);
    game.server.sessions[0]
        .pending_use_click
        .as_mut()
        .expect("click queued")
        .predicted = false; // e.g. a model-block click

    let mut inbox = Vec::new();
    let out = game.server.pump(TICK_DT, &mut inbox);
    let placed_at = floor + IVec3::Y;
    let initiator = out
        .msgs
        .iter()
        .find_map(|msg| match msg {
            crate::net::protocol::ServerToClient::Tick(u) => Some(u.as_ref()),
            _ => None,
        })
        .expect("local session batch");
    assert!(
        initiator.events.iter().any(|e| matches!(
            e,
            WorldEventMsg::BlockPlaced { pos, .. } if *pos == placed_at
        )),
        "an unpredicted place must keep the initiator's BlockPlaced, got {:?}",
        initiator.events
    );
}

/// Player block placement and (mined) breaks broadcast position-carrying
/// `WorldEventMsg`s. The initiator's own batch omits their PREDICTED place
/// presentation (echo rule), while a hold-path break they never presented
/// still reaches them; a second session receives both either way.
#[test]
fn placement_and_mined_breaks_broadcast_world_events_with_positions() {
    use crate::block::Block;
    use crate::mathh::IVec3;
    use crate::net::protocol::WorldEventMsg;

    let mut game = super::common::game_on_empty_chunk();
    game.server.sessions[0].player.pos = Vec3::new(8.5, 64.0, 8.5);
    let floor = IVec3::new(3, 63, 3);
    game.server
        .world
        .set_block_world(floor.x, floor.y, floor.z, Block::Stone);
    game.server.sessions[0].player.inventory = filled_inventory(); // Dirt in slot 0
    let observer = game
        .server
        .add_session_for_test(crate::player::Player::new(Vec3::new(2.5, 64.0, 2.5)));

    // Place: a latched use click against the floor's top face.
    game.server.sessions[0].look = Some(super::common::hit(floor, IVec3::Y));
    game.server.queue_place_click_for_test(0);
    let mut inbox = Vec::new();
    let out = game.server.pump(TICK_DT, &mut inbox);
    let placed_at = floor + IVec3::Y;
    let initiator = out
        .msgs
        .iter()
        .find_map(|msg| match msg {
            crate::net::protocol::ServerToClient::Tick(u) => Some(u.as_ref()),
            _ => None,
        })
        .expect("local session batch");
    assert!(
        initiator.events.iter().all(|e| !matches!(
            e,
            WorldEventMsg::BlockPlaced { pos, .. } if *pos == placed_at
        )),
        "initiator must not re-hear their own BlockPlaced, got {:?}",
        initiator.events
    );
    let observer_id = game.server.sessions[observer].id;
    let observer_batch = out
        .remote
        .iter()
        .find(|(id, _)| *id == observer_id)
        .and_then(|(_, msgs)| {
            msgs.iter().find_map(|msg| match msg {
                crate::net::protocol::ServerToClient::Tick(u) => Some(u.as_ref()),
                _ => None,
            })
        })
        .expect("observer batch");
    assert!(
        observer_batch.events.iter().any(|e| matches!(
            e,
            WorldEventMsg::BlockPlaced { pos, block_id }
                if *pos == placed_at && *block_id == Block::Dirt.0
        )),
        "observers still receive the placement, got {:?}",
        observer_batch.events
    );

    // Break: a PURE hold-path finish — no BreakFinished was ever sent, so the
    // client never presented (its timer reset on a sub-tick target flicker,
    // or the break delta cancelled its mining). The initiator MUST receive
    // BlockBroken; stripping it here was the silent-break bug. A predicted
    // finish merely in flight presents once regardless: the client's own
    // suppress belt (`predicted_presentation_cells`) drops the wire copy.
    game.server.sessions[0].look = Some(super::common::hit(placed_at, IVec3::Y));
    game.server.sessions[0].intent_gameplay = true;
    game.server.sessions[0].intent_break_held = true;
    let mut initiator_heard = false;
    let mut observer_heard = false;
    for _ in 0..200 {
        let mut inbox = Vec::new();
        let out = game.server.pump(TICK_DT, &mut inbox);
        let local = out.msgs.iter().find_map(|msg| match msg {
            crate::net::protocol::ServerToClient::Tick(u) => Some(u.as_ref()),
            _ => None,
        });
        if let Some(u) = local {
            if u.events.iter().any(|e| {
                matches!(
                    e,
                    WorldEventMsg::BlockBroken { pos, .. } if *pos == placed_at
                )
            }) {
                initiator_heard = true;
            }
            if Block::from_id(
                game.server
                    .world
                    .chunk_block(placed_at.x, placed_at.y, placed_at.z),
            ) == Block::Air
            {
                let obs = out
                    .remote
                    .iter()
                    .find(|(id, _)| *id == observer_id)
                    .and_then(|(_, msgs)| {
                        msgs.iter().find_map(|msg| match msg {
                            crate::net::protocol::ServerToClient::Tick(u) => Some(u.as_ref()),
                            _ => None,
                        })
                    });
                if let Some(u) = obs {
                    observer_heard = u.events.iter().any(|e| {
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

/// The client-side suppress belt (`predicted_presentation_cells`): with the
/// hold-path no longer stripping on assumption (a never-presented break must
/// flow — the test above), this belt is what keeps a predicted finish whose
/// request is still IN FLIGHT from presenting twice. A wire `BlockBroken`
/// for a cell this client already presented is dropped until its request
/// resolves; any other cell's event assembles normally.
#[test]
fn wire_break_for_a_presented_cell_is_suppressed_while_its_request_is_pending() {
    use crate::block::Block;
    use crate::mathh::IVec3;
    use crate::net::protocol::{ServerToClient, TickUpdate, WorldEventMsg};

    let mut game = game();
    let presented = IVec3::new(8, 64, 8);
    let other = IVec3::new(3, 64, 3);
    game.game.predicted_presentation_cells.insert(presented);

    let update = TickUpdate {
        events: vec![
            WorldEventMsg::BlockBroken {
                pos: presented,
                block_id: Block::Stone.0,
                normal: None,
            },
            WorldEventMsg::BlockBroken {
                pos: other,
                block_id: Block::Stone.0,
                normal: None,
            },
        ],
        ..Default::default()
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
    use crate::net::protocol::{TickUpdate, WorldEventMsg};

    let mut game = super::common::game();

    let mut first = TickUpdate {
        tick: 10,
        ..Default::default()
    };
    first.self_events.picked_up_item = true;
    first.events.push(WorldEventMsg::ChestOpened {
        pos: crate::mathh::IVec3::new(1, 65, 1),
    });

    let mut second = TickUpdate {
        tick: 11,
        ..Default::default()
    };
    second.self_events.player_damaged = true;
    second.events.push(WorldEventMsg::ChestClosed {
        pos: crate::mathh::IVec3::new(1, 65, 1),
    });

    game.apply_tick_update(Box::new(first));
    game.apply_tick_update(Box::new(second));

    let ev = &game.game.pending_events;
    assert!(
        ev.self_events.picked_up_item && ev.self_events.player_damaged,
        "one-shots from BOTH batches survive (OR, not overwrite)"
    );
    assert_eq!(
        ev.world,
        vec![
            crate::game::WorldEvent::ChestOpened {
                pos: crate::mathh::IVec3::new(1, 65, 1)
            },
            crate::game::WorldEvent::ChestClosed {
                pos: crate::mathh::IVec3::new(1, 65, 1)
            },
        ],
        "world events append in arrival order"
    );
    assert_eq!(game.current_tick(), 11, "latest state wins for the clock");
}
