//! Contract tests for the entity + self replication batches (multiplayer
//! Phase C2c-i): the pump emits `TickUpdate`s, the client's replicated stores
//! feed presentation with prev/curr interpolation pairs, absent ids drop, the
//! inventory rides a `SelfState` only when its revision moved, and the HUD
//! read models mirror session truth through the batch — never by direct read.

use super::super::presentation::GamePresentationScratch;
use super::super::tick::{TickEvents, TICK_DT};
use super::common::{count_item, filled_inventory, game, install_empty_chunk};
use crate::controls::PointerButton;
use crate::entity::DroppedItem;
use crate::events::DamageSource;
use crate::gui::{CraftHit, MenuSlot};
use crate::item::{ItemStack, ItemType};
use crate::mathh::Vec3;
use crate::mob::Mob;
use crate::net::protocol::MobStateRow;

/// One pump that must have executed at least one fixed tick, returning its
/// replication batch (the trailing `Tick` message of the pump's output).
fn pump_one_tick(game: &mut super::common::TestGame) -> Box<crate::net::protocol::TickUpdate> {
    let mut inbox = Vec::new();
    let out = game.server.pump(TICK_DT, &mut inbox);
    out.msgs
        .into_iter()
        .find_map(|msg| match msg {
            crate::net::protocol::ServerToClient::Tick(update) => Some(update),
            _ => None,
        })
        .expect("a full tick's dt executes a tick and emits a batch")
}

fn mob_row(id: u64, pos: Vec3, hurt_timer: f32) -> MobStateRow {
    MobStateRow {
        id,
        kind_id: Mob::Owl.0,
        pos,
        yaw: 0.0,
        anim_time: 0.0,
        moving: false,
        idle_anim: None,
        head_yaw: 0.0,
        head_pitch: 0.0,
        hurt_timer,
        dead: false,
        shorn: false,
        ragdoll: None,
    }
}

/// Store semantics: a fresh id starts with prev == curr, a repeated id shifts
/// curr→prev, and an id absent from a batch is dropped.
#[test]
fn replicated_store_pairs_consecutive_batches_and_drops_absent_ids() {
    let mut store = crate::game::replicated::ReplicatedMobs::default();
    let p1 = Vec3::new(1.0, 70.0, 1.0);
    let p2 = Vec3::new(1.5, 69.0, 1.0);

    store.apply(vec![mob_row(7, p1, 0.3), mob_row(9, p1, 0.0)]);
    let fresh = store.iter().find(|e| e.curr.id == 7).expect("stored");
    assert_eq!(fresh.prev.pos, p1, "a fresh id interpolates from itself");
    assert_eq!(store.len(), 2);

    store.apply(vec![mob_row(7, p2, 0.25)]);
    assert_eq!(store.len(), 1, "id 9 was absent from the batch: dropped");
    let paired = store.iter().next().expect("id 7 kept");
    assert_eq!(paired.prev.pos, p1, "previous batch became the prev row");
    assert_eq!(paired.curr.pos, p2);
    assert_eq!(paired.prev.hurt_timer, 0.3);
    assert_eq!(paired.curr.hurt_timer, 0.25);
}

