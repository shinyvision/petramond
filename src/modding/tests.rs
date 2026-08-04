//! Host contract tests: failure-policy contracts (disable-on-trap,
//! registration window) against hand-built hostile WAT guests, plus fixture
//! helpers for real bundled mods.

use std::path::PathBuf;
use std::process::Command;

use mod_api::{AttachSide, HostCall, Stage as ApiStage};

use crate::events::{Attach, EventBus, PostEvent, Stage, TickSystems};
use crate::events::tick::TickEvents;
use crate::mathh::Vec3;
use crate::player::Player;
use crate::world::World;

use super::instance::ModInstance;
use super::ModHost;

struct Sim {
    world: World,
    player: Player,
    gui_state: std::sync::Arc<crate::gui_state::GuiStateMap>,
    feed: TickEvents,
    bus: EventBus,
    systems: TickSystems,
}

impl Sim {
    fn new() -> Self {
        Self {
            world: World::new(1, 1),
            player: Player::new(Vec3::new(0.0, 80.0, 0.0)),
            gui_state: crate::gui_state::empty_gui_state(),
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
            &mut self.gui_state,
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
            &mut self.gui_state,
            &mut self.feed,
            self.bus.queue_mut(),
        );
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
        client_wasm: None,
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

/// Build a `mods-src/` crate for test with the `playtest` profile and return
/// the wasm path, or `None` (with a visible message) when the wasm target
/// isn't installed so plain `cargo test` never hard-fails on machines without
/// it. Shipped `make mods` builds remain release-profile work, never tests.
pub fn built_mod_wasm(krate: &str) -> Option<PathBuf> {
    let mods_src = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("mods-src");
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let out = Command::new(cargo)
        .current_dir(&mods_src)
        // The engine's target dir must not capture the guest build.
        .env_remove("CARGO_TARGET_DIR")
        .args([
            "build",
            "--profile",
            "playtest",
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
        "target/wasm32-unknown-unknown/playtest/{krate}.wasm"
    )))
}

/// Stage a fixture `mods/` root holding the REAL packs of `ids` with freshly
/// built wasm, for child-process tests that need pack content registry-visible
/// (`PETRAMOND_MODS` + the 2a re-spawn pattern). Returns the fixture root
/// (removed by [`run_child_test`]), or `None` when the wasm32 target is
/// missing (the test skips, like [`built_mod_wasm`]).
pub fn stage_mods_fixture(tag: &str, ids: &[&str]) -> Option<PathBuf> {
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
    let root = std::env::temp_dir().join(format!("petramond-fixture-{tag}-{}", std::process::id()));
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

/// Re-spawn the test binary on `test_path` (an `#[ignore]`d inner test) with
/// `PETRAMOND_MODS` pointing at `root/mods`, then clean the fixture up.
/// `PETRAMOND_DATA_DIR` is pinned to this process's shared test root (the one
/// the app tests use): saves stay out of the developer's real data dir, and
/// the disk module cache there lets every child after the first deserialize
/// precompiled mod modules (~1 ms) instead of recompiling them (~1 s).
pub fn run_child_test(root: &std::path::Path, test_path: &str) {
    let exe = std::env::current_exe().expect("test binary path");
    let out = std::process::Command::new(exe)
        .arg(test_path)
        .arg("--exact")
        .arg("--ignored")
        .arg("--nocapture")
        .env("PETRAMOND_MODS", root.join("mods"))
        .env(
            "PETRAMOND_DATA_DIR",
            std::env::temp_dir().join(format!("petramond-test-data-{}", std::process::id())),
        )
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
    hostile_guest_with_id("hostile", body)
}

/// [`hostile_guest`] under a caller-chosen mod id, so a test can target the
/// id-keyed [`super::host::HOST_CALL_TEST_HOOK`] without touching other
/// tests' guests.
fn hostile_guest_with_id(id: &str, body: &str) -> ModInstance {
    guest_with_data(id, "", body)
}

/// Where [`calling_guest`] stages its call payloads: clear of the registration
/// blob at 0 and of the fixed `mod_alloc` scratch at 4096 (host replies land
/// there, and they must not overwrite a later call's bytes).
const CALL_STAGE_ADDR: u32 = 1024;

/// The shared guest template: `mod_init` issues one registration host-call,
/// `mod_dispatch` runs `body`, and `extra_data` adds whatever data segments
/// the body reads from.
fn guest_with_data(id: &str, extra_data: &str, body: &str) -> ModInstance {
    let registration = mod_api::encode(&HostCall::RegisterTickSystem {
        stage: ApiStage::Mining,
        attach: AttachSide::Before,
        priority: 0,
        system_id: 7,
    })
    .unwrap();
    let reg_bytes = wat_bytes(&registration);
    let reg_len = registration.len();
    let wat = format!(
        r#"(module
  (import "env" "host_dispatch" (func $hd (param i32 i32) (result i64)))
  (memory (export "memory") 1)
  (data (i32.const 0) "{reg_bytes}")
  (data (i32.const 512) "\00")
{extra_data}  (func (export "mod_init")
    (drop (call $hd (i32.const 0) (i32.const {reg_len}))))
  (func (export "mod_alloc") (param i32) (result i32) (i32.const 4096))
  (func (export "mod_free") (param i32 i32))
  (func (export "mod_dispatch") (param i32 i32) (result i64)
    {body}))"#,
    );
    let module = wasmtime::Module::new(super::host::engine(), wat.as_bytes())
        .expect("assemble hostile guest");
    ModInstance::from_module(id, &module, 1).expect("instantiate hostile guest")
}

fn wat_bytes(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("\\{b:02x}")).collect()
}

