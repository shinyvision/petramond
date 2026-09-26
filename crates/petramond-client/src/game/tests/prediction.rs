//! Optimistic client prediction: ledger rollback, menu and placement
//! prediction, mining deny. Movement claims live in `movement_claims.rs`.

use super::common::*;
use crate::game::prediction::PredictionSnapshot;
use crate::game::tick::{GameInput, PlacePrediction};
use petramond::events::tick::{TickEvents, TICK_DT};
use petramond::net::protocol::{
    ActionDenyReason, ClientToServer, MenuSlotWire, PlayerAction, TickUpdate,
};
use petramond_math::math::IVec3;
use petramond_math::world_pos::WorldPos;
use petramond_world::block::Block;
use petramond_world::gui_state::PointerButton;
use petramond_world::gui_state::{GuiKind, MenuSlot};

#[test]
fn menu_click_deny_restores_inventory_snapshot() {
    let mut game = game_on_empty_chunk();
    game.server_player_mut().inventory = filled_inventory();
    game.sync_self_view_for_test();

    let before_cursor = game.replica.self_view.inventory.cursor().copied();
    game.menu_click(MenuSlot::Inventory(0), PointerButton::Primary, false, false);
    assert!(
        game.replica.self_view.inventory.cursor().is_some(),
        "optimistic pick onto cursor"
    );

    let id = 0;
    let rollbacks =
        game.prediction
            .reconcile(&[petramond::net::protocol::ActionOutcome::deny(
                id,
                ActionDenyReason::Denied,
            )]);
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
        .replica.self_view
        .inventory
        .add(petramond_world::item::ItemStack::new(
            petramond_world::item::ItemType::Grass,
            10,
        ));
    game.game.replica.self_view.inventory.click_slot(0);
    game.game.replica.menu_view.container = Some(petramond_world::gui_state::ContainerView {
        slots: vec![None; petramond::world::chest::CHEST_SLOTS],
    });
    game.game.replica.menu_view.container_kind =
        petramond_world::gui_state::resolve_kind("petramond:chest");

    game.game.menu_drag(
        GuiKind::Chest,
        vec![MenuSlot::Inventory(9), MenuSlot::Container(0)],
        PointerButton::Primary,
    );
    assert!(game.game.replica.self_view.inventory.cursor().is_none());
    assert_eq!(
        game.game
            .replica.self_view
            .inventory
            .slot(9)
            .map(|stack| stack.count),
        Some(5)
    );
    assert_eq!(
        game.game.replica.menu_view.container.as_ref().unwrap().slots[0].map(|stack| stack.count),
        Some(5)
    );

    game.game.apply_tick_update(Box::new(TickUpdate {
        action_outcomes: vec![petramond::net::protocol::ActionOutcome::deny(
            0,
            ActionDenyReason::Denied,
        )],
        ..Default::default()
    }));
    assert_eq!(
        game.game
            .replica.self_view
            .inventory
            .cursor()
            .map(|stack| stack.count),
        Some(10)
    );
    assert!(game.game.replica.self_view.inventory.slot(9).is_none());
    assert!(game.game.replica.menu_view.container.as_ref().unwrap().slots[0].is_none());
}

#[test]
fn accepted_menu_drag_prediction_reconciles_without_double_applying() {
    let mut game = game_on_empty_chunk();
    let pos = IVec3::new(3, 64, 3);
    game.server_world_mut().set_block_world(3, 64, 3, Block::Chest);
    game.server_world_mut()
        .insert_chest(pos, petramond_world::block_model::DEFAULT_MODEL_FACING);
    game.server_player_mut()
        .inventory
        .add(petramond_world::item::ItemStack::new(
            petramond_world::item::ItemType::Grass,
            10,
        ));
    game.server_player_mut().inventory.click_slot(0);
    let mut ev = TickEvents::default();
    game.sim_mut().open_chest_screen_for(0, pos, &mut ev);
    game.sync_self_view_for_test();
    game.sync_menu_view_for_test();

    game.game.menu_drag(
        GuiKind::Chest,
        vec![MenuSlot::Inventory(9), MenuSlot::Container(0)],
        PointerButton::Primary,
    );
    assert_eq!(game.game.prediction.pending_len(), 1);
    assert!(game.game.replica.self_view.inventory.cursor().is_none());
    assert_eq!(
        game.game
            .replica.self_view
            .inventory
            .slot(9)
            .map(|stack| stack.count),
        Some(5)
    );
    assert_eq!(
        game.game
            .replica.menu_view
            .container
            .as_ref()
            .and_then(|chest| chest.slots[0])
            .map(|stack| stack.count),
        Some(5)
    );

    game.tick(TICK_DT, &GameInput::default());

    assert_eq!(game.game.prediction.pending_len(), 0);
    assert!(game.game.replica.self_view.inventory.cursor().is_none());
    assert_eq!(
        game.game
            .replica.self_view
            .inventory
            .slot(9)
            .map(|stack| stack.count),
        Some(5)
    );
    assert_eq!(
        game.game
            .replica.menu_view
            .container
            .as_ref()
            .and_then(|chest| chest.slots[0])
            .map(|stack| stack.count),
        Some(5)
    );
}