/// Two pumped batches feed the presentation path: `collect_mobs` reads the
/// REPLICATED store and yields prev/curr rows matching the two batches (the
/// interpolation source the renderer blends), with the replicated kind.
#[test]
fn pumped_mob_batches_become_interpolated_presentation_rows() {
    let mut game = game();
    install_empty_chunk(&mut game);
    game.server.sessions[0].player.pos = Vec3::new(8.5, 64.0, 8.5);
    // An owl in free fall right above the player: gravity guarantees its
    // position differs between consecutive ticks.
    assert!(game
        .server
        .world
        .spawn_mob(Mob::Owl, Vec3::new(8.5, 70.0, 8.5), 0.0));
    let id = game.server.world.mobs().instances()[0].id();

    let batch1 = pump_one_tick(&mut game);
    let row1 = batch1
        .mobs
        .iter()
        .find(|m| m.id == id)
        .cloned()
        .expect("the owl replicates");
    game.apply_tick_update(batch1);

    let batch2 = pump_one_tick(&mut game);
    let row2 = batch2
        .mobs
        .iter()
        .find(|m| m.id == id)
        .cloned()
        .expect("still replicating");
    assert_ne!(row1.pos, row2.pos, "the falling owl moved between ticks");
    game.apply_tick_update(batch2);

    let mut scratch = GamePresentationScratch::new();
    let presentation = scratch.snapshot(&game);
    let row = presentation
        .mobs
        .iter()
        .find(|m| m.id == id)
        .expect("presentation reads the replicated store");
    assert_eq!(row.kind, Mob::Owl);
    assert_eq!(row.prev_pos, row1.pos, "prev = previous batch state");
    assert_eq!(row.pos, row2.pos, "curr = latest batch state");
}

/// A killed/despawned mob vanishes from the next batch, so its id drops from
/// the store and the presentation rows.
#[test]
fn a_despawned_mob_drops_from_the_store_on_the_next_batch() {
    let mut game = game();
    install_empty_chunk(&mut game);
    game.server.sessions[0].player.pos = Vec3::new(8.5, 64.0, 8.5);
    assert!(game
        .server
        .world
        .spawn_mob(Mob::Owl, Vec3::new(8.5, 70.0, 8.5), 0.0));
    let id = game.server.world.mobs().instances()[0].id();

    let batch = pump_one_tick(&mut game);
    game.apply_tick_update(batch);
    assert!(game.replicated_mobs.iter().any(|e| e.curr.id == id));

    let index = game
        .server
        .world
        .mobs()
        .index_of_id(id)
        .expect("still alive server-side");
    assert!(game.server.world.mobs_mut().remove(index));
    let batch = pump_one_tick(&mut game);
    game.apply_tick_update(batch);

    assert!(
        !game.replicated_mobs.iter().any(|e| e.curr.id == id),
        "an id absent from the batch drops from the store"
    );
    let mut scratch = GamePresentationScratch::new();
    let presentation = scratch.snapshot(&game);
    assert!(
        !presentation.mobs.iter().any(|m| m.id == id),
        "and from the presentation rows"
    );
}

/// Dropped items replicate through the same path: batch rows carry the stable
/// per-spawn id, and presentation reads the replicated store.
#[test]
fn dropped_items_replicate_with_stable_ids_into_presentation() {
    let mut game = game();
    install_empty_chunk(&mut game);
    // Far from the player so no pickup interferes; above the floor so it moves
    // (falls) between ticks.
    let mut drop = DroppedItem::new(
        Vec3::new(2.5, 70.0, 2.5),
        ItemStack::new(ItemType::Dirt, 3),
        1,
    );
    drop.vel = Vec3::ZERO;
    game.server.world.spawn_item(drop);
    let id = game.server.world.item_entities()[0].id;
    assert_ne!(id, 0, "entering the active set assigns a stable id");

    let batch1 = pump_one_tick(&mut game);
    let row1 = *batch1
        .items
        .iter()
        .find(|i| i.id == id)
        .expect("the drop replicates");
    assert_eq!(row1.item_id, ItemType::Dirt.0);
    assert_eq!(row1.count, 3);
    game.apply_tick_update(batch1);

    let batch2 = pump_one_tick(&mut game);
    let row2 = *batch2
        .items
        .iter()
        .find(|i| i.id == id)
        .expect("still replicating");
    game.apply_tick_update(batch2);

    let mut scratch = GamePresentationScratch::new();
    let presentation = scratch.snapshot(&game);
    let row = presentation
        .item_entities
        .iter()
        .find(|i| i.item == ItemType::Dirt)
        .expect("presentation reads the replicated store");
    assert_eq!(row.prev_pos, row1.pos, "prev = previous batch state");
    assert_eq!(row.pos, row2.pos, "curr = latest batch state");
    assert_eq!(row.count, 3);
}

