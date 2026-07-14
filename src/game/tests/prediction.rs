//! Optimistic client prediction: ledger rollback, mining deny, movement snap.

use super::common::*;
use crate::block::Block;
use crate::controls::PointerButton;
use crate::game::prediction::{self, PredictionSnapshot};
use crate::game::tick::{GameInput, PlacePrediction, TickEvents, TICK_DT};
use crate::gui::{GuiKind, MenuSlot, WorkbenchHit};
use crate::mathh::{IVec3, Vec3};
use crate::net::protocol::{
    ActionDenyReason, ClientToServer, MenuSlotWire, PlayerAction, SelfTransform, TickUpdate,
};

#[test]
fn menu_click_deny_restores_inventory_snapshot() {
    let mut game = game();
    install_empty_chunk(&mut game);
    game.server.sessions[0].player.inventory = filled_inventory();
    game.sync_self_view_for_test();

    let before_cursor = game.self_view.inventory.cursor().copied();
    game.menu_click(MenuSlot::Inventory(0), PointerButton::Primary, false, false);
    assert!(
        game.self_view.inventory.cursor().is_some(),
        "optimistic pick onto cursor"
    );

    let id = 0;
    let (rollbacks, _) = game
        .prediction
        .reconcile(&[prediction::deny(id, ActionDenyReason::Denied)]);
    assert_eq!(rollbacks.len(), 1);
    match &rollbacks[0] {
        PredictionSnapshot::Inventory(inv) => {
            assert_eq!(inv.cursor().copied(), before_cursor);
        }
        other => panic!("expected inventory snapshot, got {other:?}"),
    }
}

#[test]
fn mixed_menu_drag_prediction_rolls_back_as_one_unit_on_deny() {
    let mut game = game();
    game.game
        .self_view
        .inventory
        .add(crate::item::ItemStack::new(
            crate::item::ItemType::Grass,
            10,
        ));
    game.game.self_view.inventory.click_slot(0);
    game.game.menu_view.chest = Some(crate::gui::ChestView {
        slots: [None; crate::world::chest::CHEST_SLOTS],
    });

    game.game.menu_drag(
        GuiKind::Chest,
        vec![MenuSlot::Inventory(9), MenuSlot::Chest(0)],
        PointerButton::Primary,
    );
    assert!(game.game.self_view.inventory.cursor().is_none());
    assert_eq!(
        game.game
            .self_view
            .inventory
            .slot(9)
            .map(|stack| stack.count),
        Some(5)
    );
    assert_eq!(
        game.game.menu_view.chest.unwrap().slots[0].map(|stack| stack.count),
        Some(5)
    );

    game.game.apply_tick_update(Box::new(TickUpdate {
        action_outcomes: vec![prediction::deny(0, ActionDenyReason::Denied)],
        ..Default::default()
    }));
    assert_eq!(
        game.game
            .self_view
            .inventory
            .cursor()
            .map(|stack| stack.count),
        Some(10)
    );
    assert!(game.game.self_view.inventory.slot(9).is_none());
    assert!(game.game.menu_view.chest.unwrap().slots[0].is_none());
}

#[test]
fn accepted_menu_drag_prediction_reconciles_without_double_applying() {
    let mut game = game();
    game.server.open_workbench_screen_for(0);
    game.server.sessions[0]
        .player
        .inventory
        .add(crate::item::ItemStack::new(
            crate::item::ItemType::Grass,
            10,
        ));
    game.server.sessions[0].player.inventory.click_slot(0);
    game.sync_self_view_for_test();
    game.sync_menu_view_for_test();

    game.game.menu_drag(
        GuiKind::FurnitureWorkbench,
        vec![
            MenuSlot::Inventory(9),
            MenuSlot::Workbench(WorkbenchHit::Input),
        ],
        PointerButton::Primary,
    );
    assert_eq!(game.game.prediction.pending_len(), 1);
    assert!(game.game.self_view.inventory.cursor().is_none());
    assert_eq!(
        game.game
            .self_view
            .inventory
            .slot(9)
            .map(|stack| stack.count),
        Some(5)
    );
    assert_eq!(
        game.game
            .menu_view
            .workbench
            .as_ref()
            .and_then(|workbench| workbench.input)
            .map(|stack| stack.count),
        Some(5)
    );

    game.tick(TICK_DT, &GameInput::default());

    assert_eq!(game.game.prediction.pending_len(), 0);
    assert!(game.game.self_view.inventory.cursor().is_none());
    assert_eq!(
        game.game
            .self_view
            .inventory
            .slot(9)
            .map(|stack| stack.count),
        Some(5)
    );
    assert_eq!(
        game.game
            .menu_view
            .workbench
            .as_ref()
            .and_then(|workbench| workbench.input)
            .map(|stack| stack.count),
        Some(5)
    );
}

#[test]
fn break_finished_without_observed_mining_is_denied() {
    let mut game = game();
    install_empty_chunk(&mut game);
    let pos = IVec3::new(2, 64, 2);
    assert!(game
        .server
        .world
        .set_block_world(pos.x, pos.y, pos.z, Block::Stone));

    game.server.sessions[0].player.pos = Vec3::new(2.5, 65.0, 4.5);
    game.server.sessions[0].claim_pos = game.server.sessions[0].player.pos;

    // Never started mining: the finish is TooFast-deferred, then abandoned
    // in the same tick (no active target) — deny + corrective, no clear.
    game.server.apply_message(
        0,
        ClientToServer::Action(PlayerAction::BreakFinished {
            request_id: 7,
            pos,
            tool_item_id: None,
            predicted: true,
        }),
    );
    let mut ev = TickEvents::default();
    game.server.tick_mining(0, &mut ev);
    assert_eq!(
        Block::from_id(game.server.world.chunk_block(pos.x, pos.y, pos.z)),
        Block::Stone,
        "too-fast break must not clear the cell"
    );
    let outcomes = &game.server.sessions[0].pending_action_outcomes;
    assert_eq!(outcomes.len(), 1);
    assert!(!outcomes[0].accepted);
    assert_eq!(outcomes[0].reason, Some(ActionDenyReason::TooFast));
    assert!(
        game.server.sessions[0]
            .pending_corrective_cells
            .contains(&pos),
        "abandoned TooFast must correct the optimistic clear"
    );
}