/// A drag leg over a FILTERED slot must be predicted exactly as the server
/// applies it.
///
/// The two sides read their specs from different places (the client from the
/// document, the server from its own list), so "does this slot admit what I am
/// holding" is asked twice and can drift — and a drag splits the held stack by
/// the NUMBER of admitting destinations, so a slot only one side counts moves
/// the amount landing in every OTHER slot too. The divergence is never
/// confined to the leg that snaps back.
#[test]
fn a_drag_leg_over_a_filtered_slot_predicts_what_the_server_applies() {
    let mut game = game_on_empty_chunk();
    let pos = IVec3::new(3, 64, 3);
    game.server_world_mut().set_block_world(3, 64, 3, Block::Furnace);
    game.server_world_mut()
        .insert_furnace(pos, petramond_world::block_model::DEFAULT_MODEL_FACING);
    // Grass is neither fuel nor smeltable, so the furnace's fuel slot refuses
    // it — the one slot of the gesture that is not a destination.
    game.server_player_mut()
        .inventory
        .add(petramond_world::item::ItemStack::new(
            petramond_world::item::ItemType::Grass,
            10,
        ));
    game.server_player_mut().inventory.click_slot(0);
    game.sim_mut().open_furnace_screen_for(0, pos);
    game.sync_self_view_for_test();
    game.sync_menu_view_for_test();

    game.game.menu_drag(
        GuiKind::Furnace,
        vec![
            MenuSlot::Inventory(9),
            MenuSlot::Container(petramond_world::furnace::SLOT_FUEL),
        ],
        PointerButton::Primary,
    );
    let predicted_inv = game.game.replica.self_view.inventory.slot(9).map(|s| s.count);
    assert_eq!(
        predicted_inv,
        Some(10),
        "the refused leg is not a destination, so the whole stack goes to the other one"
    );

    game.tick(TICK_DT, &GameInput::default());
    assert_eq!(
        game.server_player()
            .inventory
            .slot(9)
            .map(|s| s.count),
        predicted_inv,
        "the server split the same way the client predicted"
    );
    assert!(
        game.server_world()
            .container_at(pos)
            .is_some_and(|c| c.slots[petramond_world::furnace::SLOT_FUEL].is_none()),
        "and nothing reached the filtered slot"
    );
}

/// The one-by-one container fill: a plain click on an open chest's slot is a
/// P1 prediction — the mirror slot and cursor move at click time, and the
/// outcome batch reconciles to the same state, never back through the
/// pre-click view (the counter up-down-up flicker, 2026-07-21).
#[test]
fn predicted_chest_slot_click_applies_immediately_and_survives_reconcile() {
    let mut game = game_on_empty_chunk();
    let pos = IVec3::new(3, 64, 3);
    game.server_world_mut().set_block_world(3, 64, 3, Block::Chest);
    game.server_world_mut()
        .insert_chest(pos, petramond_world::block_model::DEFAULT_MODEL_FACING);
    game.server_player_mut()
        .inventory
        .add(petramond_world::item::ItemStack::new(
            petramond_world::item::ItemType::Grass,
            10,
        ));
    game.server_player_mut().inventory.click_slot(0);
    let mut ev = TickEvents::default();
    game.sim_mut().open_chest_screen_for(0, pos, &mut ev);
    game.sync_self_view_for_test();
    game.sync_menu_view_for_test();

    game.menu_click(
        MenuSlot::Container(0),
        PointerButton::Secondary,
        false,
        false,
    );
    assert_eq!(
        game.game.replica.menu_view.container.as_ref().unwrap().slots[0].map(|stack| stack.count),
        Some(1),
        "the mirror slot fills at click time"
    );
    assert_eq!(
        game.game
            .replica.self_view
            .inventory
            .cursor()
            .map(|stack| stack.count),
        Some(9),
        "the cursor pays at click time"
    );

    game.tick(TICK_DT, &GameInput::default());

    assert_eq!(game.game.prediction.pending_len(), 0);
    assert_eq!(
        game.game.replica.menu_view.container.as_ref().unwrap().slots[0].map(|stack| stack.count),
        Some(1),
        "the authoritative pair confirms the prediction in place"
    );
    assert_eq!(
        game.game
            .replica.self_view
            .inventory
            .cursor()
            .map(|stack| stack.count),
        Some(9)
    );
}

/// Pipelined one-by-one clicks: a batch answering only the FIRST click
/// carries inventory/menu truth that predates the still-pending second — it
/// must not stomp the newer prediction (the rapid-click regress). The second
/// click's own forced outcome batch installs the final truth.
#[test]
fn a_stale_authoritative_pair_does_not_stomp_a_newer_pending_click() {
    use petramond::net::protocol::{ItemSlotWire, MenuSyncMsg, MenuTargetWire, SelfState};

    let grass = petramond_world::item::ItemType::Grass;
    let pos = IVec3::new(3, 64, 3);
    let mut game = game();
    game.game
        .replica.self_view
        .inventory
        .add(petramond_world::item::ItemStack::new(grass, 10));
    game.game.replica.self_view.inventory.click_slot(0);
    game.game.replica.menu_view.container = Some(petramond_world::gui_state::ContainerView {
        slots: vec![None; petramond::world::chest::CHEST_SLOTS],
    });
    game.game.replica.menu_view.container_kind =
        petramond_world::gui_state::resolve_kind("petramond:chest");

    game.game.menu_click(
        MenuSlot::Container(0),
        PointerButton::Secondary,
        false,
        false,
    );
    game.game.menu_click(
        MenuSlot::Container(0),
        PointerButton::Secondary,
        false,
        false,
    );
    let chest_count = |game: &TestGame| {
        game.game.replica.menu_view.container.as_ref().unwrap().slots[0].map(|stack| stack.count)
    };
    let cursor_count = |game: &TestGame| {
        game.game
            .replica.self_view
            .inventory
            .cursor()
            .map(|stack| stack.count)
    };
    assert_eq!(chest_count(&game), Some(2));
    assert_eq!(cursor_count(&game), Some(8));

    let self_state = |cursor: u8, revision: u64| {
        let mut slots: Vec<Option<ItemSlotWire>> = vec![None; 36];
        slots.push(Some(ItemSlotWire {
            item_id: grass.0,
            count: cursor,
            data: None,
        }));
        SelfState {
            conditions: Vec::new(),
            health: 20,
            mode: 0,
            effects: Vec::new(),
            inventory_revision: revision,
            inventory: Some(slots),
            eating: None,
            eating_off_hand: false,
            move_scale: 1.0,
            denied_actions: Default::default(),
            held_pose_main: None,
            held_pose_off: None,
            held_display: [None; 2],
            bone_poses: Vec::new(),
            animator: Default::default(),
            sleeping: None,
            sleep_bed: None,
            transform: None,
        }
    };
    let chest_sync = |count: u8| {
        let mut slots: Vec<Option<ItemSlotWire>> = vec![None; petramond::world::chest::CHEST_SLOTS];
        slots[0] = Some(ItemSlotWire {
            item_id: grass.0,
            count,
            data: None,
        });
        MenuSyncMsg {
            target: MenuTargetWire::Container {
                kind_key: "petramond:chest".to_string(),
                anchor: Some(pos.into()),
                slots: Some(slots),
                gui_state: None,
            },
        }
    };

    // The first click's batch: truth as of click #1 only.
    game.game.apply_tick_update(Box::new(TickUpdate {
        action_outcomes: vec![petramond::net::protocol::ActionOutcome::accept(0)],
        self_state: Some(self_state(9, 1)),
        menu_sync: Some(chest_sync(1)),
        ..Default::default()
    }));
    assert_eq!(
        chest_count(&game),
        Some(2),
        "the still-pending second click's prediction stays visible"
    );
    assert_eq!(cursor_count(&game), Some(8));

    // The second click's own batch: final truth, pending queue drains.
    game.game.apply_tick_update(Box::new(TickUpdate {
        action_outcomes: vec![petramond::net::protocol::ActionOutcome::accept(1)],
        self_state: Some(self_state(8, 2)),
        menu_sync: Some(chest_sync(2)),
        ..Default::default()
    }));
    assert_eq!(game.game.prediction.pending_len(), 0);
    assert_eq!(chest_count(&game), Some(2));
    assert_eq!(cursor_count(&game), Some(8));
}

