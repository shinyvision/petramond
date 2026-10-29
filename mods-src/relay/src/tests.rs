use super::*;
use crate::relay::next_move;
use crate::testing::*;

const AT: [i32; 3] = [0, 5, 0];
const ABOVE: [i32; 3] = [0, 6, 0];
const BELOW: [i32; 3] = [0, 4, 0];
const ROW: &str = r#"{"interval": 8, "stack": 16}"#;

fn stack_of(count: u8) -> Option<ItemStackData> {
    Some(stack(STONE, count))
}

/// A relay at `AT` with `ROW`, as `init` builds it.
fn relay(session: &Session) -> Relay {
    session.world.place(AT, RELAY);
    session.world.row(RELAY, ROW);
    let mut relay = Relay::default();
    relay.init();
    relay
}

fn tick(relay: &mut Relay, kind: BlockHookKind) {
    relay.block_hooks(RELAY_HOOK, &[(kind, AT)]);
}

#[test]
fn every_declared_pack_id_is_shipped() {
    pack_check::assert_declared(&[keys::PACK_KEYS]);
}

#[test]
fn shipped_relay_rows_parse() {
    let rows = pack_rows_with_data(include_str!("../pack/blocks.json"), "blocks", TRANSFER_KEY);
    assert!(!rows.is_empty());
    for (block, raw) in &rows {
        let t = parse_row_data::<Transfer>(raw).unwrap_or_else(|e| panic!("{block}: {e}"));
        assert!(t.interval > 0 && t.stack > 0, "{block}");
    }
}

#[test]
fn a_move_takes_the_first_stack_capped_at_the_row() {
    assert_eq!(
        next_move(&[None, stack_of(40), stack_of(3)], 16),
        Some((1, 16))
    );
    assert_eq!(next_move(&[None, stack_of(5)], 16), Some((1, 5)));
}

#[test]
fn an_empty_container_moves_nothing() {
    assert_eq!(next_move(&[], 16), None);
    assert_eq!(next_move(&[None, None], 16), None);
    assert_eq!(next_move(&[stack_of(0)], 16), None);
}

// --- step, through block_hooks ---

#[test]
fn a_tick_moves_one_capped_stack_and_schedules_the_interval() {
    let session = Session::new();
    let mut relay = relay(&session);
    session.world.chest(ABOVE, 3);
    session.world.chest(BELOW, 3);
    session.world.put(ABOVE, 0, STONE, 40);
    tick(&mut relay, BlockHookKind::ScheduledTick);
    assert_eq!(session.world.count(ABOVE, STONE), 24);
    assert_eq!(session.world.count(BELOW, STONE), 16);
    assert_eq!(session.world.ticks(), vec![(AT, 8)]);
}

#[test]
fn a_never_opened_target_chest_receives_the_items() {
    let session = Session::new();
    let mut relay = relay(&session);
    session.world.chest(ABOVE, 3);
    session.world.unopened_chest(BELOW, 3);
    session.world.put(ABOVE, 0, STONE, 10);
    assert!(!session.world.has_container(BELOW));
    tick(&mut relay, BlockHookKind::ScheduledTick);
    assert_eq!(session.world.count(BELOW, STONE), 10);
    assert_eq!(session.world.count(ABOVE, STONE), 0);
    assert_eq!(session.world.ticks(), vec![(AT, 8)]);
}

#[test]
fn a_missing_source_moves_nothing_and_waits_the_idle_interval() {
    let session = Session::new();
    let mut relay = relay(&session);
    session.world.chest(BELOW, 3);
    tick(&mut relay, BlockHookKind::ScheduledTick);
    assert_eq!(session.world.count(BELOW, STONE), 0);
    assert_eq!(session.world.ticks(), vec![(AT, 64)]);
}