#[test]
fn lagged_break_finished_after_hold_path_accepts_without_restore() {
    let mut game = game();
    install_empty_chunk(&mut game);
    let pos = IVec3::new(8, 64, 8);
    assert!(game
        .server
        .world
        .set_block_world(pos.x, pos.y, pos.z, Block::Stone));
    game.server.sessions[0].player.pos = Vec3::new(8.5, 65.0, 10.5);
    game.server.sessions[0].claim_pos = game.server.sessions[0].player.pos;

    let mut u = player_update(&game, true);
    u.break_held = true;
    u.target = Some(hit(pos, IVec3::new(0, 0, 1)));
    game.server
        .apply_message(0, ClientToServer::PlayerUpdate(u));

    // Hold-path clears the cell BEFORE BreakFinished arrives (slow uplink).
    let expected_ticks = (crate::mining::break_time(Block::Stone, None) / TICK_DT).round() as usize;
    for _ in 0..expected_ticks + 2 {
        game.server.tick_mining(0, &mut TickEvents::default());
        if Block::from_id(game.server.world.chunk_block(pos.x, pos.y, pos.z)) == Block::Air {
            break;
        }
    }
    assert_eq!(
        Block::from_id(game.server.world.chunk_block(pos.x, pos.y, pos.z)),
        Block::Air
    );
    assert!(
        game.server.sessions[0].pending_break_ack.contains_key(&pos),
        "hold-path owes a BreakFinished accept"
    );

    game.server.apply_message(
        0,
        ClientToServer::Action(PlayerAction::BreakFinished {
            request_id: 22,
            pos,
            tool_item_id: None,
            predicted: true,
        }),
    );
    game.server.tick_mining(0, &mut TickEvents::default());
    assert_eq!(
        Block::from_id(game.server.world.chunk_block(pos.x, pos.y, pos.z)),
        Block::Air,
        "lagged finish must NOT restore the block"
    );
    assert!(
        game.server.sessions[0]
            .pending_action_outcomes
            .iter()
            .any(|o| o.id == 22 && o.accepted),
        "lagged finish after own hold-path must accept"
    );
    assert!(
        game.server.sessions[0].pending_corrective_cells.is_empty(),
        "accept must not ship corrective cells"
    );
}

#[test]
fn early_break_finished_defers_then_accepts_on_hold_path_without_restore() {
    let mut game = game();
    install_empty_chunk(&mut game);
    let pos = IVec3::new(8, 64, 8);
    assert!(game
        .server
        .world
        .set_block_world(pos.x, pos.y, pos.z, Block::Stone));
    game.server.sessions[0].player.pos = Vec3::new(8.5, 65.0, 10.5);
    game.server.sessions[0].claim_pos = game.server.sessions[0].player.pos;

    // Start the server's observed mining window.
    let mut u = player_update(&game, true);
    u.break_held = true;
    u.target = Some(hit(pos, IVec3::new(0, 0, 1)));
    game.server
        .apply_message(0, ClientToServer::PlayerUpdate(u));

    // One tick of progress — far short of stone's break time.
    game.server.tick_mining(0, &mut TickEvents::default());
    game.server.apply_message(
        0,
        ClientToServer::Action(PlayerAction::BreakFinished {
            request_id: 11,
            pos,
            tool_item_id: None,
            predicted: true,
        }),
    );
    game.server.tick_mining(0, &mut TickEvents::default());
    assert!(
        game.server.sessions[0].pending_action_outcomes.is_empty(),
        "TooFast while mining must defer, not deny (no restore)"
    );
    assert!(
        game.server.sessions[0].deferred_break_finished.is_some(),
        "the finish waits for the hold-path"
    );
    assert!(
        game.server.sessions[0].pending_corrective_cells.is_empty(),
        "deferred TooFast must not ship corrective cells"
    );
    assert_eq!(
        Block::from_id(game.server.world.chunk_block(pos.x, pos.y, pos.z)),
        Block::Stone,
        "server cell stays until the hold-path finishes"
    );

    // Hold until the server's timer breaks the block.
    let expected_ticks = (crate::mining::break_time(Block::Stone, None) / TICK_DT).round() as usize;
    for _ in 0..expected_ticks + 2 {
        game.server.tick_mining(0, &mut TickEvents::default());
        if Block::from_id(game.server.world.chunk_block(pos.x, pos.y, pos.z)) == Block::Air {
            break;
        }
    }
    assert_eq!(
        Block::from_id(game.server.world.chunk_block(pos.x, pos.y, pos.z)),
        Block::Air,
        "hold-path clears the cell"
    );
    let outcomes = &game.server.sessions[0].pending_action_outcomes;
    assert!(
        outcomes.iter().any(|o| o.id == 11 && o.accepted),
        "deferred finish accepts when the hold-path breaks, got {outcomes:?}"
    );
    assert!(
        game.server.sessions[0].presented_breaks.contains(&pos),
        "a deferred PREDICTED finish strips the initiator's BlockBroken"
    );
}

#[test]
fn break_finished_after_the_observed_mining_window_is_accepted() {
    let mut game = game();
    install_empty_chunk(&mut game);
    let pos = IVec3::new(8, 64, 8);
    assert!(game
        .server
        .world
        .set_block_world(pos.x, pos.y, pos.z, Block::Stone));
    game.server.sessions[0].player.pos = Vec3::new(8.5, 65.0, 10.5);

    // Latch a held break on the target: the server's own mining timer is the
    // observation the finish is validated against.
    let mut u = player_update(&game, true);
    u.break_held = true;
    u.target = Some(hit(pos, IVec3::new(0, 0, 1)));
    game.server
        .apply_message(0, ClientToServer::PlayerUpdate(u));

    let expected_ticks = (crate::mining::break_time(Block::Stone, None) / TICK_DT).round() as usize;
    // Hold just short of the server's own finish, then deliver the client's.
    for _ in 0..expected_ticks - 2 {
        game.server.tick_mining(0, &mut TickEvents::default());
    }
    game.server.apply_message(
        0,
        ClientToServer::Action(PlayerAction::BreakFinished {
            request_id: 6,
            pos,
            tool_item_id: None,
            predicted: true,
        }),
    );
    game.server.tick_mining(0, &mut TickEvents::default());
    assert_eq!(
        Block::from_id(game.server.world.chunk_block(pos.x, pos.y, pos.z)),
        Block::Air,
        "an observed full mining window accepts the client's finish"
    );
    let outcomes = &game.server.sessions[0].pending_action_outcomes;
    assert!(outcomes.iter().any(|o| o.id == 6 && o.accepted));
}