#[test]
fn break_finished_without_observed_mining_is_denied() {
    let mut game = game_on_empty_chunk();
    let pos = IVec3::new(2, 64, 2);
    assert!(game
        .server_world_mut()
        .set_block_world(pos.x, pos.y, pos.z, Block::Stone));

    game.server_player_mut().pos = WorldPos::new(2.5, 65.0, 4.5);
    game.session_mut().claim_pos = game.server_player().pos;

    // Never started mining: the finish is TooFast-deferred, then abandoned
    // in the same tick (no active target) — deny + corrective, no clear.
    game.send_to_server(ClientToServer::Action(PlayerAction::BreakFinished {
            request_id: 7,
            pos,
            tool_item_id: None,
            predicted: true,
        }),
    );
    let mut ev = TickEvents::default();
    game.sim_mut().tick_mining(0, &mut ev);
    assert_eq!(
        Block::from_id(game.server_world().chunk_block(pos.x, pos.y, pos.z)),
        Block::Stone,
        "too-fast break must not clear the cell"
    );
    let outcomes = &game.session().pending_action_outcomes;
    assert_eq!(outcomes.len(), 1);
    assert!(!outcomes[0].accepted);
    assert_eq!(outcomes[0].reason, Some(ActionDenyReason::TooFast));
    assert!(
        game.session()
            .pending_corrective_cells
            .contains(&pos),
        "abandoned TooFast must correct the optimistic clear"
    );
}

#[test]
fn two_instabreak_finishes_in_one_tick_window_both_accept() {
    let mut game = game_on_empty_chunk();
    let a = IVec3::new(2, 64, 2);
    let b = IVec3::new(3, 64, 2);
    assert!(game
        .server_world_mut()
        .set_block_world(a.x, a.y, a.z, Block::Poppy));
    assert!(game
        .server_world_mut()
        .set_block_world(b.x, b.y, b.z, Block::Poppy));
    game.server_player_mut().pos = WorldPos::new(2.5, 65.0, 4.5);
    game.session_mut().claim_pos = game.server_player().pos;

    // Two instabreak blocks broken back-to-back land in the same tick window.
    game.send_to_server(ClientToServer::Action(PlayerAction::BreakFinished {
            request_id: 1,
            pos: a,
            tool_item_id: None,
            predicted: true,
        }),
    );
    game.send_to_server(ClientToServer::Action(PlayerAction::BreakFinished {
            request_id: 2,
            pos: b,
            tool_item_id: None,
            predicted: true,
        }),
    );
    game.sim_mut().tick_mining(0, &mut TickEvents::default());

    assert_eq!(
        Block::from_id(game.server_world().chunk_block(a.x, a.y, a.z)),
        Block::Air,
        "the first instabreak must clear its cell"
    );
    assert_eq!(
        Block::from_id(game.server_world().chunk_block(b.x, b.y, b.z)),
        Block::Air,
        "the second instabreak must clear its cell"
    );
    let outcomes = &game.session().pending_action_outcomes;
    assert!(
        outcomes.iter().any(|o| o.id == 1 && o.accepted),
        "the first finish must accept, got {outcomes:?}"
    );
    assert!(
        outcomes.iter().any(|o| o.id == 2 && o.accepted),
        "the second finish must accept, got {outcomes:?}"
    );
    assert!(
        game.session().pending_corrective_cells.is_empty(),
        "no corrective cells for two legitimate instabreaks"
    );
}