#[test]
fn a_missing_target_keeps_the_items_and_reschedules() {
    let session = Session::new();
    let mut relay = relay(&session);
    session.world.chest(ABOVE, 3);
    session.world.put(ABOVE, 0, STONE, 10);
    tick(&mut relay, BlockHookKind::ScheduledTick);
    assert_eq!(session.world.count(ABOVE, STONE), 10);
    assert!(!session.world.has_container(BELOW));
    assert_eq!(session.world.ticks(), vec![(AT, 64)]);
}

#[test]
fn a_target_block_without_a_container_keeps_the_items() {
    let session = Session::new();
    let mut relay = relay(&session);
    session.world.chest(ABOVE, 3);
    session.world.put(ABOVE, 0, STONE, 10);
    session.world.place(BELOW, BlockId(1));
    tick(&mut relay, BlockHookKind::ScheduledTick);
    assert_eq!(session.world.count(ABOVE, STONE), 10);
    assert_eq!(session.world.ticks(), vec![(AT, 64)]);
}

#[test]
fn a_full_target_loses_nothing() {
    let session = Session::new();
    let mut relay = relay(&session);
    session.world.chest(ABOVE, 3);
    session.world.chest(BELOW, 1);
    session.world.put(ABOVE, 0, STONE, 16);
    session.world.put(BELOW, 0, STONE, 64);
    tick(&mut relay, BlockHookKind::ScheduledTick);
    assert_eq!(session.world.count(ABOVE, STONE), 16);
    assert_eq!(session.world.count(BELOW, STONE), 64);
    assert_eq!(session.world.ticks(), vec![(AT, 64)]);
}

#[test]
fn a_partly_full_target_takes_what_fits_and_the_rest_stays() {
    let session = Session::new();
    let mut relay = relay(&session);
    session.world.chest(ABOVE, 3);
    session.world.chest(BELOW, 1);
    session.world.put(ABOVE, 0, STONE, 16);
    session.world.put(BELOW, 0, STONE, 59);
    tick(&mut relay, BlockHookKind::ScheduledTick);
    assert_eq!(session.world.count(BELOW, STONE), 64);
    assert_eq!(session.world.count(ABOVE, STONE), 11);
    assert_eq!(session.world.ticks(), vec![(AT, 8)]);
}

#[test]
fn a_picky_target_refuses_other_items() {
    let session = Session::new();
    let mut relay = relay(&session);
    session.world.chest(ABOVE, 3);
    session.world.chest(BELOW, 3);
    session.world.only(BELOW, STONE);
    session.world.put(ABOVE, 0, DIRT, 16);
    tick(&mut relay, BlockHookKind::ScheduledTick);
    assert_eq!(session.world.count(ABOVE, DIRT), 16);
    assert_eq!(session.world.count(BELOW, DIRT), 0);
    assert_eq!(session.world.ticks(), vec![(AT, 64)]);
}

#[test]
fn an_idle_interval_in_the_row_sets_the_wait() {
    let session = Session::new();
    session.world.place(AT, RELAY);
    session.world.row(
        RELAY,
        r#"{"interval": 8, "stack": 16, "idle_interval": 20}"#,
    );
    let mut relay = Relay::default();
    relay.init();
    tick(&mut relay, BlockHookKind::ScheduledTick);
    assert_eq!(session.world.ticks(), vec![(AT, 20)]);
}

// --- hooks ---

#[test]
fn a_random_tick_restarts_the_timer_without_moving_anything() {
    let session = Session::new();
    let mut relay = relay(&session);
    session.world.chest(ABOVE, 3);
    session.world.chest(BELOW, 3);
    session.world.put(ABOVE, 0, STONE, 10);
    tick(&mut relay, BlockHookKind::RandomTick);
    assert_eq!(session.world.count(ABOVE, STONE), 10);
    assert_eq!(session.world.ticks(), vec![(AT, 8)]);
}

#[test]
fn a_tick_for_a_removed_relay_stops_the_chain() {
    let session = Session::new();
    let mut relay = relay(&session);
    session.world.place(AT, BlockId(0));
    tick(&mut relay, BlockHookKind::ScheduledTick);
    tick(&mut relay, BlockHookKind::RandomTick);
    assert!(session.world.ticks().is_empty());
}