#[test]
fn impossible_speed_claim_is_rejected_for_integrated_pos() {
    let mut game = game();
    install_empty_chunk(&mut game);
    let start = game.server.sessions[0].player.pos;
    let mut u = player_update(&game, true);
    u.pos = start + Vec3::new(50.0, 0.0, 0.0);
    u.vel = Vec3::new(200.0, 0.0, 0.0);
    u.wishdir = Vec3::ZERO;
    game.server
        .apply_message(0, ClientToServer::PlayerUpdate(u));
    game.server.tick_movement(0);
    let after = game.server.sessions[0].player.pos;
    assert!(
        (after - start).length() < 5.0,
        "server must not adopt an impossible speed claim (start={start:?} after={after:?})"
    );
}

#[test]
fn teleport_claim_with_plausible_velocity_is_rejected() {
    let mut game = game();
    install_empty_chunk(&mut game);
    game.server.sessions[0].player.pos = Vec3::new(8.5, 70.0, 8.5);
    let start = game.server.sessions[0].player.pos;
    let mut u = player_update(&game, true);
    u.pos = start + Vec3::new(50.0, 0.0, 0.0);
    u.vel = Vec3::ZERO;
    u.wishdir = Vec3::ZERO;
    game.server
        .apply_message(0, ClientToServer::PlayerUpdate(u));
    game.server.tick_movement(0);
    let after = game.server.sessions[0].player.pos;
    assert!(
        (after - start).length() < 5.0,
        "a position jump under an innocent velocity must not teleport (after={after:?})"
    );
}

#[test]
fn sprint_jump_claim_is_accepted() {
    let mut game = game();
    install_empty_chunk(&mut game);
    game.server.sessions[0].player.pos = Vec3::new(8.5, 70.0, 8.5);
    let start = game.server.sessions[0].player.pos;
    let mut u = player_update(&game, true);
    // Take-off frame of a sprint jump: horizontal sprint + full jump speed.
    // The caps are per-axis, so the combined magnitude must still pass.
    u.pos = start + Vec3::new(0.2, 0.0, 0.0);
    u.vel = Vec3::new(5.6, 8.4, 0.0);
    u.on_ground = false;
    game.server
        .apply_message(0, ClientToServer::PlayerUpdate(u.clone()));
    game.server.tick_movement(0);
    let sess = &game.server.sessions[0];
    assert_eq!(
        sess.player.pos, u.pos,
        "a legitimate sprint-jump claim must be soft-accepted"
    );
    assert_eq!(sess.player.vel, u.vel);
}

#[test]
fn claim_inside_solid_geometry_is_rejected() {
    let mut game = game();
    install_empty_chunk(&mut game);
    assert!(game.server.world.set_block_world(8, 64, 8, Block::Stone));
    game.server.sessions[0].player.pos = Vec3::new(8.5, 66.0, 8.5);
    let mut u = player_update(&game, true);
    u.pos = Vec3::new(8.5, 64.3, 8.5); // feet well inside the stone cell
    u.vel = Vec3::ZERO;
    game.server
        .apply_message(0, ClientToServer::PlayerUpdate(u));
    game.server.tick_movement(0);
    let after = game.server.sessions[0].player.pos;
    assert!(
        after.y > 65.0,
        "a claim inside solid geometry must not be adopted (after={after:?})"
    );
}

#[test]
fn each_queued_drop_in_one_tick_window_gets_its_own_outcome() {
    let mut game = game();
    install_empty_chunk(&mut game);
    game.server.sessions[0].player.inventory = filled_inventory();
    game.server.apply_message(
        0,
        ClientToServer::Action(PlayerAction::Drop {
            all: false,
            request_id: 11,
        }),
    );
    game.server.apply_message(
        0,
        ClientToServer::Action(PlayerAction::Drop {
            all: false,
            request_id: 12,
        }),
    );
    // Nothing on the cursor: the throw cannot even queue, denied immediately.
    game.server.apply_message(
        0,
        ClientToServer::Action(PlayerAction::ThrowCursorOne { request_id: 13 }),
    );
    let mut ev = TickEvents::default();
    game.server.tick_drops(0, &mut ev);
    let outcomes = &game.server.sessions[0].pending_action_outcomes;
    assert_eq!(
        outcomes.len(),
        3,
        "every request id is answered, even coalesced or unqueueable ones"
    );
    assert!(outcomes.iter().any(|o| o.id == 11 && o.accepted));
    assert!(outcomes.iter().any(|o| o.id == 12 && o.accepted));
    assert!(outcomes.iter().any(|o| o.id == 13 && !o.accepted));
}

#[test]
fn multi_deny_rollback_restores_the_oldest_snapshot() {
    let mut game = game();
    install_empty_chunk(&mut game);
    game.server.sessions[0].player.inventory = filled_inventory();
    game.sync_self_view_for_test();
    let before = game.self_view.inventory.clone();

    // Two predicted drops back to back: the second snapshot already embeds
    // the first prediction's effect.
    game.game.drop_selected_item(false); // id 0
    game.game.drop_selected_item(false); // id 1

    // Both denied in one batch: the restore must end on the OLDEST snapshot.
    let update = TickUpdate {
        action_outcomes: vec![
            prediction::deny(0, ActionDenyReason::Denied),
            prediction::deny(1, ActionDenyReason::Denied),
        ],
        ..Default::default()
    };
    game.game.apply_tick_update(Box::new(update));
    assert_eq!(
        game.self_view
            .inventory
            .slot(game.self_view.inventory.active_slot() as usize),
        before.slot(before.active_slot() as usize),
        "both denied predictions must be rolled back, not just the newest"
    );
}