/// The inventory revision moves on every mutation class the HUD cares about:
/// pickup, menu click, drop, and craft.
#[test]
fn pickup_menu_click_drop_and_craft_each_bump_the_inventory_revision() {
    let mut game = game();
    install_empty_chunk(&mut game);
    let rev = |game: &super::common::TestGame| game.server.sessions[0].player.inventory.revision();

    // Pickup: an eligible drop at the body centre is collected in one tick.
    game.server.sessions[0].player.pos = Vec3::new(8.5, 64.0, 8.5);
    let mut drop = DroppedItem::new(
        game.server.sessions[0].player.body_center(),
        ItemStack::new(ItemType::Dirt, 2),
        1,
    );
    drop.ticks_lived = crate::world::ITEM_PICKUP_DELAY_TICKS;
    game.server.world.spawn_item(drop);
    let before = rev(&game);
    assert!(game.server.item_pickup_tick(0), "the drop was collected");
    assert_ne!(rev(&game), before, "a pickup bumps the revision");
    assert_eq!(count_item(game.inventory(), ItemType::Dirt), 2);

    // Menu click: picking the stack up onto the cursor.
    let before = rev(&game);
    game.menu_click(MenuSlot::Inventory(0), PointerButton::Primary, false, false);
    game.apply_latched_actions_for_test();
    assert!(game.inventory().cursor().is_some(), "stack on the cursor");
    assert_ne!(rev(&game), before, "a menu click bumps the revision");
    // Put it back for the drop below.
    game.menu_click(MenuSlot::Inventory(0), PointerButton::Primary, false, false);
    game.apply_latched_actions_for_test();

    // Drop: Q drops one of the selected stack.
    game.server.sessions[0].player.inventory = filled_inventory();
    let before = rev(&game);
    game.drop_selected_item(false);
    game.apply_latched_actions_for_test();
    assert_eq!(count_item(game.inventory(), ItemType::Dirt), 63);
    assert_ne!(rev(&game), before, "a drop bumps the revision");

    // Craft: log → cursor → grid, then take the planks result.
    game.server.sessions[0]
        .player
        .inventory
        .add(ItemStack::new(ItemType::OakLog, 1));
    game.open_crafting(2);
    for slot in [
        MenuSlot::Inventory(1), // the log landed after the dirt stack
        MenuSlot::Craft(CraftHit::Input(0)),
    ] {
        game.menu_click(slot, PointerButton::Primary, false, false);
        game.apply_latched_actions_for_test();
    }
    let before = rev(&game);
    game.menu_click(
        MenuSlot::Craft(CraftHit::Result),
        PointerButton::Primary,
        false,
        false,
    );
    game.apply_latched_actions_for_test();
    assert_eq!(
        game.inventory().cursor().map(|s| s.item),
        Some(ItemType::OakPlanks),
        "the craft result landed on the cursor"
    );
    assert_ne!(
        rev(&game),
        before,
        "taking a craft result bumps the revision"
    );
}

/// The full inventory rides a `SelfState` only when the revision moved —
/// always on the first update after join, then only after a change.
#[test]
fn self_state_ships_the_inventory_only_when_the_revision_moved() {
    let mut game = game();
    install_empty_chunk(&mut game);

    let up1 = pump_one_tick(&mut game);
    let s1 = up1.self_state.as_ref().expect("self state every batch");
    assert!(
        s1.inventory.is_some(),
        "the first update after join always carries the inventory"
    );
    assert_eq!(
        s1.inventory.as_ref().map(|v| v.len()),
        Some(crate::inventory::TOTAL_SLOTS + 1),
        "36 slots + the cursor"
    );

    let up2 = pump_one_tick(&mut game);
    let s2 = up2.self_state.as_ref().expect("self state every batch");
    assert!(
        s2.inventory.is_none(),
        "an unchanged revision ships no inventory body"
    );

    game.server.sessions[0]
        .player
        .inventory
        .add(ItemStack::new(ItemType::Stone, 5));
    let up3 = pump_one_tick(&mut game);
    let s3 = up3.self_state.as_ref().expect("self state every batch");
    let slots = s3
        .inventory
        .as_ref()
        .expect("a mutation re-ships the inventory");
    assert_eq!(
        slots[0].map(|w| (w.item_id, w.count)),
        Some((ItemType::Stone.0, 5))
    );
}