#[test]
fn other_hook_kinds_do_nothing() {
    let session = Session::new();
    let mut relay = relay(&session);
    tick(&mut relay, BlockHookKind::NeighborUpdate);
    tick(&mut relay, BlockHookKind::ContainerChanged);
    assert!(session.world.ticks().is_empty());
}

#[test]
fn another_behavior_callback_is_ignored() {
    let session = Session::new();
    let mut relay = relay(&session);
    relay.block_hooks(RELAY_HOOK + 1, &[(BlockHookKind::ScheduledTick, AT)]);
    assert!(session.world.ticks().is_empty());
}

// --- events ---

#[test]
fn placing_a_relay_schedules_its_first_tick() {
    let session = Session::new();
    let mut relay = relay(&session);
    let mut payload = EventPayload::BlockPlaced {
        pos: AT,
        block: RELAY,
    };
    relay.handle_event(ON_BLOCK_PLACED, &mut payload);
    assert_eq!(session.world.ticks(), vec![(AT, 8)]);
}

#[test]
fn placing_another_block_schedules_nothing() {
    let session = Session::new();
    let mut relay = relay(&session);
    let mut payload = EventPayload::BlockPlaced {
        pos: AT,
        block: CHEST,
    };
    relay.handle_event(ON_BLOCK_PLACED, &mut payload);
    assert!(session.world.ticks().is_empty());
}

// --- init ---

#[test]
fn init_registers_the_behavior_and_the_placed_handler() {
    let session = Session::new();
    relay(&session);
    assert_eq!(
        session.world.state().registered,
        vec!["behavior", "handler"]
    );
}

#[test]
fn init_skips_an_invalid_row_by_name_and_keeps_the_rest() {
    let session = Session::new();
    session.world.row(RELAY, r#"{"interval": 0, "stack": 16}"#);
    session.world.row(CHEST, ROW);
    let mut relay = Relay::default();
    relay.init();
    let logs = session.world.logs();
    assert!(
        logs.iter()
            .any(|l| l.contains("relay:relay") && l.contains("skipped")),
        "{logs:?}"
    );
    session.world.place(AT, RELAY);
    session.world.place([1, 5, 0], CHEST);
    relay.hooks(&[
        (BlockHookKind::RandomTick, AT),
        (BlockHookKind::RandomTick, [1, 5, 0]),
    ]);
    assert_eq!(session.world.ticks(), vec![([1, 5, 0], 8)]);
}

#[test]
fn init_with_no_valid_rows_stays_idle() {
    let session = Session::new();
    session.world.row(RELAY, r#"{"interval": 8, "stack": 0}"#);
    let mut relay = Relay::default();
    relay.init();
    assert!(session.world.state().registered.is_empty());
    assert!(session
        .world
        .logs()
        .iter()
        .any(|l| l.contains("stays idle")));
}

#[test]
fn init_with_no_rows_stays_idle() {
    let session = Session::new();
    let mut relay = Relay::default();
    relay.init();
    assert!(session.world.state().registered.is_empty());
}

#[test]
fn init_warns_about_a_stack_above_the_item_limit_but_keeps_the_row() {
    let session = Session::new();
    session.world.row(RELAY, r#"{"interval": 8, "stack": 200}"#);
    let mut relay = Relay::default();
    relay.init();
    assert!(!relay.is_idle());
    assert!(session
        .world
        .logs()
        .iter()
        .any(|l| l.contains("relay:relay") && l.contains("above the largest item stack")));
}

#[test]
fn a_row_with_an_unknown_field_is_not_kept() {
    let session = Session::new();
    session
        .world
        .row(RELAY, r#"{"interval": 8, "stack": 16, "speed": 3}"#);
    let mut relay = Relay::default();
    relay.init();
    assert!(relay.is_idle());
}
