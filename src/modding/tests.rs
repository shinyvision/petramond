//! Host contract tests: the dispatch round-trip against the real smoke mod,
//! and the failure-policy contracts (disable-on-trap, registration window)
//! against hand-built hostile WAT guests.

use std::path::PathBuf;
use std::process::Command;

use mod_api::{AttachSide, HostCall, Stage as ApiStage};

use crate::block::Block;
use crate::events::{Attach, EventBus, PostEvent, Stage, TickSystems};
use crate::game::TickEvents;
use crate::mathh::{IVec3, Vec3};
use crate::player::Player;
use crate::world::World;

use super::instance::ModInstance;
use super::ModHost;

struct Sim {
    world: World,
    player: Player,
    feed: TickEvents,
    bus: EventBus,
    systems: TickSystems,
}

impl Sim {
    fn new() -> Self {
        Self {
            world: World::new(1, 1),
            player: Player::new(Vec3::new(0.0, 80.0, 0.0)),
            feed: TickEvents::default(),
            bus: EventBus::default(),
            systems: TickSystems::default(),
        }
    }

    fn init(&mut self, host: &mut ModHost) {
        let mut next_spatial_sound_handle = 1;
        host.initialize(
            &mut self.world,
            &mut self.player,
            &mut self.bus,
            &mut self.systems,
            &mut next_spatial_sound_handle,
        );
    }

    fn run_slot(&mut self, at: Attach) {
        self.systems.run(
            at,
            &mut self.world,
            &mut self.player,
            &mut self.feed,
            self.bus.queue_mut(),
        );
    }

    /// Run every stage seam once — one tick's worth of system dispatches
    /// without pinning where a mod chose to attach.
    fn run_all_slots(&mut self) {
        const STAGES: [Stage; Stage::COUNT] = [
            Stage::Mining,
            Stage::Placement,
            Stage::Attack,
            Stage::Drops,
            Stage::Menu,
            Stage::PlayerDamage,
            Stage::WorldScheduled,
            Stage::NaturalBreaks,
            Stage::Pickup,
            Stage::Mobs,
            Stage::ItemPhysics,
            Stage::Spawning,
        ];
        for stage in STAGES {
            self.run_slot(Attach::Before(stage));
            self.run_slot(Attach::After(stage));
        }
    }
}

/// Per-world mod enablement: a disabled pack contributes NO wasm instance to
/// the session — and therefore no tick systems, event handlers, worldgen
/// hooks, or GUI click ownership (all of those exist only through an
/// instance's `mod_init` registrations). Content-only packs never had wasm to
/// gate.
#[test]
fn disabled_packs_contribute_no_wasm_instance() {
    let pack = |name: &str, id: Option<&str>, wasm: Option<&str>| crate::assets::Pack {
        dir: PathBuf::from(format!("/fixture/{name}")),
        name: name.to_owned(),
        id: id.map(str::to_owned),
        version: None,
        description: String::new(),
        summary: None,
        icon: None,
        wasm: wasm.map(PathBuf::from),
    };
    let packs = [
        pack("alpha", Some("alpha"), Some("/fixture/alpha/mod.wasm")),
        pack("content_only", None, None),
        pack("omega", Some("omega"), Some("/fixture/omega/mod.wasm")),
    ];

    let none: std::collections::BTreeSet<String> = Default::default();
    let all_ids: Vec<String> = super::session_wasm_mods(&packs, &none)
        .into_iter()
        .map(|(id, _)| id)
        .collect();
    assert_eq!(all_ids, ["alpha", "omega"], "wasm-bearing packs load");

    let disabled: std::collections::BTreeSet<String> = ["omega".to_owned()].into();
    let ids: Vec<String> = super::session_wasm_mods(&packs, &disabled)
        .into_iter()
        .map(|(id, _)| id)
        .collect();
    assert_eq!(ids, ["alpha"], "the disabled pack's wasm is never selected");
}

/// Build a `mods-src/` crate exactly like `make mods` and return the wasm
/// path, or `None` (with a visible message) when the wasm target isn't
/// installed so plain `cargo test` never hard-fails on machines without it.
pub(crate) fn built_mod_wasm(krate: &str) -> Option<PathBuf> {
    let mods_src = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("mods-src");
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let out = Command::new(cargo)
        .current_dir(&mods_src)
        // The engine's target dir must not capture the guest build.
        .env_remove("CARGO_TARGET_DIR")
        .args([
            "build",
            "--release",
            "--target",
            "wasm32-unknown-unknown",
            "-p",
            krate,
        ])
        .output()
        .expect("spawn cargo for the mod build");
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        if stderr.contains("may not be installed") || stderr.contains("E0463") {
            eprintln!(
                "SKIPPING the '{krate}' wasm test: the wasm32-unknown-unknown target is \
                 missing (install with `rustup target add wasm32-unknown-unknown`)"
            );
            return None;
        }
        panic!("building the '{krate}' mod failed:\n{stderr}");
    }
    Some(mods_src.join(format!(
        "target/wasm32-unknown-unknown/release/{krate}.wasm"
    )))
}