// ---- C2c-iii session-boundary contracts: events + menu sync on the wire ----

/// Chest viewer-count transitions emit `ChestOpened`/`ChestClosed` world
/// events ONLY at the 0↔1 boundaries — a second overlapping viewer opens and
/// closes silently.
#[test]
fn chest_viewer_transitions_emit_events_only_at_zero_boundaries() {
    use crate::block::Block;
    use crate::mathh::IVec3;

    let mut game = super::common::game();
    super::common::install_empty_chunk(&mut game);
    let pos = IVec3::new(3, 64, 3);
    game.server.world.set_block_world(3, 64, 3, Block::Chest);
    game.server
        .world
        .insert_chest(pos, crate::block_model::DEFAULT_MODEL_FACING);
    let s1 = game
        .server
        .add_session_for_test(crate::player::Player::new(Vec3::new(2.5, 64.0, 2.5)));

    let mut ev = TickEvents::default();
    game.server.open_chest_screen_for(0, pos, &mut ev);
    assert_eq!(ev.world.chest_changed, vec![(pos, true)], "0→1 opens");
    game.server.open_chest_screen_for(s1, pos, &mut ev);
    assert_eq!(ev.world.chest_changed.len(), 1, "1→2 emits nothing");
    game.server.close_open_menu_for(0, &mut ev);
    assert_eq!(ev.world.chest_changed.len(), 1, "2→1 emits nothing");
    game.server.close_open_menu_for(s1, &mut ev);
    assert_eq!(
        ev.world.chest_changed,
        vec![(pos, true), (pos, false)],
        "1→0 closes, exactly once"
    );
}

/// A SECOND session's chest interaction (the tick-side open) reaches session
/// 0's replication batch: `open_chests` gains the chest and exactly one
/// `ChestOpened` event rides `events` — while session 0's own `self_events`
/// carries no open-screen (it wasn't the opener).
#[test]
fn a_remote_sessions_chest_open_reaches_the_local_batch_exactly_once() {
    use crate::block::Block;
    use crate::mathh::IVec3;
    use crate::net::protocol::WorldEventMsg;

    let mut game = super::common::game();
    super::common::install_empty_chunk(&mut game);
    let pos = IVec3::new(3, 64, 3);
    game.server.world.set_block_world(3, 64, 3, Block::Chest);
    game.server
        .world
        .insert_chest(pos, crate::block_model::DEFAULT_MODEL_FACING);
    let s1 = game
        .server
        .add_session_for_test(crate::player::Player::new(Vec3::new(2.5, 64.0, 2.5)));

    // Session 1 right-clicked the chest (latched edge + look, as its
    // PlayerUpdate/UseClick messages would leave them).
    game.server.sessions[s1].look = Some(super::common::hit(pos, IVec3::Y));
    game.server.sessions[s1].pending_place = true;

    let update = pump_one_tick(&mut game);
    assert!(
        update.open_chests.contains(&pos),
        "the other player's open lifts the replicated lid set"
    );
    let opened: Vec<_> = update
        .events
        .iter()
        .filter(|e| matches!(e, WorldEventMsg::ChestOpened { pos: p } if *p == pos))
        .collect();
    assert_eq!(opened.len(), 1, "exactly one ChestOpened event broadcast");
    assert_eq!(
        update.self_events.open_screen, None,
        "the non-opening recipient gets no open-screen one-shot"
    );
}