#[test]
fn denied_cell_rollback_yields_to_a_same_batch_authoritative_delta() {
    let mut game = game();
    // A loaded replica cell the ghost writes into.
    let pos = IVec3::new(3, 64, 3);
    game.game.replica.insert_chunk_for_test(
        crate::chunk::ChunkPos::new(0, 0),
        crate::chunk::Chunk::new(0, 0),
    );

    // Predict a ghost placement (World snapshot, prev = air).
    let id = game.game.prediction.begin(PredictionSnapshot::World {
        inventory: None,
        cells: vec![(pos, Block::Air.0)],
    });
    assert!(game
        .game
        .replica
        .set_block_world(pos.x, pos.y, pos.z, Block::Dirt));

    // Same batch: the deny AND an authoritative delta at the cell (another
    // player's block won it). The delta must survive the rollback.
    let update = TickUpdate {
        block_deltas: vec![crate::net::protocol::BlockDelta {
            pos,
            block_id: Block::Stone.0,
            water: None,
            state: None,
        }],
        action_outcomes: vec![prediction::deny(id, ActionDenyReason::Denied)],
        ..Default::default()
    };
    game.game.apply_tick_update(Box::new(update));
    assert_eq!(
        Block::from_id(game.game.replica.chunk_block(pos.x, pos.y, pos.z)),
        Block::Stone,
        "an authoritative same-batch delta wins over the deny rollback"
    );
}

#[test]
fn place_resolves_at_the_click_target_not_the_freshest_look() {
    let mut game = game();
    install_empty_chunk(&mut game);
    let a = IVec3::new(8, 63, 8);
    let b = IVec3::new(11, 63, 11);
    assert!(game
        .server
        .world
        .set_block_world(a.x, a.y, a.z, Block::Stone));
    assert!(game
        .server
        .world
        .set_block_world(b.x, b.y, b.z, Block::Stone));
    game.server.sessions[0].player.pos = Vec3::new(9.5, 63.0, 9.5);
    game.server.sessions[0].player.inventory = filled_inventory(); // dirt

    // Click aimed at A...
    let mut u = player_update(&game, true);
    u.target = Some(hit(a, IVec3::Y));
    game.server
        .apply_message(0, ClientToServer::PlayerUpdate(u));
    game.server.apply_message(
        0,
        ClientToServer::Action(PlayerAction::UseClick {
            mob: None,
            target: Some(hit(a, IVec3::Y)),
            request_id: Some(3),
            predicted: true,
            jabbed: false,
        }),
    );
    // ...then the crosshair moves to B before the tick resolves the click.
    let mut u2 = player_update(&game, true);
    u2.target = Some(hit(b, IVec3::Y));
    game.server
        .apply_message(0, ClientToServer::PlayerUpdate(u2));

    game.server.tick_place(0, &mut TickEvents::default());
    assert_eq!(
        Block::from_id(game.server.world.chunk_block(a.x, a.y + 1, a.z)),
        Block::Dirt,
        "the block lands where the CLICK aimed (the client's ghost)"
    );
    assert_eq!(
        Block::from_id(game.server.world.chunk_block(b.x, b.y + 1, b.z)),
        Block::Air,
        "the fresher look must not hijack the click"
    );
    let outcomes = &game.server.sessions[0].pending_action_outcomes;
    assert!(outcomes.iter().any(|o| o.id == 3 && o.accepted));
}

#[test]
fn no_op_use_click_queues_the_disputed_cells_for_corrective_sync() {
    let mut game = game();
    install_empty_chunk(&mut game);
    let t = IVec3::new(8, 64, 8);
    assert!(game
        .server
        .world
        .set_block_world(t.x, t.y, t.z, Block::Stone));
    game.server.sessions[0].player.pos = Vec3::new(8.5, 65.5, 10.5);

    // Empty hand, non-interactable stone: the server consumes nothing — the
    // client may have clicked a cell that only exists in ITS replica, so the
    // authoritative state of the disputed cells ships back.
    let mut u = player_update(&game, true);
    u.target = Some(hit(t, IVec3::Y));
    game.server
        .apply_message(0, ClientToServer::PlayerUpdate(u));
    game.server.apply_message(
        0,
        ClientToServer::Action(PlayerAction::UseClick {
            mob: None,
            target: Some(hit(t, IVec3::Y)),
            request_id: None,
            predicted: false,
            jabbed: false,
        }),
    );
    game.server.tick_place(0, &mut TickEvents::default());
    let cells = &game.server.sessions[0].pending_corrective_cells;
    assert!(cells.contains(&t), "the clicked cell reconciles");
    assert!(
        cells.contains(&(t + IVec3::Y)),
        "the would-be place cell reconciles"
    );
}

#[test]
fn claim_after_a_slow_client_gap_is_accepted() {
    let mut game = game();
    install_empty_chunk(&mut game);
    game.server.sessions[0].player.pos = Vec3::new(8.5, 70.0, 8.5);
    let start = game.server.sessions[0].player.pos;
    let mut u = player_update(&game, true);
    u.pos = start;
    u.vel = Vec3::ZERO;
    game.server
        .apply_message(0, ClientToServer::PlayerUpdate(u));
    game.server.tick_movement(0);
    // A slow client: four ticks free-run with no fresh claim.
    for _ in 0..4 {
        game.server.tick_movement(0);
    }
    // Its next report legitimately drifted further than one frame's worth.
    let mut u2 = player_update(&game, true);
    u2.pos = start + Vec3::new(3.5, 0.0, 0.0);
    u2.vel = Vec3::new(5.6, 0.0, 0.0);
    game.server
        .apply_message(0, ClientToServer::PlayerUpdate(u2.clone()));
    game.server.tick_movement(0);
    assert_eq!(
        game.server.sessions[0].player.pos, u2.pos,
        "the closeness ring scales with the claim gap — no rubber-banding"
    );
}

