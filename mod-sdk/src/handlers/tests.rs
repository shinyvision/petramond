use super::*;
use crate::testing::MockHost;
use crate::{CoreCall, HostCall};

#[derive(Default)]
struct Counter {
    typed_ticks: u32,
    raw_ticks: Vec<u32>,
    events: u32,
    hooks: Vec<[i32; 3]>,
}

impl Mod for Counter {
    fn init(&mut self) {
        // A power user's hand-numbered registration beside the typed ones.
        crate::register_tick_system(Stage::Mining, AttachSide::Before, 0, 5);
    }

    fn tick_system(&mut self, system_id: u32) {
        self.raw_ticks.push(system_id);
    }
}

impl TypedMod for Counter {
    fn register(&mut self, on: &mut Registrar<Self>) {
        on.on_tick(Stage::Mining, AttachSide::After, 0, |s| s.typed_ticks += 1);
        on.on::<event_types::ModEvent>(0, |s, _| {
            s.events += 1;
            Outcome::Cancel
        });
        on.on::<event_types::BlockPlaced>(0, |s, _| {
            s.events += 100;
            Outcome::Continue
        });
        on.block_hook("counter:pot", |s, _, pos| s.hooks.push(pos));
    }
}

fn mod_event() -> EventPayload {
    EventPayload::ModEvent {
        key: "counter:ping".into(),
        data: Vec::new(),
    }
}

#[test]
fn typed_registrations_allocate_ids_and_dispatch_to_their_closures() {
    let host = MockHost::new();
    let mut m = Typed::<Counter>::default();
    host.run(|| {
        m.init();
        m.tick_system(TYPED_ID_BASE);
        m.tick_system(5);
        assert!(matches!(
            m.handle_event(TYPED_ID_BASE, &mut mod_event()),
            Outcome::Cancel
        ));
        m.block_hook(TYPED_ID_BASE, BlockHookKind::RandomTick, [1, 2, 3]);
    });
    assert!(host.calls().iter().any(|call| matches!(
        call,
        HostCall::Core(CoreCall::RegisterTickSystem {
            system_id: TYPED_ID_BASE,
            attach: AttachSide::After,
            ..
        })
    )));
    assert!(host.calls().iter().any(|call| matches!(
        call,
        HostCall::Core(CoreCall::RegisterEventHandler {
            handler_id,
            event: EventKind::BlockPlaced,
            ..
        }) if *handler_id == TYPED_ID_BASE + 1
    )));
    assert_eq!(m.state.typed_ticks, 1);
    assert_eq!(m.state.raw_ticks, vec![5], "a raw id reaches the raw hook");
    assert_eq!(m.state.events, 1);
    assert_eq!(m.state.hooks, vec![[1, 2, 3]]);
}

#[test]
fn mismatched_and_unknown_dispatches_are_logged_once_not_dropped_silently() {
    let host = MockHost::new();
    let mut m = Typed::<Counter>::default();
    host.run(|| {
        m.init();
        // The BlockPlaced handler must never see another kind's payload.
        for _ in 0..3 {
            assert!(matches!(
                m.handle_event(TYPED_ID_BASE + 1, &mut mod_event()),
                Outcome::Continue
            ));
        }
        m.tick_system(TYPED_ID_BASE + 9);
        m.tick_system(TYPED_ID_BASE + 9);
    });
    assert_eq!(m.state.events, 0);
    let logs = host.logs();
    assert_eq!(logs.len(), 2, "one line per unhandled (lane, id): {logs:?}");
    assert!(logs[0].contains("ModEvent") && logs[0].contains("BlockPlaced"));
    assert!(logs[1].contains("no typed tick system"));
}

#[test]
#[should_panic(expected = "registered twice")]
fn a_duplicate_key_fails_at_registration() {
    let host = MockHost::new();
    let mut handlers = Handlers::<Counter>::default();
    host.run(|| {
        handlers.ai_node("counter:think", |_, _| None);
        handlers.ai_node("counter:think", |_, _| None);
    });
}