#[test]
fn lagged_break_finished_after_hold_path_accepts_without_restore() {
    let mut game = game_on_empty_chunk();
    let pos = IVec3::new(8, 64, 8);
    assert!(game
        .server_world_mut()
        .set_block_world(pos.x, pos.y, pos.z, Block::Stone));
    game.server_player_mut().pos = WorldPos::new(8.5, 65.0, 10.5);
    game.session_mut().claim_pos = game.server_player().pos;

    let mut u = player_update(&game, true);
    u.break_held = true;
    u.target = Some(hit(pos, IVec3::new(0, 0, 1)));
    game.send_to_server(ClientToServer::PlayerUpdate(u));

    // Hold-path clears the cell BEFORE BreakFinished arrives (slow uplink).
    let expected_ticks =
        (petramond_world::mining::break_time(Block::Stone, None) / TICK_DT).round() as usize;
    for _ in 0..expected_ticks + 2 {
        game.sim_mut().tick_mining(0, &mut TickEvents::default());
        if Block::from_id(game.server_world().chunk_block(pos.x, pos.y, pos.z)) == Block::Air {
            break;
        }
    }
    assert_eq!(
        Block::from_id(game.server_world().chunk_block(pos.x, pos.y, pos.z)),
        Block::Air
    );
    assert!(
        game.session().pending_break_ack.contains_key(&pos),
        "hold-path owes a BreakFinished accept"
    );

    game.send_to_server(ClientToServer::Action(PlayerAction::BreakFinished {
            request_id: 22,
            pos,
            tool_item_id: None,
            predicted: true,
        }),
    );
    game.sim_mut().tick_mining(0, &mut TickEvents::default());
    assert_eq!(
        Block::from_id(game.server_world().chunk_block(pos.x, pos.y, pos.z)),
        Block::Air,
        "lagged finish must NOT restore the block"
    );
    assert!(
        game.session()
            .pending_action_outcomes
            .iter()
            .any(|o| o.id == 22 && o.accepted),
        "lagged finish after own hold-path must accept"
    );
    assert!(
        game.session().pending_corrective_cells.is_empty(),
        "accept must not ship corrective cells"
    );
}

#[test]
fn early_break_finished_defers_then_accepts_on_hold_path_without_restore() {
    let mut game = game_on_empty_chunk();
    let pos = IVec3::new(8, 64, 8);
    assert!(game
        .server_world_mut()
        .set_block_world(pos.x, pos.y, pos.z, Block::Stone));
    game.server_player_mut().pos = WorldPos::new(8.5, 65.0, 10.5);
    game.session_mut().claim_pos = game.server_player().pos;

    // Start the server's observed mining window.
    let mut u = player_update(&game, true);
    u.break_held = true;
    u.target = Some(hit(pos, IVec3::new(0, 0, 1)));
    game.send_to_server(ClientToServer::PlayerUpdate(u));

    // One tick of progress — far short of stone's break time.
    game.sim_mut().tick_mining(0, &mut TickEvents::default());
    game.send_to_server(ClientToServer::Action(PlayerAction::BreakFinished {
            request_id: 11,
            pos,
            tool_item_id: None,
            predicted: true,
        }),
    );
    game.sim_mut().tick_mining(0, &mut TickEvents::default());
    assert!(
        game.session().pending_action_outcomes.is_empty(),
        "TooFast while mining must defer, not deny (no restore)"
    );
    assert!(
        game.session().deferred_break_finished.is_some(),
        "the finish waits for the hold-path"
    );
    assert!(
        game.session().pending_corrective_cells.is_empty(),
        "deferred TooFast must not ship corrective cells"
    );
    assert_eq!(
        Block::from_id(game.server_world().chunk_block(pos.x, pos.y, pos.z)),
        Block::Stone,
        "server cell stays until the hold-path finishes"
    );

    // Hold until the server's timer breaks the block.
    let expected_ticks =
        (petramond_world::mining::break_time(Block::Stone, None) / TICK_DT).round() as usize;
    for _ in 0..expected_ticks + 2 {
        game.sim_mut().tick_mining(0, &mut TickEvents::default());
        if Block::from_id(game.server_world().chunk_block(pos.x, pos.y, pos.z)) == Block::Air {
            break;
        }
    }
    assert_eq!(
        Block::from_id(game.server_world().chunk_block(pos.x, pos.y, pos.z)),
        Block::Air,
        "hold-path clears the cell"
    );
    let outcomes = &game.session().pending_action_outcomes;
    assert!(
        outcomes.iter().any(|o| o.id == 11 && o.accepted),
        "deferred finish accepts when the hold-path breaks, got {outcomes:?}"
    );
    assert!(
        game.session().presented_breaks.contains(&pos),
        "a deferred PREDICTED finish strips the initiator's BlockBroken"
    );
}

#[test]
fn break_finished_after_the_observed_mining_window_is_accepted() {
    let mut game = game_on_empty_chunk();
    let pos = IVec3::new(8, 64, 8);
    assert!(game
        .server_world_mut()
        .set_block_world(pos.x, pos.y, pos.z, Block::Stone));
    game.server_player_mut().pos = WorldPos::new(8.5, 65.0, 10.5);

    // Latch a held break on the target: the server's own mining timer is the
    // observation the finish is validated against.
    let mut u = player_update(&game, true);
    u.break_held = true;
    u.target = Some(hit(pos, IVec3::new(0, 0, 1)));
    game.send_to_server(ClientToServer::PlayerUpdate(u));

    let expected_ticks =
        (petramond_world::mining::break_time(Block::Stone, None) / TICK_DT).round() as usize;
    // Hold just short of the server's own finish, then deliver the client's.
    for _ in 0..expected_ticks - 2 {
        game.sim_mut().tick_mining(0, &mut TickEvents::default());
    }
    game.send_to_server(ClientToServer::Action(PlayerAction::BreakFinished {
            request_id: 6,
            pos,
            tool_item_id: None,
            predicted: true,
        }),
    );
    game.sim_mut().tick_mining(0, &mut TickEvents::default());
    assert_eq!(
        Block::from_id(game.server_world().chunk_block(pos.x, pos.y, pos.z)),
        Block::Air,
        "an observed full mining window accepts the client's finish"
    );
    let outcomes = &game.session().pending_action_outcomes;
    assert!(outcomes.iter().any(|o| o.id == 6 && o.accepted));
}