#[test]
fn transform_corrections_ship_only_on_real_divergence() {
    let mut game = game();
    install_empty_chunk(&mut game);
    let sess = &mut game.server.sessions[0];
    sess.player.pos = Vec3::new(8.5, 70.0, 8.5);
    sess.player.vel = Vec3::new(0.0, -1.4, 0.0);
    // The server free-ran a little past the client's last claim: small pos
    // phase drift, one tick of gravity — time-phase, not divergence.
    sess.last_reported_transform = Some(SelfTransform {
        pos: sess.player.pos + Vec3::new(0.4, 0.5, 0.0),
        vel: Vec3::ZERO,
        yaw: sess.player.yaw,
        pitch: sess.player.pitch,
        on_ground: sess.player.on_ground,
    });
    assert!(
        game.server.build_self_state(0).transform.is_none(),
        "extrapolation past the claim must not rubber-band the client"
    );

    // A genuine tick-side teleport still corrects.
    game.server.sessions[0].player.pos += Vec3::new(50.0, 0.0, 0.0);
    assert!(
        game.server.build_self_state(0).transform.is_some(),
        "a real teleport ships a SelfTransform"
    );
}

#[test]
fn hotbar_selection_is_client_owned_and_never_yanked_by_a_batch() {
    let mut game = game();
    game.server.sessions[0].player.inventory = filled_inventory();

    // The client scrolls ahead of the server (which still thinks slot 0)...
    game.game.set_active_hotbar(3);
    assert_eq!(game.self_view.inventory.active_slot(), 3);

    // ...and a full-inventory batch from the lagging server must keep the
    // client's newer selection, not echo the stale one back.
    game.sync_self_view_for_test();
    assert_eq!(
        game.self_view.inventory.active_slot(),
        3,
        "a server batch must never yank the client-owned hotbar selection"
    );
}

#[test]
fn menu_click_ships_request_id_and_server_accepts() {
    let mut game = game();
    game.server.sessions[0].player.inventory = filled_inventory();
    game.server.apply_message(
        0,
        ClientToServer::MenuClick {
            slot: MenuSlotWire::from_menu_slot(&MenuSlot::Inventory(0)),
            button: 0,
            shift: false,
            gather: false,
            request_id: 42,
        },
    );
    game.server.tick_menu(0, &mut TickEvents::default());
    let outcomes = &game.server.sessions[0].pending_action_outcomes;
    assert_eq!(outcomes.len(), 1);
    assert!(outcomes[0].accepted);
    assert_eq!(outcomes[0].id, 42);
}

#[test]
fn optimistic_place_mutates_replica_hotbar_and_queues_world_event() {
    let mut game = game();
    install_empty_chunk(&mut game);
    // Mirror the chunk onto the replica so the place ghost can write.
    game.game.replica.insert_chunk_for_test(
        crate::chunk::ChunkPos::new(0, 0),
        crate::chunk::Chunk::new(0, 0),
    );
    let floor = IVec3::new(8, 63, 8);
    assert!(game
        .game
        .replica
        .set_block_world(floor.x, floor.y, floor.z, Block::Stone));
    // Park the body clear of the place cell so placement_blocked_by_body
    // does not refuse the ghost.
    game.game.player.pos = Vec3::new(100.0, 64.0, 100.0);
    game.server.sessions[0].player.inventory = filled_inventory();
    game.sync_self_view_for_test();
    let before = game
        .self_view
        .inventory
        .selected()
        .expect("holding dirt")
        .count;

    assert!(matches!(
        game.game.predict_place_at_for_test(floor, IVec3::Y, false),
        PlacePrediction::Predicted(_)
    ));

    let place_pos = floor + IVec3::Y;
    assert_eq!(
        Block::from_id(
            game.game
                .replica
                .chunk_block(place_pos.x, place_pos.y, place_pos.z)
        ),
        Block::Dirt,
        "replica cell must change immediately"
    );
    assert_eq!(
        game.self_view
            .inventory
            .selected()
            .expect("still holding")
            .count,
        before - 1,
        "hotbar decrements with the ghost"
    );
    assert!(
        game.game.pending_events.world.iter().any(
            |e| matches!(e, crate::game::tick::WorldEvent::BlockPlaced { pos, block }
                if *pos == place_pos && *block == Block::Dirt)
        ),
        "local BlockPlaced must queue for sound this frame"
    );
    assert_eq!(game.game.local_placed_block, Some(Block::Dirt));
}

#[test]
fn optimistic_torch_place_records_wall_mount_immediately() {
    let mut game = game();
    install_empty_chunk(&mut game);
    game.game.replica.insert_chunk_for_test(
        crate::chunk::ChunkPos::new(0, 0),
        crate::chunk::Chunk::new(0, 0),
    );
    let wall = IVec3::new(8, 64, 8);
    assert!(game
        .game
        .replica
        .set_block_world(wall.x, wall.y, wall.z, Block::Stone));
    game.game.player.pos = Vec3::new(100.0, 64.0, 100.0);
    let mut inv = crate::inventory::Inventory::new();
    inv.add(crate::item::ItemStack::new(crate::item::ItemType::Torch, 1));
    game.server.sessions[0].player.inventory = inv;
    game.sync_self_view_for_test();

    // Click the wall's west face: the predicted torch must carry its mount
    // BEFORE the frame's remesh, or it renders the Floor default until the
    // authoritative delta lands (the one-frame floor-torch flicker).
    assert!(matches!(
        game.game.predict_place_at_for_test(wall, -IVec3::X, false),
        PlacePrediction::Predicted(_)
    ));

    let torch = wall - IVec3::X;
    assert_eq!(
        Block::from_id(game.game.replica.chunk_block(torch.x, torch.y, torch.z)),
        Block::Torch
    );
    assert_eq!(
        game.game.replica.torch_placement(torch),
        crate::torch::TorchPlacement::West,
        "predicted place must record the wall mount for the same-frame mesh"
    );
}