/// A guest whose every dispatch ISSUES `calls`, in order, and then answers
/// `GuestRet::Unit`. The payloads are the real postcard encodings baked into
/// data segments, so the engine decodes exactly what a compiled mod's SDK
/// would have written and nothing in a test can route around the ABI — which
/// is what makes this a composition fixture rather than a second call site for
/// `handle_host_call`.
fn calling_guest(id: &str, calls: &[HostCall]) -> ModInstance {
    let mut data = String::new();
    let mut body = String::new();
    let mut at = CALL_STAGE_ADDR;
    for call in calls {
        let bytes = mod_api::encode(call).expect("encode a staged host call");
        data.push_str(&format!(
            "  (data (i32.const {at}) \"{}\")\n",
            wat_bytes(&bytes)
        ));
        body.push_str(&format!(
            "(drop (call $hd (i32.const {at}) (i32.const {})))\n    ",
            bytes.len()
        ));
        at += bytes.len() as u32;
    }
    assert!(at < 4096, "staged calls run into the reply scratch");
    guest_with_data(id, &data, &format!("{body}(i64.const 2199023255553)"))
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
    let ran_after = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    {
        let ran_after = ran_after.clone();
        sim.systems
            .attach(Attach::Before(Stage::Mining), 0, move |_| {
                ran_after.store(true, std::sync::atomic::Ordering::Relaxed)
            });
    }

    sim.run_slot(Attach::Before(Stage::Mining));
    let (disabled, dispatches, _) = host.probe(0);
    assert!(disabled, "the trap disabled the mod");
    assert_eq!(
        dispatches, dispatches_after_init,
        "the dispatch never completed"
    );
    assert!(
        ran_after.load(std::sync::atomic::Ordering::Relaxed),
        "the tick continued past the trapping mod"
    );

    // Still ticking, and the disabled mod is not dispatched again.
    ran_after.store(false, std::sync::atomic::Ordering::Relaxed);
    sim.run_slot(Attach::Before(Stage::Mining));
    let (_, dispatches_again, _) = host.probe(0);
    assert_eq!(dispatches_again, dispatches);
    assert!(ran_after.load(std::sync::atomic::Ordering::Relaxed));

    // The bus keeps draining post events normally with a disabled mod around.
    sim.bus.emit(PostEvent::PlayerDied);
    let Sim {
        world,
        player,
        gui_state,
        feed,
        bus,
        ..
    } = &mut sim;
    bus.drain_post(world, player, gui_state, feed);
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

/// Contract: a guest spinning on host calls forever is stopped by the
/// per-dispatch host-call cap (host-call time is deliberately not charged
/// against the epoch deadline, so the call count is what bounds this shape
/// of runaway) and disabled for the session.
#[test]
fn host_call_spinning_dispatch_is_disabled_by_the_call_cap() {
    let mut instance = hostile_guest_with_id(
        "spinny",
        "(loop $spin (drop (call $hd (i32.const 0) (i32.const 5))) (br $spin))\n    (i64.const 0)",
    );
    instance.call_init_detached();
    assert!(!instance.disabled());

    let ret = instance.call_guest_detached(&mod_api::GuestCall::TickSystem { id: 7 });
    assert!(ret.is_none());
    assert!(instance.disabled(), "the call cap disabled the mod");
    // The cap (not the epoch deadline) is what fired: exactly MAX calls were
    // handled during the dispatch, plus init's one registration.
    assert_eq!(
        instance.stats().host_calls,
        1 + super::host::DISPATCH_HOST_CALL_MAX as u64
    );
}

/// Contract: the dispatch watchdog charges GUEST compute only. A host call
/// that stalls for many epochs (a slow storage read, an I/O hiccup) must not
/// get the mod disabled, while a runaway guest loop still traps. Runs in a
/// child process because it advances the process-wide engine epoch, which
/// could spuriously trap unrelated guests in parallel tests.
#[test]
fn watchdog_charges_guest_compute_only() {
    run_isolated("modding::tests::watchdog_charges_guest_compute_only_inner");
}

#[test]
#[ignore] // run by watchdog_charges_guest_compute_only in a child process
fn watchdog_charges_guest_compute_only_inner() {
    // Every host call from "stally" stalls for triple the whole deadline.
    fn stall() {
        super::host::test_advance_epochs(super::host::DISPATCH_DEADLINE_EPOCHS * 3);
    }
    *super::host::HOST_CALL_TEST_HOOK.lock().unwrap() = Some(("stally".into(), stall));
    let mut instance = hostile_guest_with_id(
        "stally",
        "(drop (call $hd (i32.const 0) (i32.const 5)))\n    \
         (drop (call $hd (i32.const 0) (i32.const 5)))\n    \
         (drop (call $hd (i32.const 0) (i32.const 5)))\n    \
         (i64.const 2199023255553)",
    );
    instance.call_init_detached();
    assert!(!instance.disabled(), "init survived a stalling host call");
    let ret = instance.call_guest_detached(&mod_api::GuestCall::TickSystem { id: 7 });
    assert!(
        ret.is_some() && !instance.disabled(),
        "three multi-deadline host-call stalls did not disable the mod"
    );

    // A guest that never yields still traps: advance the epoch from a helper
    // thread (standing in for the real-time ticker) while it spins.
    std::thread::spawn(|| {
        for _ in 0..1000 {
            std::thread::sleep(std::time::Duration::from_millis(2));
            super::host::test_advance_epochs(10);
        }
    });
    let mut runaway =
        hostile_guest_with_id("runaway", "(loop $spin (br $spin))\n    (i64.const 0)");
    runaway.call_init_detached();
    let ret = runaway.call_guest_detached(&mod_api::GuestCall::TickSystem { id: 7 });
    assert!(
        ret.is_none() && runaway.disabled(),
        "the runaway loop trapped"
    );
}

/// A [`Sim`] holding one placed multi-cell model block, with its group anchor
/// and a NON-anchor footprint cell — the cell a mod would address it by.
fn sim_with_a_placed_machine() -> (Sim, crate::mathh::IVec3, crate::mathh::IVec3) {
    use crate::mathh::IVec3;

    let mut sim = Sim::new();
    sim.world.clear_world();
    sim.world
        .insert_empty_column_for_test(crate::chunk::ChunkPos::new(0, 0));
    let anchor = IVec3::new(5, 64, 5);
    assert!(
        sim.world
            .place_model_block(anchor, crate::block::Block::FurnitureWorkbench),
        "fixture: a multi-cell model block places"
    );
    let cells = sim.world.model_group(anchor).expect("a placed group").2;
    let addressed = *cells.last().expect("a footprint cell");
    assert!(
        cells.len() > 1 && addressed != anchor,
        "fixture: multi-cell, and addressed by a cell that is NOT the anchor"
    );
    (sim, anchor, addressed)
}

/// The parts mask stored at `c`.
fn parts_mask_at(world: &World, c: crate::mathh::IVec3) -> Option<u32> {
    world
        .cell_kv_get(c.x, c.y, c.z, crate::block_model::PARTS_KV_KEY)
        .map(<[u8; 4]>::try_from)
        .and_then(Result::ok)
        .map(u32::from_le_bytes)
}

/// The two presentation calls a machine makes every tick, addressed at `pos`.
fn dressing_calls(pos: crate::mathh::IVec3, parts: u32, tint: [u8; 3]) -> Vec<HostCall> {
    let item = crate::registry::names()
        .items
        .name(crate::item::ItemType::Dirt.id())
        .expect("fixture: a registered item")
        .to_owned();
    let pos = [pos.x, pos.y, pos.z];
    vec![
        HostCall::SetModelParts {
            pos,
            parts,
            tint: Some(tint),
        },
        HostCall::SetBlockDraw {
            pos,
            prims: vec![
                mod_api::DrawPrim::Cuboid {
                    min: [0.2, 0.0, 0.2],
                    max: [0.8, 0.5, 0.8],
                    tile: "stone".into(),
                    tint: [255, 0, 0],
                    emissive: true,
                },
                mod_api::DrawPrim::Item {
                    at: [0.5, 0.6, 0.5],
                    scale: 0.4,
                    yaw: 0.0,
                    pitch: 0.0,
                    item,
                    tint: [255, 255, 255],
                },
            ],
        },
    ]
}

/// The presentation seams IN COMPOSITION, driven by a real guest across the
/// real ABI: a mod dresses a placed multi-cell machine from inside a tick
/// dispatch, and a client joining afterwards ends up holding the same picture.
///
/// Every seam here has a unit test of its own; composition is where they
/// disagree. The parts mask lands on EVERY FOOTPRINT CELL and the draw set at
/// the group ANCHOR alone, both are addressed by whichever cell the mod
/// happens to have, and the two reach a joiner by different routes — cell KV
/// rides the section's own states, while a draw set is a world-level record
/// the section payload has to go and fetch. A test per seam proves each rule;
/// only running them together proves they are the same rule.
#[test]
fn a_guest_dresses_a_placed_machine_and_a_joiner_sees_it() {
    const PARTS: u32 = 0b101;
    const TINT: [u8; 3] = [12, 200, 34];

    let (mut sim, anchor, addressed) = sim_with_a_placed_machine();
    // The engine's own namespace: these calls dress the CALLER'S OWN block, and
    // the only multi-cell model row a bare test registry has is an engine one —
    // so the guest stands in for the pack that would ship the machine.
    let mut host = ModHost::from_instances(vec![calling_guest(
        crate::registry::ENGINE_NAMESPACE,
        &dressing_calls(addressed, PARTS, TINT),
    )]);
    sim.init(&mut host);
    sim.run_slot(Attach::Before(Stage::Mining));
    assert!(
        !host.probe(0).0,
        "the guest's calls were replies, not traps"
    );

    for &c in &sim.world.model_group(anchor).expect("still placed").2 {
        assert_eq!(parts_mask_at(&sim.world, c), Some(PARTS), "{c:?}");
        assert_eq!(
            sim.world
                .cell_kv_get(c.x, c.y, c.z, crate::block::TINT_KV_KEY),
            Some(&TINT[..]),
            "{c:?} takes the tint with the mask"
        );
    }
    let set = sim
        .world
        .block_draw_at(anchor)
        .expect("the set is keyed at the group ANCHOR");
    assert_eq!(set.resolved.len(), 2, "both prims resolved");
    assert!(
        sim.world.block_draw_at(addressed).is_none(),
        "a set under a non-anchor cell would never be hit-tested nor forgotten on break"
    );

    // A client joining now streams the section, which is the ONLY way a
    // machine that last redrew itself before the join can reach it.
    let mut replica = World::new_with_pool(
        0,
        1,
        crate::world::WorldRole::ClientReplica,
        std::sync::Arc::new(crate::worker::JobPool::new(1)),
    );
    let cp = crate::chunk::ChunkPos::new(0, 0);
    replica.install_remote_column(sim.world.column_payload(cp).expect("a column payload"));
    let sp = crate::chunk::SectionPos::from_world(anchor.x, anchor.y, anchor.z).expect("in range");
    replica.install_remote_section(sim.world.section_payload(sp).expect("a loaded section"));

    assert_eq!(
        parts_mask_at(&replica, anchor),
        Some(PARTS),
        "the joiner's model wears the mask"
    );
    assert_eq!(
        replica
            .block_draw_at(anchor)
            .expect("the joiner sees the drawing")
            .resolved
            .len(),
        2
    );
}

/// The ownership gate holds through the whole dispatch, not just at the
/// handler: a pack dresses ITS OWN machine and nothing else. Same guest, same
/// calls, a foreign namespace — and the machine stays undressed rather than
/// half dressed (the two calls are separate host calls, so a gate applied to
/// one of them only is a machine wearing another pack's parts).
#[test]
fn a_foreign_mod_cannot_dress_someone_elses_machine() {
    let (mut sim, anchor, addressed) = sim_with_a_placed_machine();
    let mut host = ModHost::from_instances(vec![calling_guest(
        "intruder",
        &dressing_calls(addressed, 0b111, [9, 9, 9]),
    )]);
    sim.init(&mut host);
    sim.run_slot(Attach::Before(Stage::Mining));

    assert!(
        !host.probe(0).0,
        "a refused call is an error reply, not a trap"
    );
    assert_eq!(parts_mask_at(&sim.world, anchor), None);
    assert!(sim.world.block_draw_at(anchor).is_none());
}

/// Contract: the disable-message diagnostics stay bounded — a call carrying a
/// multi-hundred-KiB payload must not render byte-by-byte into the log line.
#[test]
fn short_debug_bounds_large_payloads() {
    let call = HostCall::ClientImageSet {
        key: "m:img".into(),
        width: 256,
        height: 256,
        rgba: vec![7; 256 * 256 * 4],
    };
    let rendered = super::host::short_debug(&call, 160);
    assert!(rendered.starts_with("ClientImageSet"));
    assert!(rendered.ends_with('…') && rendered.len() <= 164);
}

/// Re-spawn the test binary on `test_path` (an `#[ignore]`d inner test) so it
/// runs alone in a fresh process.
fn run_isolated(test_path: &str) {
    let exe = std::env::current_exe().expect("test binary path");
    let out = Command::new(exe)
        .args([test_path, "--exact", "--ignored", "--nocapture"])
        .output()
        .expect("spawn test binary");
    assert!(
        out.status.success(),
        "inner test failed\n--- stdout ---\n{}\n--- stderr ---\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
}