/// The full Phase 2b loop against the REAL smoke mod (built from `mods-src/`
/// like `make mods` does): instantiate → mod_init registers through
/// host_dispatch → tick-system dispatch into the guest (which calls back for
/// current_tick/rng/log) → post-event dispatch — all observed via the host's
/// dispatch/host-call counters, no mod internals pinned.
#[test]
fn smoke_mod_round_trip_via_wasm_host() {
    let Some(wasm) = built_mod_wasm("smoke") else {
        return;
    };
    let mut sim = Sim::new();
    let mut host = ModHost::from_wasm_list(0x312, &[("smoke".into(), wasm)]);
    assert_eq!(host.loaded(), 1, "the smoke wasm instantiates");

    sim.init(&mut host);
    let (disabled, after_init_dispatches, init_stats) = host.probe(0);
    assert!(!disabled, "init leaves the mod healthy");
    assert!(after_init_dispatches >= 1, "mod_init dispatched");
    assert_eq!(
        init_stats.registered, 3,
        "a tick system, an event handler, and a worldgen feature"
    );
    assert_eq!(init_stats.rejected_registrations, 0);
    assert!(
        init_stats.host_calls >= 3,
        "registrations + log flowed through host_dispatch"
    );

    // Tick dispatch: the registered system runs at its seam and calls back in
    // (current_tick at minimum; tick 0 is a heartbeat, so log + rng too).
    sim.run_all_slots();
    let (disabled, after_tick_dispatches, tick_stats) = host.probe(0);
    assert!(!disabled);
    assert!(
        after_tick_dispatches > after_init_dispatches,
        "the tick system dispatched into the guest"
    );
    assert!(
        tick_stats.host_calls > init_stats.host_calls,
        "the tick system made host calls with the SimCtx scope active"
    );

    // Event dispatch: the block_placed observer sees a queued post event.
    sim.bus.emit(PostEvent::BlockPlaced {
        pos: IVec3::new(1, 2, 3),
        block: Block::Stone,
    });
    let Sim {
        world,
        player,
        feed,
        bus,
        ..
    } = &mut sim;
    bus.drain_post(world, player, feed);
    let (disabled, after_event_dispatches, _) = host.probe(0);
    assert!(!disabled, "the whole round trip leaves the mod healthy");
    assert!(
        after_event_dispatches > after_tick_dispatches,
        "the event handler dispatched into the guest"
    );
}

/// Phase 3b guest round trip against the REAL smoke mod: the engine plants
/// `smoke:probe` in the world KV, the guest's tick system reads it (a
/// cross-boundary WorldKvGet), writes `smoke:pong` (an own-namespace
/// WorldKvSet), and spawns an owl (SpawnMob through the 3a registry) — all
/// through wasm `host_dispatch`, observed on the engine-side world.
#[test]
fn smoke_mod_sets_world_kv_and_spawns_a_mob_via_wasm() {
    let Some(wasm) = built_mod_wasm("smoke") else {
        return;
    };
    let mut sim = Sim::new();
    let mut host = ModHost::from_wasm_list(0x77, &[("smoke".into(), wasm)]);
    sim.init(&mut host);

    sim.world.mod_kv_set("smoke:probe".into(), vec![0xAB, 0xCD]);
    sim.run_all_slots();

    let (disabled, _, _) = host.probe(0);
    assert!(!disabled, "the probe round trip leaves the mod healthy");
    assert_eq!(
        sim.world.mod_kv_get("smoke:pong"),
        Some(&[0xABu8, 0xCD][..]),
        "the guest read the probe and echoed it into its own namespace"
    );
    assert_eq!(sim.world.mobs().len(), 1, "the guest spawned a mob");
    assert_eq!(sim.world.mobs().instances()[0].kind, crate::mob::Mob::Owl);

    // One-shot: another tick's worth of dispatches doesn't spawn again.
    sim.run_all_slots();
    assert_eq!(sim.world.mobs().len(), 1);
}