#[test]
fn optimistic_stair_place_records_orientation_immediately() {
    let mut game = game();
    install_empty_chunk(&mut game);
    game.game.replica.insert_chunk_for_test(
        crate::chunk::ChunkPos::new(0, 0),
        crate::chunk::Chunk::new(0, 0),
    );
    let floor = IVec3::new(8, 63, 8);
    assert!(game
        .game
        .replica
        .set_block_world(floor.x, floor.y, floor.z, Block::Stone));
    game.game.player.pos = Vec3::new(100.0, 64.0, 100.0);
    let mut inv = crate::inventory::Inventory::new();
    inv.add(crate::item::ItemStack::new(
        crate::item::ItemType::OakStairs,
        1,
    ));
    game.server.sessions[0].player.inventory = inv;
    game.sync_self_view_for_test();

    // The absent-state fallback the mesher would read pre-fix; make the
    // player's facing produce something else, so the assert can tell a
    // recorded orientation from the fallback.
    let default_state = game
        .game
        .replica
        .section_at_world_for_test(floor.x, floor.y, floor.z)
        .expect("floor section")
        .stair_state(0, 0, 0);
    let expected_state = |g: &crate::game::Game| {
        crate::block_state::StairState::new(
            crate::server::placement::facing_from_forward(g.player.forward()),
            crate::block_state::StairHalf::Bottom,
        )
    };
    if expected_state(&game.game) == default_state {
        game.game.player.yaw += std::f32::consts::PI;
    }
    let expected = expected_state(&game.game);
    assert_ne!(expected, default_state, "fixture: non-default orientation");

    assert!(matches!(
        game.game.predict_place_at_for_test(floor, IVec3::Y, false),
        PlacePrediction::Predicted(_)
    ));

    let cell = floor + IVec3::Y;
    assert_eq!(
        Block::from_id(game.game.replica.chunk_block(cell.x, cell.y, cell.z)),
        Block::OakStairs
    );
    assert_eq!(
        game.game
            .replica
            .section_at_world_for_test(cell.x, cell.y, cell.z)
            .expect("stair section")
            .stair_state(8, 0, 8),
        expected,
        "predicted place must record the stair orientation for the same-frame mesh"
    );
}

#[test]
fn optimistic_chest_place_records_front_facing_immediately() {
    let mut game = game();
    install_empty_chunk(&mut game);
    game.game.replica.insert_chunk_for_test(
        crate::chunk::ChunkPos::new(0, 0),
        crate::chunk::Chunk::new(0, 0),
    );
    let floor = IVec3::new(8, 63, 8);
    assert!(game
        .game
        .replica
        .set_block_world(floor.x, floor.y, floor.z, Block::Stone));
    game.game.player.pos = Vec3::new(100.0, 64.0, 100.0);
    let mut inv = crate::inventory::Inventory::new();
    inv.add(crate::item::ItemStack::new(crate::item::ItemType::Chest, 1));
    game.server.sessions[0].player.inventory = inv;
    game.sync_self_view_for_test();

    let default_facing = game
        .game
        .replica
        .section_at_world_for_test(floor.x, floor.y, floor.z)
        .expect("floor section")
        .entity_facing(0, 0, 0);
    let facing_of =
        |g: &crate::game::Game| crate::server::placement::facing_from_forward(g.player.forward());
    if facing_of(&game.game) == default_facing {
        game.game.player.yaw += std::f32::consts::PI;
    }
    let expected = facing_of(&game.game);
    assert_ne!(expected, default_facing, "fixture: non-default facing");

    assert!(matches!(
        game.game.predict_place_at_for_test(floor, IVec3::Y, false),
        PlacePrediction::Predicted(_)
    ));

    let cell = floor + IVec3::Y;
    assert_eq!(
        game.game
            .replica
            .section_at_world_for_test(cell.x, cell.y, cell.z)
            .expect("chest section")
            .entity_facing(8, 0, 8),
        expected,
        "predicted place must record the front facing (chest render + furnace mesh)"
    );
}

#[test]
fn slab_stack_click_is_not_predicted() {
    let mut game = game();
    install_empty_chunk(&mut game);
    game.game.replica.insert_chunk_for_test(
        crate::chunk::ChunkPos::new(0, 0),
        crate::chunk::Chunk::new(0, 0),
    );
    game.game.player.pos = Vec3::new(100.0, 64.0, 100.0);
    // A bottom slab in the cell: clicking its top face stacks INTO that cell
    // server-side, off the ghost convention (`target + normal`), so the
    // request denies by design — the client must not ghost a slab above.
    let cell = IVec3::new(8, 64, 8);
    let facing = crate::server::placement::facing_from_forward(game.game.player.forward());
    let slot = crate::slab::slot_for_rotation(Default::default(), IVec3::Y, facing);
    assert!(game
        .game
        .replica
        .place_slab_layer(cell, Block::OakSlab, slot));
    let mut inv = crate::inventory::Inventory::new();
    inv.add(crate::item::ItemStack::new(
        crate::item::ItemType::OakSlab,
        1,
    ));
    game.server.sessions[0].player.inventory = inv;
    game.sync_self_view_for_test();

    assert!(
        matches!(
            game.game.predict_place_at_for_test(cell, IVec3::Y, false),
            PlacePrediction::Plausible
        ),
        "a stack click must classify Plausible: jab, no ghost (the server places in the CLICKED cell)"
    );
    let above = cell + IVec3::Y;
    assert_eq!(
        game.game.replica.chunk_block(above.x, above.y, above.z),
        Block::Air.id(),
        "no ghost slab in the cell above"
    );
}

#[test]
fn optimistic_break_clears_replica_and_queues_world_event() {
    let mut game = game();
    install_empty_chunk(&mut game);
    game.game.replica.insert_chunk_for_test(
        crate::chunk::ChunkPos::new(0, 0),
        crate::chunk::Chunk::new(0, 0),
    );
    let pos = IVec3::new(8, 64, 8);
    assert!(game
        .game
        .replica
        .set_block_world(pos.x, pos.y, pos.z, Block::Poppy));

    game.game.predict_break_at_for_test(pos, Block::Poppy);

    assert_eq!(
        Block::from_id(game.game.replica.chunk_block(pos.x, pos.y, pos.z)),
        Block::Air,
        "instant break must clear the replica immediately"
    );
    assert!(
        game.game.pending_events.world.iter().any(
            |e| matches!(e, crate::game::tick::WorldEvent::BlockBroken { pos: p, block, .. }
                if *p == pos && *block == Block::Poppy)
        ),
        "local BlockBroken must queue for sound/burst this frame"
    );
    assert_eq!(game.game.local_broke_block, Some(Block::Poppy));
}