/// `menu_sync` rides a batch only when the menu view CHANGED: the first batch
/// ships the initial (closed) view, an unchanged menu ships `None`, and a
/// tick-side open ships the new target once.
#[test]
fn menu_sync_ships_on_change_only() {
    use crate::block::Block;
    use crate::mathh::IVec3;
    use crate::net::protocol::MenuTargetWire;

    let mut game = super::common::game();
    super::common::install_empty_chunk(&mut game);
    let pos = IVec3::new(3, 64, 3);
    game.server.world.set_block_world(3, 64, 3, Block::Chest);
    game.server
        .world
        .insert_chest(pos, crate::block_model::DEFAULT_MODEL_FACING);

    let up1 = pump_one_tick(&mut game);
    let sync = up1
        .menu_sync
        .expect("the first batch ships the initial view");
    assert_eq!(sync.target, MenuTargetWire::None);
    let up2 = pump_one_tick(&mut game);
    assert!(
        up2.menu_sync.is_none(),
        "unchanged (closed) menu ships nothing"
    );

    let mut ev = TickEvents::default();
    game.server.open_chest_screen_for(0, pos, &mut ev);
    let up3 = pump_one_tick(&mut game);
    let sync = up3.menu_sync.expect("the open ships the new view");
    assert!(
        matches!(sync.target, MenuTargetWire::Chest { pos: p, .. } if p == pos),
        "the chest target replicates, got {:?}",
        sync.target
    );
    let up4 = pump_one_tick(&mut game);
    assert!(
        up4.menu_sync.is_none(),
        "a still-open, untouched chest ships nothing"
    );
}

/// The mod-GUI state map rides `menu_sync` only when its `Arc` changed: once
/// at open (the cleared map), once per tick-side write (copy-on-write forces
/// a fresh allocation), never in between.
#[test]
fn gui_state_ships_in_menu_sync_only_on_arc_change() {
    use crate::gui::GuiValue;
    use crate::net::protocol::{GuiValueWire, MenuTargetWire};

    let mut game = super::common::game();
    game.set_mods_for_test(crate::modding::ModHost::test_unit_guest_host("modtest"));
    let kind = crate::gui::intern_kind("modtest:panel").expect("mod kind registers");
    game.server.open_mod_gui_screen_for(0, kind, None);

    let up = pump_one_tick(&mut game);
    let MenuTargetWire::ModGui { gui_state, .. } = up.menu_sync.expect("the open ships").target
    else {
        panic!("expected a ModGui target");
    };
    assert_eq!(
        gui_state,
        Some(Vec::new()),
        "a fresh session ships its cleared (empty) map"
    );

    let up = pump_one_tick(&mut game);
    assert!(up.menu_sync.is_none(), "no writes → no sync");

    // What a mod's GuiStateSet HostCall does on the tick: a copy-on-write
    // write against the session's map.
    crate::gui::gui_state_set(
        &mut game.server.sessions[0].gui_state,
        "modtest:v".into(),
        GuiValue::I32(7),
    );
    let up = pump_one_tick(&mut game);
    let MenuTargetWire::ModGui { gui_state, .. } =
        up.menu_sync.expect("the write ships a sync").target
    else {
        panic!("expected a ModGui target");
    };
    assert_eq!(
        gui_state,
        Some(vec![("modtest:v".into(), GuiValueWire::I32(7))]),
        "the changed map rides whole"
    );

    let up = pump_one_tick(&mut game);
    assert!(up.menu_sync.is_none(), "same Arc → nothing ships");
}