/// Phase 5 guest round trip against the REAL smoke mod: dispatching the
/// `bump` button's `gui_click` makes the guest read-modify-write the session
/// state map (`GuiStateGet` + `GuiStateSet` through wasm `host_dispatch`),
/// observed on the engine-side world. A click on a kind the mod does not own
/// dispatches nothing (owner = the namespace prefix).
#[test]
fn smoke_mod_gui_click_updates_gui_state_via_wasm() {
    let Some(wasm) = built_mod_wasm("smoke") else {
        return;
    };
    let mut sim = Sim::new();
    let mut host = ModHost::from_wasm_list(0x55, &[("smoke".into(), wasm)]);
    sim.init(&mut host);

    let mut click = |host: &mut ModHost, kind: &str, widget: &str| {
        let Sim {
            world,
            player,
            feed,
            bus,
            ..
        } = &mut sim;
        let mut ctx = crate::events::SimCtx {
            world,
            player,
            feed,
            queue: bus.queue_mut(),
        };
        host.dispatch_gui_click(&mut ctx, kind, widget, Some([1, 2, 3]));
    };

    click(&mut host, "smoke:panel", "bump");
    click(&mut host, "smoke:panel", "bump");
    // A kind owned by nobody loaded: no dispatch, no state change.
    let (_, dispatches_before, _) = host.probe(0);
    click(&mut host, "ghostpack:panel", "bump");
    let (disabled, dispatches_after, _) = host.probe(0);
    assert!(!disabled, "the GUI round trip leaves the mod healthy");
    assert_eq!(
        dispatches_after, dispatches_before,
        "a foreign kind dispatches nothing"
    );

    assert_eq!(
        sim.world.gui_state_get("smoke:count"),
        Some(&crate::gui::GuiValue::I32(2)),
        "each click incremented the session counter via GuiStateGet/Set"
    );
    assert_eq!(
        sim.world.gui_state_get("smoke:count_text"),
        Some(&crate::gui::GuiValue::Str("clicks: 2".into())),
        "the label's bound key follows the counter"
    );
}