#[test]
fn denied_place_restores_cell_and_inventory_silently() {
    let mut game = game();
    let pos = IVec3::new(3, 64, 3);
    game.game.replica.insert_chunk_for_test(
        crate::chunk::ChunkPos::new(0, 0),
        crate::chunk::Chunk::new(0, 0),
    );
    game.server.sessions[0].player.inventory = filled_inventory();
    game.sync_self_view_for_test();
    let before = game.self_view.inventory.clone();

    let id = game.game.prediction.begin(PredictionSnapshot::World {
        inventory: Some(before.clone()),
        cells: vec![(pos, Block::Air.0)],
    });
    assert!(game
        .game
        .replica
        .set_block_world(pos.x, pos.y, pos.z, Block::Dirt));
    game.self_view.inventory.decrement_selected();

    let update = TickUpdate {
        action_outcomes: vec![prediction::deny(id, ActionDenyReason::Denied)],
        ..Default::default()
    };
    game.game.apply_tick_update(Box::new(update));
    assert_eq!(
        Block::from_id(game.game.replica.chunk_block(pos.x, pos.y, pos.z)),
        Block::Air,
        "deny silently restores the cell"
    );
    assert_eq!(
        game.self_view.inventory.selected().map(|s| s.count),
        before.selected().map(|s| s.count),
        "deny restores the hotbar"
    );
    assert!(
        game.game.pending_events.world.is_empty(),
        "rollback must not emit presentation events"
    );
}

#[test]
fn break_finished_deny_queues_corrective_cells() {
    let mut game = game();
    install_empty_chunk(&mut game);
    let pos = IVec3::new(2, 64, 2);
    assert!(game
        .server
        .world
        .set_block_world(pos.x, pos.y, pos.z, Block::Stone));
    game.server.sessions[0].player.pos = Vec3::new(2.5, 65.0, 4.5);
    game.server.sessions[0].claim_pos = game.server.sessions[0].player.pos;

    game.server.apply_message(
        0,
        ClientToServer::Action(PlayerAction::BreakFinished {
            request_id: 9,
            pos,
            tool_item_id: None,
            predicted: true,
        }),
    );
    game.server.tick_mining(0, &mut TickEvents::default());
    let cells = &game.server.sessions[0].pending_corrective_cells;
    assert!(
        cells.contains(&pos),
        "a denied break finish must queue the claimed cell for corrective sync"
    );
    assert!(game.server.sessions[0]
        .pending_action_outcomes
        .iter()
        .any(|o| o.id == 9 && !o.accepted));
}

#[test]
fn far_claim_does_not_grant_reach() {
    let mut game = game();
    install_empty_chunk(&mut game);
    let far = IVec3::new(14, 64, 14);
    assert!(game
        .server
        .world
        .set_block_world(far.x, far.y, far.z, Block::Stone));
    game.server.sessions[0].player.pos = Vec3::new(2.5, 65.0, 2.5);

    // A fabricated claim right next to the far block: outside the drift ring
    // of the server's own integration, so it must not become the reach eye.
    let mut u = player_update(&game, true);
    u.pos = Vec3::new(13.5, 65.0, 13.5);
    u.vel = Vec3::ZERO;
    u.target = Some(hit(far, IVec3::Y));
    game.server
        .apply_message(0, ClientToServer::PlayerUpdate(u));
    assert!(
        game.server.sessions[0].look.is_none(),
        "an implausible claim must not validate a far look target"
    );

    game.server.apply_message(
        0,
        ClientToServer::Action(PlayerAction::BreakFinished {
            request_id: 30,
            pos: far,
            tool_item_id: None,
            predicted: true,
        }),
    );
    game.server.tick_mining(0, &mut TickEvents::default());
    assert_eq!(
        Block::from_id(game.server.world.chunk_block(far.x, far.y, far.z)),
        Block::Stone,
        "remote reach must not break the block"
    );
    assert!(
        game.server.sessions[0]
            .pending_action_outcomes
            .iter()
            .any(|o| o.id == 30 && !o.accepted && o.reason == Some(ActionDenyReason::OutOfReach)),
        "the far finish denies OutOfReach"
    );
}

#[test]
fn horizontal_teleport_claim_is_rejected() {
    let mut game = game();
    install_empty_chunk(&mut game);
    game.server.sessions[0].player.pos = Vec3::new(8.5, 70.0, 8.5);
    let start = game.server.sessions[0].player.pos;

    // A sideways hop far beyond any legitimate horizontal speed, under an
    // innocent velocity — the old isotropic (terminal-speed) ring accepted
    // this; the per-axis ring must not.
    let mut u = player_update(&game, true);
    u.pos = start + Vec3::new(3.5, 0.0, 0.0);
    u.vel = Vec3::new(5.0, 0.0, 0.0);
    game.server
        .apply_message(0, ClientToServer::PlayerUpdate(u));
    game.server.tick_movement(0);
    let after = game.server.sessions[0].player.pos;
    assert!(
        (after - start).length() < 2.0,
        "a horizontal jump beyond the sprint envelope must not be adopted (after={after:?})"
    );
}

#[test]
fn fake_on_ground_claims_do_not_evade_fall_damage() {
    let mut game = game();
    install_empty_chunk(&mut game);
    assert!(game.server.world.set_block_world(8, 64, 8, Block::Stone));
    game.server.sessions[0].player.pos = Vec3::new(8.5, 80.0, 8.5);
    game.server.sessions[0].claim_pos = game.server.sessions[0].player.pos;
    game.server.sessions[0].fall.reset(80.0);

    // Descend claiming on_ground every tick — the mid-air flag is fabricated
    // (no support under the feet), so the peak must survive to the landing.
    for y in [80.0, 76.0, 72.0, 68.0, 65.0] {
        let mut u = player_update(&game, true);
        u.pos = Vec3::new(8.5, y, 8.5);
        u.vel = Vec3::new(0.0, -20.0, 0.0);
        u.on_ground = true;
        game.server
            .apply_message(0, ClientToServer::PlayerUpdate(u));
        game.server.tick_movement(0);
    }
    assert!(
        game.server.sessions[0].pending_fall >= 14.0,
        "faked grounded claims must not reset the fall (measured {})",
        game.server.sessions[0].pending_fall
    );
}