#[test]
fn host_written_mod_gui_state_syncs_to_matching_remote_session() {
    use crate::gui::GuiValue;
    use crate::net::protocol::{GuiValueWire, MenuTargetWire};

    let mut game = super::common::game();
    game.set_mods_for_test(crate::modding::ModHost::test_unit_guest_host("kitchen"));
    let remote = game
        .server
        .add_session_for_test(crate::player::Player::new(Vec3::new(2.5, 64.0, 2.5)));
    let kind = crate::gui::intern_kind("kitchen:oven").expect("mod kind registers");
    let pos = crate::mathh::IVec3::new(4, 64, 4);

    game.server.open_mod_gui_screen_for(remote, kind, Some(pos));
    crate::gui::gui_state_set(
        &mut game.server.sessions[0].gui_state,
        "kitchen:cook01".into(),
        GuiValue::F32(0.5),
    );

    let MenuTargetWire::ModGui { gui_state, .. } = game
        .server
        .build_menu_sync(remote)
        .expect("remote menu sync includes the shared mod GUI state")
        .target
    else {
        panic!("expected a ModGui target");
    };
    assert_eq!(
        gui_state,
        Some(vec![("kitchen:cook01".into(), GuiValueWire::F32(0.5))]),
        "a single-instance mod machine's gauge publish reaches the remote opener"
    );
}

/// The screen-open request queued at the tick's interaction site arrives as
/// `SelfEvents.open_screen` and `Game::tick` maps it onto the app-facing
/// `GameEvents` field unchanged-consumer-side.
#[test]
fn open_screen_one_shot_maps_back_onto_game_events() {
    use crate::game::GameInput;
    use crate::mathh::IVec3;

    let mut game = super::common::game();
    super::common::install_empty_chunk(&mut game);
    let pos = IVec3::new(3, 64, 3);
    // The tick's request site (interaction arm) writes this outbox field;
    // seed it directly to isolate the SelfEvents → GameEvents pipe.
    game.server.sessions[0].request_open_chest = Some(pos);

    let events = game.tick(TICK_DT, &GameInput::default());
    assert_eq!(
        events.open_chest,
        Some(pos),
        "the one-shot rode SelfEvents.open_screen into GameEvents"
    );
    assert!(
        game.server.sessions[0].request_open_chest.is_none(),
        "the request outbox is consumed by the batch"
    );

    let events = game.tick(TICK_DT, &GameInput::default());
    assert_eq!(events.open_chest, None, "one-shots don't repeat");
}

/// Player block placement and (mined) breaks broadcast position-carrying
/// `WorldEventMsg`s alongside the recipient's own lossy one-shots.
#[test]
fn placement_and_mined_breaks_broadcast_world_events_with_positions() {
    use crate::block::Block;
    use crate::mathh::IVec3;
    use crate::net::protocol::WorldEventMsg;

    let mut game = super::common::game();
    super::common::install_empty_chunk(&mut game);
    game.server.sessions[0].player.pos = Vec3::new(8.5, 64.0, 8.5);
    let floor = IVec3::new(3, 63, 3);
    game.server
        .world
        .set_block_world(floor.x, floor.y, floor.z, Block::Stone);
    game.server.sessions[0].player.inventory = filled_inventory(); // Dirt in slot 0

    // Place: a latched use click against the floor's top face.
    game.server.sessions[0].look = Some(super::common::hit(floor, IVec3::Y));
    game.server.sessions[0].pending_place = true;
    let update = pump_one_tick(&mut game);
    let placed_at = floor + IVec3::Y;
    assert!(
        update.events.iter().any(|e| matches!(
            e,
            WorldEventMsg::BlockPlaced { pos, block_id }
                if *pos == placed_at && *block_id == Block::Dirt.0
        )),
        "the placement broadcast its position, got {:?}",
        update.events
    );
    assert_eq!(update.self_events.placed_block, Some(Block::Dirt.0));

    // Break: hold the primary button on the placed dirt until it gives.
    game.server.sessions[0].look = Some(super::common::hit(placed_at, IVec3::Y));
    game.server.sessions[0].intent_gameplay = true;
    game.server.sessions[0].intent_break_held = true;
    let mut broke = None;
    for _ in 0..200 {
        let update = pump_one_tick(&mut game);
        if let Some(ev) = update.events.iter().find_map(|e| match e {
            WorldEventMsg::BlockBroken { pos, block_id, .. } => Some((*pos, *block_id)),
            _ => None,
        }) {
            assert_eq!(update.self_events.broke_block, Some(Block::Dirt.0));
            broke = Some(ev);
            break;
        }
    }
    assert_eq!(
        broke,
        Some((placed_at, Block::Dirt.0)),
        "the mined break broadcast its position"
    );
}