#[test]
fn each_queued_drop_in_one_tick_window_gets_its_own_outcome() {
    let mut game = game_on_empty_chunk();
    game.server_player_mut().inventory = filled_inventory();
    game.send_to_server(ClientToServer::Action(PlayerAction::Drop {
            all: false,
            request_id: 11,
        }),
    );
    game.send_to_server(ClientToServer::Action(PlayerAction::Drop {
            all: false,
            request_id: 12,
        }),
    );
    // Nothing on the cursor: the throw cannot even queue, denied immediately.
    game.send_to_server(ClientToServer::Action(PlayerAction::ThrowCursor {
            amount: petramond::net::protocol::ThrowAmount::One,
            request_id: 13,
        }),
    );
    let mut ev = TickEvents::default();
    game.sim_mut().tick_drops(0, &mut ev);
    let outcomes = &game.session().pending_action_outcomes;
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
    let mut game = game_on_empty_chunk();
    game.server_player_mut().inventory = filled_inventory();
    game.sync_self_view_for_test();
    let before = game.replica.self_view.inventory.clone();

    // Two predicted drops back to back: the second snapshot already embeds
    // the first prediction's effect.
    game.game.drop_selected_item(false); // id 0
    game.game.drop_selected_item(false); // id 1

    // Both denied in one batch: the restore must end on the OLDEST snapshot.
    let update = TickUpdate {
        action_outcomes: vec![
            petramond::net::protocol::ActionOutcome::deny(0, ActionDenyReason::Denied),
            petramond::net::protocol::ActionOutcome::deny(1, ActionDenyReason::Denied),
        ],
        ..Default::default()
    };
    game.game.apply_tick_update(Box::new(update));
    assert_eq!(
        game.replica.self_view
            .inventory
            .slot(game.replica.self_view.inventory.active_slot() as usize),
        before.slot(before.active_slot() as usize),
        "both denied predictions must be rolled back, not just the newest"
    );
}

#[test]
fn denied_cell_rollback_yields_to_a_same_batch_authoritative_delta() {
    let mut game = game();
    // A loaded replica cell the ghost writes into.
    let pos = IVec3::new(3, 64, 3);
    game.game.replica.world.insert_chunk_for_test(
        petramond_world::chunk::ChunkPos::new(0, 0),
        petramond_world::chunk::Chunk::new(0, 0),
    );

    // Predict a ghost placement (World snapshot, prev = air).
    let id = game.game.prediction.begin(PredictionSnapshot::World {
        inventory: None,
        cells: vec![(pos, Block::Air.0)],
    });
    assert!(game
        .game
        .replica.world
        .set_block_world(pos.x, pos.y, pos.z, Block::Dirt));

    // Same batch: the deny AND an authoritative delta at the cell (another
    // player's block won it). The delta must survive the rollback.
    let update = TickUpdate {
        block_deltas: vec![petramond::net::protocol::BlockDelta {
            pos,
            block_id: Block::Stone.0,
            fluid: None,
            state: None,
            cell_kv: vec![],
        }],
        action_outcomes: vec![petramond::net::protocol::ActionOutcome::deny(
            id,
            ActionDenyReason::Denied,
        )],
        ..Default::default()
    };
    game.game.apply_tick_update(Box::new(update));
    assert_eq!(
        Block::from_id(game.game.replica.world.chunk_block(pos.x, pos.y, pos.z)),
        Block::Stone,
        "an authoritative same-batch delta wins over the deny rollback"
    );
}