#[test]
fn sprint_descent_down_steps_is_not_one_tall_fall() {
    use crate::block_state::{StairHalf, StairState};
    let mut game = game();
    install_empty_chunk(&mut game);
    // A staircase of real stair blocks descending +x (low half downhill), onto
    // a floor at y = 59 — half-block steps every half block, like any player
    // staircase.
    for i in 0..12 {
        assert!(game.server.world.place_stair(
            IVec3::new(2 + i, 70 - i, 8),
            Block::OakStairs,
            StairState::new(crate::facing::Facing::East, StairHalf::Bottom),
        ));
    }
    for x in 14..16 {
        assert!(game.server.world.set_block_world(x, 58, 8, Block::Stone));
    }

    let start = Vec3::new(2.3, 71.0, 8.5);
    game.server.sessions[0].player.pos = start;
    game.server.sessions[0].claim_pos = start;
    game.server.sessions[0].fall.reset(start.y);

    // The client's own 60 fps physics sprints down the staircase. Each step
    // contact lasts only a frame or two, so the once-per-tick report can
    // legitimately be an airborne mid-hop transform for the entire descent —
    // model that worst-case (but honest) send phase by reporting the window's
    // freshest airborne frame. The whole staircase must still never measure
    // as one tall fall: the server's own integration touched every step.
    let mut client = crate::player::Player::new(start);
    let input = crate::player::Input {
        wishdir: Vec3::new(1.0, 0.0, 0.0),
        jump: false,
        sprint: true,
        sneak: false,
    };
    for _ in 0..400 {
        let mut report = None;
        for _ in 0..3 {
            client.update(1.0 / 60.0, &game.server.world, input);
            if !client.on_ground || report.is_none() {
                report = Some((client.pos, client.vel, client.on_ground));
            }
        }
        let (pos, vel, on_ground) = report.unwrap();
        let mut u = player_update(&game, true);
        u.pos = pos;
        u.vel = vel;
        u.on_ground = on_ground;
        u.wishdir = input.wishdir;
        u.sprint = true;
        game.server
            .apply_message(0, ClientToServer::PlayerUpdate(u));
        game.server.tick_movement(0);
        if client.on_ground && client.pos.x > 14.2 {
            break;
        }
    }
    assert!(
        client.on_ground && client.pos.x > 14.2,
        "the client sim must finish the descent (ended at {:?})",
        client.pos
    );
    // Stand on the floor for a few ticks so the server observes the final
    // grounded transform (the landing that would convert a mis-measured
    // descent into damage).
    for _ in 0..3 {
        let mut u = player_update(&game, true);
        u.pos = client.pos;
        u.vel = client.vel;
        u.on_ground = true;
        game.server
            .apply_message(0, ClientToServer::PlayerUpdate(u));
        game.server.tick_movement(0);
    }

    let measured = game.server.sessions[0].pending_fall;
    assert_eq!(
        crate::server::health::fall_damage_health(measured),
        0,
        "sprinting down a staircase must not deal fall damage (measured a {measured}-block fall)"
    );
}

#[test]
fn unpredicted_break_finish_keeps_the_initiators_break_event() {
    let mut game = game();
    install_empty_chunk(&mut game);
    let pos = IVec3::new(8, 64, 8);
    assert!(game
        .server
        .world
        .set_block_world(pos.x, pos.y, pos.z, Block::Stone));
    game.server.sessions[0].player.pos = Vec3::new(8.5, 65.0, 10.5);

    let mut u = player_update(&game, true);
    u.break_held = true;
    u.target = Some(hit(pos, IVec3::new(0, 0, 1)));
    game.server
        .apply_message(0, ClientToServer::PlayerUpdate(u));
    let expected_ticks = (crate::mining::break_time(Block::Stone, None) / TICK_DT).round() as usize;
    for _ in 0..expected_ticks - 2 {
        game.server.tick_mining(0, &mut TickEvents::default());
    }
    // A TRACK-ONLY finish (frozen ledger / replica disagreement): the client
    // never presented, so the accept must not strip its BlockBroken.
    game.server.apply_message(
        0,
        ClientToServer::Action(PlayerAction::BreakFinished {
            request_id: 31,
            pos,
            tool_item_id: None,
            predicted: false,
        }),
    );
    game.server.tick_mining(0, &mut TickEvents::default());
    assert!(
        game.server.sessions[0]
            .pending_action_outcomes
            .iter()
            .any(|o| o.id == 31 && o.accepted),
        "the finish itself still accepts"
    );
    assert!(
        !game.server.sessions[0].presented_breaks.contains(&pos),
        "an unpresented break must not be stripped from the initiator's events"
    );
}

#[test]
fn multi_deny_rollback_is_emission_order_independent() {
    let mut game = game();
    install_empty_chunk(&mut game);
    game.server.sessions[0].player.inventory = filled_inventory();
    game.sync_self_view_for_test();
    let before = game.self_view.inventory.clone();

    game.game.drop_selected_item(false); // id 0
    game.game.drop_selected_item(false); // id 1

    // The server may emit an immediate deny for the NEWER id before a
    // tick-time deny for the older one — the restore must still end on the
    // oldest snapshot.
    let update = TickUpdate {
        action_outcomes: vec![
            prediction::deny(1, ActionDenyReason::Denied),
            prediction::deny(0, ActionDenyReason::Denied),
        ],
        ..Default::default()
    };
    game.game.apply_tick_update(Box::new(update));
    assert_eq!(
        game.self_view
            .inventory
            .slot(game.self_view.inventory.active_slot() as usize),
        before.slot(before.active_slot() as usize),
        "rollback must be allocation-ordered, not emission-ordered"
    );
}