/// The HUD reads the replicated self view, and after a damage tick's batch it
/// matches session truth exactly.
#[test]
fn hud_health_matches_session_truth_after_a_damage_tick() {
    let mut game = game();
    install_empty_chunk(&mut game);

    let mut events = TickEvents::default();
    assert!(game
        .server
        .damage_player(0, 3, DamageSource::Fall, None, &mut events));
    let update = pump_one_tick(&mut game);
    game.apply_tick_update(update);

    let hud = game.player_health().expect("survival draws hearts");
    assert_eq!(
        hud.current,
        game.server.sessions[0].player.health(),
        "the replicated HUD health equals the session's"
    );
    assert_eq!(hud.current, crate::player::MAX_HEALTH - 3);
}

/// Phase D: the server thread can outpace a slow frame, so several
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
    first.self_events.swung_hand = true;
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
        ev.self_events.swung_hand && ev.self_events.player_damaged,
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

// ---- Phase F: remote-player replication ----

/// Every connected session's player row rides every recipient's batch: a
/// second (remote-shaped) session's transform reaches session 0's
/// `TickUpdate`, alongside session 0's own row.
#[test]
fn every_sessions_player_row_reaches_the_local_batch() {
    let mut game = game();
    install_empty_chunk(&mut game);
    let s1_pos = Vec3::new(2.5, 64.0, 2.5);
    let s1 = game
        .server
        .add_session_for_test(crate::player::Player::new(s1_pos));
    let s1_id = game.server.sessions[s1].id;

    let update = pump_one_tick(&mut game);
    assert!(
        update
            .players
            .iter()
            .any(|p| p.id == game.server.sessions[0].id),
        "the recipient's own row ships too (the client skips it)"
    );
    let row = update
        .players
        .iter()
        .find(|p| p.id == s1_id)
        .expect("the second session's row rides the first session's batch");
    assert_eq!(row.pos, s1_pos, "players only move client-side: exact echo");
    assert!(row.alive && row.visible);
    assert!(!row.sleeping && row.sleep_yaw.is_none());
    assert!(!row.snap, "no tick-side teleport happened");
}

/// A sleeping session's row carries the server-computed lying head yaw (the
/// bed's base→pillow direction) and flags the tuck teleport as a snap.
#[test]
fn a_sleeping_sessions_row_carries_the_lying_head_yaw() {
    use crate::block::Block;
    use crate::mathh::IVec3;

    let mut game = game();
    install_empty_chunk(&mut game);
    for x in 0..16 {
        for z in 0..16 {
            game.server.world.set_block_world(x, 63, z, Block::Stone);
        }
    }
    let base = IVec3::new(7, 64, 7);
    assert!(game.server.world.place_model_block(base, Block::Bed));
    // Night gate: the core day/night system republishes only at tick END
    // (After(Spawning)), so the flag survives until the Placement stage.
    game.server
        .world
        .mod_kv_set("petramond:is_night".into(), vec![1]);
    game.server.sessions[0].look = Some(super::common::hit(base, IVec3::Y));
    game.server.sessions[0].pending_place = true;

    let update = pump_one_tick(&mut game);
    let row = update
        .players
        .iter()
        .find(|p| p.id == game.server.sessions[0].id)
        .expect("own row ships");
    assert!(row.sleeping, "the bed interaction started the sleep");
    let (_, _, cells) = game.server.world.model_group(base).expect("bed group");
    let other = cells
        .iter()
        .copied()
        .find(|c| *c != base)
        .expect("two-cell bed");
    let d = other - base;
    assert_eq!(
        row.sleep_yaw,
        Some((d.x as f32).atan2(d.z as f32)),
        "the lying head yaw points from the bed base toward the pillow"
    );
    assert!(row.snap, "the tuck teleport must snap interpolation");
}