/// Stage a fixture `mods/` root holding the REAL packs of `ids` with freshly
/// built wasm, for child-process tests that need pack content registry-visible
/// (`LLAMACRAFT_MODS` + the 2a re-spawn pattern). Returns the fixture root
/// (removed by [`run_child_test`]), or `None` when the wasm32 target is
/// missing (the test skips, like [`built_mod_wasm`]).
pub(crate) fn stage_mods_fixture(tag: &str, ids: &[&str]) -> Option<PathBuf> {
    let wasms: Vec<PathBuf> = ids
        .iter()
        .map(|id| built_mod_wasm(id))
        .collect::<Option<_>>()?;
    fn copy_tree(src: &std::path::Path, dst: &std::path::Path) {
        std::fs::create_dir_all(dst).unwrap();
        for entry in std::fs::read_dir(src).unwrap() {
            let entry = entry.unwrap();
            let to = dst.join(entry.file_name());
            if entry.path().is_dir() {
                copy_tree(&entry.path(), &to);
            } else {
                std::fs::copy(entry.path(), &to).unwrap();
            }
        }
    }
    let root =
        std::env::temp_dir().join(format!("llamacraft-fixture-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    for (id, wasm) in ids.iter().zip(&wasms) {
        let dst = root.join("mods").join(id);
        let src = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("mods-src")
            .join(id)
            .join("pack");
        copy_tree(&src, &dst);
        std::fs::copy(wasm, dst.join("mod.wasm")).unwrap();
    }
    Some(root)
}

/// The zombies pack fixture.
pub(crate) fn stage_zombies_fixture(tag: &str) -> Option<PathBuf> {
    stage_mods_fixture(tag, &["zombies"])
}

/// Re-spawn the test binary on `test_path` (an `#[ignore]`d inner test) with
/// `LLAMACRAFT_MODS` pointing at `root/mods`, then clean the fixture up.
pub(crate) fn run_child_test(root: &std::path::Path, test_path: &str) {
    let exe = std::env::current_exe().expect("test binary path");
    let out = std::process::Command::new(exe)
        .arg(test_path)
        .arg("--exact")
        .arg("--ignored")
        .arg("--nocapture")
        .env("LLAMACRAFT_MODS", root.join("mods"))
        .output()
        .expect("spawn test binary");
    let _ = std::fs::remove_dir_all(root);
    assert!(
        out.status.success(),
        "inner test failed\n--- stdout ---\n{}\n--- stderr ---\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
}

/// A guest module implementing the raw ABI by hand: `mod_init` issues one
/// registration host-call (bytes baked into a data segment), and
/// `mod_dispatch` runs `body`. The trivial allocator returns a fixed scratch
/// address — each test drives at most one buffer at a time.
fn hostile_guest(body: &str) -> ModInstance {
    let registration = mod_api::encode(&HostCall::RegisterTickSystem {
        stage: ApiStage::Mining,
        attach: AttachSide::Before,
        priority: 0,
        system_id: 7,
    })
    .unwrap();
    let reg_bytes: String = registration.iter().map(|b| format!("\\{b:02x}")).collect();
    let reg_len = registration.len();
    let wat = format!(
        r#"(module
  (import "env" "host_dispatch" (func $hd (param i32 i32) (result i64)))
  (memory (export "memory") 1)
  (data (i32.const 0) "{reg_bytes}")
  (data (i32.const 512) "\00")
  (func (export "mod_init")
    (drop (call $hd (i32.const 0) (i32.const {reg_len}))))
  (func (export "mod_alloc") (param i32) (result i32) (i32.const 4096))
  (func (export "mod_free") (param i32 i32))
  (func (export "mod_dispatch") (param i32 i32) (result i64)
    {body}))"#,
    );
    let module = wasmtime::Module::new(super::host::engine(), wat.as_bytes())
        .expect("assemble hostile guest");
    ModInstance::from_module("hostile", &module, 1).expect("instantiate hostile guest")
}

/// Contract: a trapping mod is disabled for the session with the tick
/// continuing — later systems in the same slot still run, and the disabled
/// mod receives no further dispatches.
#[test]
fn trapping_mod_is_disabled_and_the_tick_continues() {
    let mut sim = Sim::new();
    let mut host = ModHost::from_instances(vec![hostile_guest("unreachable")]);
    sim.init(&mut host);
    let (disabled, dispatches_after_init, _) = host.probe(0);
    assert!(!disabled, "init succeeded; only dispatch traps");

    // An engine system registered AFTER the mod in the same slot must still
    // run when the mod traps ahead of it.
    let ran_after = std::rc::Rc::new(std::cell::Cell::new(false));
    {
        let ran_after = ran_after.clone();
        sim.systems
            .attach(Attach::Before(Stage::Mining), 0, move |_| {
                ran_after.set(true)
            });
    }

    sim.run_slot(Attach::Before(Stage::Mining));
    let (disabled, dispatches, _) = host.probe(0);
    assert!(disabled, "the trap disabled the mod");
    assert_eq!(
        dispatches, dispatches_after_init,
        "the dispatch never completed"
    );
    assert!(ran_after.get(), "the tick continued past the trapping mod");

    // Still ticking, and the disabled mod is not dispatched again.
    ran_after.set(false);
    sim.run_slot(Attach::Before(Stage::Mining));
    let (_, dispatches_again, _) = host.probe(0);
    assert_eq!(dispatches_again, dispatches);
    assert!(ran_after.get());

    // The bus keeps draining post events normally with a disabled mod around.
    sim.bus.emit(PostEvent::PlayerDied);
    let Sim {
        world,
        player,
        feed,
        bus,
        ..
    } = &mut sim;
    bus.drain_post(world, player, feed);
}

/// Contract: the registration window is `mod_init` only — a registration
/// attempted during a tick dispatch is rejected (HostRet::Error), does not
/// attach anything, and does NOT disable the mod by itself.
#[test]
fn registration_outside_init_is_rejected() {
    // mod_dispatch re-issues the same registration call, ignores the reply,
    // and answers GuestRet::Unit from the staged data segment.
    let body = "(drop (call $hd (i32.const 0) (i32.const 5)))\n    (i64.const 2199023255553)";
    // Verify the literals the WAT hardcodes: the registration payload length
    // and the packed (512, 1) reply address.
    assert_eq!(
        mod_api::encode(&HostCall::RegisterTickSystem {
            stage: ApiStage::Mining,
            attach: AttachSide::Before,
            priority: 0,
            system_id: 7,
        })
        .unwrap()
        .len(),
        5
    );
    assert_eq!(mod_api::pack_ptr_len(512, 1), 2199023255553);

    let mut sim = Sim::new();
    let mut host = ModHost::from_instances(vec![hostile_guest(body)]);
    sim.init(&mut host);
    let (_, _, stats) = host.probe(0);
    assert_eq!(stats.registered, 1, "the init-window registration counted");

    sim.run_slot(Attach::Before(Stage::Mining));
    let (disabled, _, stats) = host.probe(0);
    assert!(!disabled, "a rejected call is an error reply, not a trap");
    assert_eq!(stats.rejected_registrations, 1);
    assert_eq!(stats.registered, 1, "nothing new was accepted");

    // Nothing got attached: the slot still holds exactly the one system from
    // init — dispatching it again yields exactly one more rejection.
    sim.run_slot(Attach::Before(Stage::Mining));
    let (_, _, stats) = host.probe(0);
    assert_eq!(stats.rejected_registrations, 2);
}