#[test]
fn place_resolves_at_the_click_target_not_the_freshest_look() {
    let mut game = game_on_empty_chunk();
    let a = IVec3::new(8, 63, 8);
    let b = IVec3::new(11, 63, 11);
    assert!(game
        .server_world_mut()
        .set_block_world(a.x, a.y, a.z, Block::Stone));
    assert!(game
        .server_world_mut()
        .set_block_world(b.x, b.y, b.z, Block::Stone));
    game.server_player_mut().pos = WorldPos::new(9.5, 63.0, 9.5);
    game.server_player_mut().inventory = filled_inventory(); // dirt

    // Click aimed at A...
    let mut u = player_update(&game, true);
    u.target = Some(hit(a, IVec3::Y));
    game.send_to_server(ClientToServer::PlayerUpdate(u));
    game.send_to_server(ClientToServer::Action(PlayerAction::UseClick {
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
    game.send_to_server(ClientToServer::PlayerUpdate(u2));

    game.sim_mut().tick_place(0, &mut TickEvents::default());
    assert_eq!(
        Block::from_id(game.server_world().chunk_block(a.x, a.y + 1, a.z)),
        Block::Dirt,
        "the block lands where the CLICK aimed (the client's ghost)"
    );
    assert_eq!(
        Block::from_id(game.server_world().chunk_block(b.x, b.y + 1, b.z)),
        Block::Air,
        "the fresher look must not hijack the click"
    );
    let outcomes = &game.session().pending_action_outcomes;
    assert!(outcomes.iter().any(|o| o.id == 3 && o.accepted));
}

#[test]
fn no_op_use_click_queues_the_disputed_cells_for_corrective_sync() {
    let mut game = game_on_empty_chunk();
    let t = IVec3::new(8, 64, 8);
    assert!(game
        .server_world_mut()
        .set_block_world(t.x, t.y, t.z, Block::Stone));
    game.server_player_mut().pos = WorldPos::new(8.5, 65.5, 10.5);

    // Empty hand, non-interactable stone: the server consumes nothing — the
    // client may have clicked a cell that only exists in ITS replica, so the
    // authoritative state of the disputed cells ships back.
    let mut u = player_update(&game, true);
    u.target = Some(hit(t, IVec3::Y));
    game.send_to_server(ClientToServer::PlayerUpdate(u));
    game.send_to_server(ClientToServer::Action(PlayerAction::UseClick {
            mob: None,
            target: Some(hit(t, IVec3::Y)),
            request_id: None,
            predicted: false,
            jabbed: false,
        }),
    );
    game.sim_mut().tick_place(0, &mut TickEvents::default());
    let cells = &game.session().pending_corrective_cells;
    assert!(cells.contains(&t), "the clicked cell reconciles");
    assert!(
        cells.contains(&(t + IVec3::Y)),
        "the would-be place cell reconciles"
    );
}

#[test]
fn menu_click_ships_request_id_and_server_accepts() {
    let mut game = game();
    game.server_player_mut().inventory = filled_inventory();
    game.send_to_server(ClientToServer::MenuClick {
            slot: MenuSlotWire::from_menu_slot(&MenuSlot::Inventory(0)),
            button: 0,
            shift: false,
            gather: false,
            request_id: 42,
        },
    );
    game.sim_mut().tick_menu(0, &mut TickEvents::default());
    let outcomes = &game.session().pending_action_outcomes;
    assert_eq!(outcomes.len(), 1);
    assert!(outcomes[0].accepted);
    assert_eq!(outcomes[0].id, 42);
}

#[test]
fn optimistic_place_mutates_replica_hotbar_and_queues_world_event() {
    let mut game = game_on_empty_chunk();
    // Mirror the chunk onto the replica so the place ghost can write.
    game.game.replica.world.insert_chunk_for_test(
        petramond_world::chunk::ChunkPos::new(0, 0),
        petramond_world::chunk::Chunk::new(0, 0),
    );
    let floor = IVec3::new(8, 63, 8);
    assert!(game
        .game
        .replica.world
        .set_block_world(floor.x, floor.y, floor.z, Block::Stone));
    // Park the body clear of the place cell so placement_blocked_by_body
    // does not refuse the ghost.
    game.game.local.player.pos = WorldPos::new(100.0, 64.0, 100.0);
    game.server_player_mut().inventory = filled_inventory();
    game.sync_self_view_for_test();
    let before = game
        .replica.self_view
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
                .replica.world
                .chunk_block(place_pos.x, place_pos.y, place_pos.z)
        ),
        Block::Dirt,
        "replica cell must change immediately"
    );
    assert_eq!(
        game.replica.self_view
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
    assert_eq!(game.game.hand.placed(), Some(Block::Dirt));
}

/// An interactive block (chest, crafting table, furnace…) clicked without
/// sneaking is claimed by the server's BUILT-IN consumer before the place rung
/// ever runs — so the client's place prediction must cancel EVERY placement
/// arm, the mod custom-shape arm included (the chain-into-chest regression,
/// 2026-07-22: the custom plan dispatched before the built-in claim gate and
/// ghosted a block the server never places). Sneaking defers the built-in
/// claim, so the same click then ghosts normally.
#[test]
fn interactive_block_click_cancels_the_custom_shape_ghost_unless_sneaking() {
    use petramond_world::block::ShapeFamily;
    // Any mod-registered custom-shape block with a linked item (the
    // furniture chain, when the pack is installed). The engine ships no
    // custom rows, so without an installed pack there is nothing to pin.
    let Some(item) = petramond_world::item::ItemType::all()
        .iter()
        .copied()
        .find(|i| {
            i.as_block()
                .is_some_and(|b| !b.is_engine() && b.shape_family() == ShapeFamily::Custom)
        })
    else {
        return;
    };
    let mut game = game_on_empty_chunk();
    game.game.replica.world.insert_chunk_for_test(
        petramond_world::chunk::ChunkPos::new(0, 0),
        petramond_world::chunk::Chunk::new(0, 0),
    );
    let chest = IVec3::new(8, 64, 8);
    assert!(game
        .game
        .replica.world
        .set_block_world(chest.x, chest.y, chest.z, Block::Chest));
    // Park the body clear of the build cell so occupancy never refuses.
    game.game.local.player.pos = WorldPos::new(100.0, 64.0, 100.0);
    give(&mut game, item, 8);
    game.sync_self_view_for_test();
    // Scripted accepted plan on the build cell — the deterministic answer a
    // loaded client instance would compute (no wasm in this harness).
    let place_pos = chest + IVec3::Y;
    game.game.client_mods.scripted_shape_plan = Some(mod_api::ShapePlacementResult {
        accepted: true,
        anchor: place_pos.to_array(),
        cells: vec![],
        block: None,
    });

    // Non-sneak: the built-in chest claim wins — silent, and no ghost cell.
    assert!(matches!(
        game.game.predict_place_at_for_test(chest, IVec3::Y, false),
        PlacePrediction::No
    ));
    assert_eq!(
        game.game
            .replica.world
            .chunk_block(place_pos.x, place_pos.y, place_pos.z),
        Block::Air.0,
        "an interactive target must never ghost a mod block"
    );

    // Sneaking defers the built-in claim: the same click ghosts in full —
    // proof the custom arm itself still works (the gate is ordering, not a
    // blanket veto).
    assert!(matches!(
        game.game.predict_place_at_for_test(chest, IVec3::Y, true),
        PlacePrediction::Predicted(_)
    ));
    assert_ne!(
        game.game
            .replica.world
            .chunk_block(place_pos.x, place_pos.y, place_pos.z),
        Block::Air.0,
        "the sneak click ghosts the custom block"
    );
}

#[test]
fn optimistic_torch_place_records_wall_mount_immediately() {
    let mut game = game_on_empty_chunk();
    game.game.replica.world.insert_chunk_for_test(
        petramond_world::chunk::ChunkPos::new(0, 0),
        petramond_world::chunk::Chunk::new(0, 0),
    );
    let wall = IVec3::new(8, 64, 8);
    assert!(game
        .game
        .replica.world
        .set_block_world(wall.x, wall.y, wall.z, Block::Stone));
    game.game.local.player.pos = WorldPos::new(100.0, 64.0, 100.0);
    give(&mut game, petramond_world::item::ItemType::Torch, 1);
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
        Block::from_id(game.game.replica.world.chunk_block(torch.x, torch.y, torch.z)),
        Block::Torch
    );
    assert_eq!(
        game.game.replica.world.torch_placement(torch),
        petramond_world::torch::TorchPlacement::West,
        "predicted place must record the wall mount for the same-frame mesh"
    );
}

#[test]
fn optimistic_stair_place_records_orientation_immediately() {
    let mut game = game_on_empty_chunk();
    game.game.replica.world.insert_chunk_for_test(
        petramond_world::chunk::ChunkPos::new(0, 0),
        petramond_world::chunk::Chunk::new(0, 0),
    );
    let floor = IVec3::new(8, 63, 8);
    assert!(game
        .game
        .replica.world
        .set_block_world(floor.x, floor.y, floor.z, Block::Stone));
    game.game.local.player.pos = WorldPos::new(100.0, 64.0, 100.0);
    give(&mut game, petramond_world::item::ItemType::OakStairs, 1);
    game.sync_self_view_for_test();

    // The absent-state fallback the mesher would read pre-fix; make the
    // player's facing produce something else, so the assert can tell a
    // recorded orientation from the fallback.
    let default_state = game
        .game
        .replica.world
        .section_at_world_for_test(floor.x, floor.y, floor.z)
        .expect("floor section")
        .stair_state(0, 0, 0);
    let expected_state = |g: &crate::game::Game| {
        petramond_world::block_state::StairState::new(
            petramond::rules::placement::facing_from_forward(g.local.player.forward()),
            petramond_world::block_state::StairHalf::Bottom,
        )
    };
    if expected_state(&game.game) == default_state {
        game.game.local.player.yaw += std::f32::consts::PI;
    }
    let expected = expected_state(&game.game);
    assert_ne!(expected, default_state, "fixture: non-default orientation");

    assert!(matches!(
        game.game.predict_place_at_for_test(floor, IVec3::Y, false),
        PlacePrediction::Predicted(_)
    ));

    let cell = floor + IVec3::Y;
    assert_eq!(
        Block::from_id(game.game.replica.world.chunk_block(cell.x, cell.y, cell.z)),
        Block::OakStairs
    );
    assert_eq!(
        game.game
            .replica.world
            .section_at_world_for_test(cell.x, cell.y, cell.z)
            .expect("stair section")
            .stair_state(8, 0, 8),
        expected,
        "predicted place must record the stair orientation for the same-frame mesh"
    );
}

#[test]
fn optimistic_chest_place_records_front_facing_immediately() {
    let mut game = game_on_empty_chunk();
    game.game.replica.world.insert_chunk_for_test(
        petramond_world::chunk::ChunkPos::new(0, 0),
        petramond_world::chunk::Chunk::new(0, 0),
    );
    let floor = IVec3::new(8, 63, 8);
    assert!(game
        .game
        .replica.world
        .set_block_world(floor.x, floor.y, floor.z, Block::Stone));
    game.game.local.player.pos = WorldPos::new(100.0, 64.0, 100.0);
    give(&mut game, petramond_world::item::ItemType::Chest, 1);
    game.sync_self_view_for_test();

    let default_facing = game
        .game
        .replica.world
        .section_at_world_for_test(floor.x, floor.y, floor.z)
        .expect("floor section")
        .entity_facing(0, 0, 0);
    let facing_of = |g: &crate::game::Game| {
        petramond::rules::placement::facing_from_forward(g.local.player.forward())
    };
    if facing_of(&game.game) == default_facing {
        game.game.local.player.yaw += std::f32::consts::PI;
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
            .replica.world
            .section_at_world_for_test(cell.x, cell.y, cell.z)
            .expect("chest section")
            .entity_facing(8, 0, 8),
        expected,
        "predicted place must record the front facing (chest render + furnace mesh)"
    );
}

#[test]
fn optimistic_ladder_place_commits_the_facing_row() {
    // Ladder facing is block IDENTITY (one row per facing): the ghost must
    // write the sibling row matching the clicked wall face — same-frame mesh,
    // panel collision, and climb probe all read that id — and must leave the
    // entity-facing map untouched (a ladder is not a block entity).
    let mut game = game_on_empty_chunk();
    game.game.replica.world.insert_chunk_for_test(
        petramond_world::chunk::ChunkPos::new(0, 0),
        petramond_world::chunk::Chunk::new(0, 0),
    );
    let wall = IVec3::new(8, 64, 8);
    assert!(game
        .game
        .replica.world
        .set_block_world(wall.x, wall.y, wall.z, Block::Stone));
    game.game.local.player.pos = WorldPos::new(100.0, 64.0, 100.0);
    give(&mut game, petramond_world::item::ItemType::Ladder, 1);
    game.sync_self_view_for_test();

    // Click the wall's +X face: the panel front points east, hanging on the
    // wall to its west.
    assert!(matches!(
        game.game.predict_place_at_for_test(wall, IVec3::X, false),
        PlacePrediction::Predicted(_)
    ));

    let cell = wall + IVec3::X;
    assert_eq!(
        game.game.replica.world.chunk_block(cell.x, cell.y, cell.z),
        Block::LadderEast.id(),
        "the ghost is the facing row, not the held base row"
    );
    assert!(
        game.game
            .replica.world
            .section_at_world_for_test(cell.x, cell.y, cell.z)
            .expect("ladder section")
            .cell_states()
            .is_empty(),
        "no per-cell state record — the ladder's facing is block identity"
    );
}

#[test]
fn slab_stack_click_is_not_predicted() {
    let mut game = game_on_empty_chunk();
    game.game.replica.world.insert_chunk_for_test(
        petramond_world::chunk::ChunkPos::new(0, 0),
        petramond_world::chunk::Chunk::new(0, 0),
    );
    game.game.local.player.pos = WorldPos::new(100.0, 64.0, 100.0);
    // A bottom slab in the cell: clicking its top face stacks INTO that cell
    // server-side, off the ghost convention (`target + normal`), so the
    // request denies by design — the client must not ghost a slab above.
    let cell = IVec3::new(8, 64, 8);
    let facing = petramond::rules::placement::facing_from_forward(game.game.local.player.forward());
    let slot = petramond_world::slab::slot_for_rotation(Default::default(), IVec3::Y, facing);
    assert!(game
        .game
        .replica.world
        .place_slab_layer(cell, Block::OakSlab, slot));
    give(&mut game, petramond_world::item::ItemType::OakSlab, 1);
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
        game.game.replica.world.chunk_block(above.x, above.y, above.z),
        Block::Air.id(),
        "no ghost slab in the cell above"
    );
}