/// Fix: the day/night shader params are written into the SERVER world's
/// environment, but the renderer reads the CLIENT replica's — the batch must
/// carry them across. Driving full frames, the replica's param map converges
/// on the server's exactly.
#[test]
fn shader_params_replicate_into_the_replica_environment() {
    let mut game = game();
    for _ in 0..3 {
        game.tick(TICK_DT, &crate::game::GameInput::default());
    }
    let server_params = game.server.world.environment().shader_params().clone();
    assert!(
        server_params.contains_key(crate::server::daynight::SKY_TIME_PARAM)
            && server_params.contains_key(crate::server::daynight::SKY_LIGHT_PARAM),
        "day/night published its params server-side"
    );
    let replica_params = game.game.replica.environment().shader_params().clone();
    assert_eq!(
        *replica_params, *server_params,
        "the replica environment mirrors the server's param map"
    );
}

/// The env rides a batch only when the map's VALUES changed since the last
/// shipped copy: the first ticked window ships the full set, a windowless
/// re-read ships `None`, and the next tick (day/night advanced) ships again.
#[test]
fn env_params_ship_on_change_and_none_when_static() {
    let mut game = game();
    let update = pump_one_tick(&mut game);
    let shipped = update
        .env
        .expect("the first batch carries the full param map");
    assert!(
        shipped
            .iter()
            .any(|(k, _)| k == crate::server::daynight::SKY_TIME_PARAM),
        "the day/night keys ride the batch: {shipped:?}"
    );

    let ev = TickEvents::default();
    let quiet = game.server.shared_tick_rows(&ev);
    assert!(
        quiet.env.is_none(),
        "an unchanged param map ships None (keep)"
    );

    let update = pump_one_tick(&mut game);
    assert!(
        update.env.is_some(),
        "the next tick moved the day/night params: the full set ships again"
    );
}

/// Remote break overlays: presentation collects the own crack (replicated
/// self view) PLUS one per visible remote row with a mining target, each at
/// its own stage; an invisible remote (spectator/dead) draws none.
#[test]
fn break_overlays_collect_own_and_visible_remote_miners() {
    use crate::mathh::IVec3;
    use crate::net::protocol::PlayerStateRow;
    use crate::server::player::PlayerId;
    use std::collections::HashMap;

    fn row(id: u8, mining: Option<(IVec3, u8)>, visible: bool) -> PlayerStateRow {
        PlayerStateRow {
            id: PlayerId(id),
            pos: Vec3::new(4.0, 64.0, 4.0),
            vel: Vec3::ZERO,
            yaw: 0.0,
            pitch: 0.0,
            on_ground: true,
            sneaking: false,
            sleeping: false,
            sleep_yaw: None,
            alive: visible,
            visible,
            held_item: None,
            mining,
            eating: false,
            hurt_recent: false,
            snap: false,
        }
    }

    let mut game = game();
    game.game.self_view.mining = Some((IVec3::new(1, 64, 1), 4));
    let own_id = game.game.self_id;
    let rows = [
        row(1, Some((IVec3::new(3, 64, 3), 7)), true),
        row(2, Some((IVec3::new(5, 64, 5), 2)), false), // hidden: no overlay
        row(3, None, true),                             // not mining: no overlay
    ];
    game.game
        .remote_players
        .apply(&rows, &[], own_id, &HashMap::new());

    let mut scratch = GamePresentationScratch::new();
    let presentation = scratch.snapshot(&game);
    let overlays = presentation.break_overlays;
    assert_eq!(overlays.len(), 2, "own + the one visible remote miner");
    assert!(
        overlays
            .iter()
            .any(|o| o.block == IVec3::new(1, 64, 1) && o.stage == 4),
        "the own overlay keeps its target + stage"
    );
    assert!(
        overlays
            .iter()
            .any(|o| o.block == IVec3::new(3, 64, 3) && o.stage == 7),
        "the remote row's overlay carries ITS stage"
    );
}