#[test]
fn optimistic_break_clears_replica_and_queues_world_event() {
    let mut game = game_on_empty_chunk();
    game.game.replica.world.insert_chunk_for_test(
        petramond_world::chunk::ChunkPos::new(0, 0),
        petramond_world::chunk::Chunk::new(0, 0),
    );
    let pos = IVec3::new(8, 64, 8);
    assert!(game
        .game
        .replica.world
        .set_block_world(pos.x, pos.y, pos.z, Block::Poppy));

    game.game.predict_break_at_for_test(pos, Block::Poppy);

    assert_eq!(
        Block::from_id(game.game.replica.world.chunk_block(pos.x, pos.y, pos.z)),
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
    assert_eq!(game.game.hand.broke(), Some(Block::Poppy));
}

#[test]
fn denied_place_restores_cell_and_inventory_silently() {
    let mut game = game();
    let pos = IVec3::new(3, 64, 3);
    game.game.replica.world.insert_chunk_for_test(
        petramond_world::chunk::ChunkPos::new(0, 0),
        petramond_world::chunk::Chunk::new(0, 0),
    );
    game.server_player_mut().inventory = filled_inventory();
    game.sync_self_view_for_test();
    let before = game.replica.self_view.inventory.clone();

    let id = game.game.prediction.begin(PredictionSnapshot::World {
        inventory: Some(before.clone()),
        cells: vec![(pos, Block::Air.0)],
    });
    assert!(game
        .game
        .replica.world
        .set_block_world(pos.x, pos.y, pos.z, Block::Dirt));
    game.replica.self_view.inventory.decrement_selected();

    let update = TickUpdate {
        action_outcomes: vec![petramond::net::protocol::ActionOutcome::deny(
            id,
            ActionDenyReason::Denied,
        )],
        ..Default::default()
    };
    game.game.apply_tick_update(Box::new(update));
    assert_eq!(
        Block::from_id(game.game.replica.world.chunk_block(pos.x, pos.y, pos.z)),
        Block::Air,
        "deny silently restores the cell"
    );
    assert_eq!(
        game.replica.self_view.inventory.selected().map(|s| s.count),
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
    let mut game = game_on_empty_chunk();
    let pos = IVec3::new(2, 64, 2);
    assert!(game
        .server_world_mut()
        .set_block_world(pos.x, pos.y, pos.z, Block::Stone));
    game.server_player_mut().pos = WorldPos::new(2.5, 65.0, 4.5);
    game.session_mut().claim_pos = game.server_player().pos;

    game.send_to_server(ClientToServer::Action(PlayerAction::BreakFinished {
            request_id: 9,
            pos,
            tool_item_id: None,
            predicted: true,
        }),
    );
    game.sim_mut().tick_mining(0, &mut TickEvents::default());
    let cells = &game.session().pending_corrective_cells;
    assert!(
        cells.contains(&pos),
        "a denied break finish must queue the claimed cell for corrective sync"
    );
    assert!(game.session()
        .pending_action_outcomes
        .iter()
        .any(|o| o.id == 9 && !o.accepted));
}

#[test]
fn unpredicted_break_finish_keeps_the_initiators_break_event() {
    let mut game = game_on_empty_chunk();
    let pos = IVec3::new(8, 64, 8);
    assert!(game
        .server_world_mut()
        .set_block_world(pos.x, pos.y, pos.z, Block::Stone));
    game.server_player_mut().pos = WorldPos::new(8.5, 65.0, 10.5);

    let mut u = player_update(&game, true);
    u.break_held = true;
    u.target = Some(hit(pos, IVec3::new(0, 0, 1)));
    game.send_to_server(ClientToServer::PlayerUpdate(u));
    let expected_ticks =
        (petramond_world::mining::break_time(Block::Stone, None) / TICK_DT).round() as usize;
    for _ in 0..expected_ticks - 2 {
        game.sim_mut().tick_mining(0, &mut TickEvents::default());
    }
    // A TRACK-ONLY finish (frozen ledger / replica disagreement): the client
    // never presented, so the accept must not strip its BlockBroken.
    game.send_to_server(ClientToServer::Action(PlayerAction::BreakFinished {
            request_id: 31,
            pos,
            tool_item_id: None,
            predicted: false,
        }),
    );
    game.sim_mut().tick_mining(0, &mut TickEvents::default());
    assert!(
        game.session()
            .pending_action_outcomes
            .iter()
            .any(|o| o.id == 31 && o.accepted),
        "the finish itself still accepts"
    );
    assert!(
        !game.session().presented_breaks.contains(&pos),
        "an unpresented break must not be stripped from the initiator's events"
    );
}

#[test]
fn multi_deny_rollback_is_emission_order_independent() {
    let mut game = game_on_empty_chunk();
    game.server_player_mut().inventory = filled_inventory();
    game.sync_self_view_for_test();
    let before = game.replica.self_view.inventory.clone();

    game.game.drop_selected_item(false); // id 0
    game.game.drop_selected_item(false); // id 1

    // The server may emit an immediate deny for the NEWER id before a
    // tick-time deny for the older one — the restore must still end on the
    // oldest snapshot.
    let update = TickUpdate {
        action_outcomes: vec![
            petramond::net::protocol::ActionOutcome::deny(1, ActionDenyReason::Denied),
            petramond::net::protocol::ActionOutcome::deny(0, ActionDenyReason::Denied),
        ],
        ..Default::default()
    };
    game.game.apply_tick_update(Box::new(update));
    assert_eq!(
        game.replica.self_view
            .inventory
            .slot(game.replica.self_view.inventory.active_slot() as usize),
        before.slot(before.active_slot() as usize),
        "rollback must be allocation-ordered, not emission-ordered"
    );
}
