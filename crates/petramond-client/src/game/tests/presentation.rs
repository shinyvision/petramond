use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::Instant;

use mod_api::capture::{
    decode_documented, parse_record_head, record_envelope, records, ClientCapturedClock,
    ClientEnvelope, ClientEventsPhase, ClientPieceKind,
};
use mod_api::{
    ClientFileRange, ClientFileRanges, ClientPose, ClientStateSelect, ClientStorageScope, HostCall,
    HostRet,
};

use super::common::{game_for_world, TestGame};
use crate::game::{Game, GameInput};
use petramond::modding::client::present::Request;
use petramond::modding::client::ClientModRuntime;
use petramond_math::world_pos::WorldPos;
use petramond_render::camera::Camera;

const DT: f32 = 0.05;
const MOD: &str = "minimap";

fn listing(dir: &Path) -> BTreeMap<PathBuf, (u64, std::time::SystemTime)> {
    let mut out = BTreeMap::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(meta) = entry.metadata() else { continue };
            if meta.is_dir() {
                stack.push(path);
            } else {
                out.insert(path, (meta.len(), meta.modified().unwrap()));
            }
        }
    }
    out
}

fn inert_guest(dir: &Path) -> PathBuf {
    let unit = mod_api::encode(&mod_api::GuestRet::Unit).unwrap();
    let bytes: String = unit.iter().map(|b| format!("\\{b:02x}")).collect();
    let packed = (1024u64 << 32) | unit.len() as u64;
    let abi = petramond::modding::wat_abi_exports();
    let wat = format!(
        r#"(module
  (import "env" "host_dispatch" (func $hd (param i32 i32) (result i64)))
  (memory (export "memory") 1)
  (data (i32.const 1024) "{bytes}")
{abi}  (func (export "mod_init") (param i32 i64))
  (func (export "mod_alloc") (param i32) (result i32) (i32.const 4096))
  (func (export "mod_free") (param i32 i32))
  (func (export "mod_dispatch") (param i32 i32) (result i64) (i64.const {packed})))"#
    );
    let path = dir.join("inert.wat");
    std::fs::write(&path, wat).unwrap();
    path
}

struct Captured {
    dir: &'static str,
    state: [u64; 2],
    tables: [u64; 2],
    tick: u64,
    events: [u64; 2],
    seed: u32,
    eye: [f64; 3],
}

fn call(game: &mut Game, call: HostCall) -> HostRet {
    game.client_call_for_test(MOD, call)
        .expect("the capturing mod is loaded")
}

struct Live {
    game: TestGame,
    look: Option<f32>,
}

impl Live {
    fn new(world: &str) -> Self {
        Self {
            game: game_for_world(world),
            look: None,
        }
    }

    fn frame(&mut self) {
        self.game.tick(DT, &GameInput::default());
        let view = self
            .look
            .filter(|_| self.game.capture_wants_view())
            .map(|yaw| {
                let motion = self.game.local_motion(0.0);
                let frame = self.game.client_frame(0.0);
                let mut cue = self.game.eye_view_cue(
                    [&frame.held_item, &frame.off_hand_item],
                    &frame.animator,
                    &[],
                    petramond::capture::view::CueShake::default(),
                    &motion,
                );
                cue.yaw = yaw;
                cue
            });
        self.game.capture_frame_end(view);
    }
}

fn capture(live: &mut Live, dir: &'static str, frames: u32) -> Captured {
    let game = &mut live.game;
    let HostRet::ClientStateTicket(written) = call(
        game,
        HostCall::from(mod_api::calls::ClientWorldStateWrite {
            scope: ClientStorageScope::Pack,
            path: format!("{dir}/world.pmc"),
            select: ClientStateSelect::All,
            kinds: None,
            envelopes: None,
        }),
    ) else {
        panic!("the state write is accepted");
    };
    let HostRet::Ticket(log) = call(
        game,
        HostCall::from(mod_api::calls::ClientWorldEventsBegin {
            scope: ClientStorageScope::Pack,
            path: format!("{dir}/events.pmc"),
            envelopes: None,
        }),
    ) else {
        panic!("the events log begins");
    };
    for _ in 0..frames {
        live.frame();
    }
    let game = &mut live.game;
    call(
        game,
        HostCall::from(mod_api::calls::ClientWorldEventsEnd { events: log }),
    );
    let deadline = Instant::now() + petramond_util::test_time::TEST_HARD_DEADLINE;
    let mut state = None;
    loop {
        let game = &mut live.game;
        if state.is_none() {
            match call(
                game,
                HostCall::from(mod_api::calls::ClientFilePoll {
                    ticket: written.write,
                }),
            ) {
                HostRet::ClientFilePolled(None) => {}
                HostRet::ClientFilePolled(Some(mod_api::ClientFileAnswer::Done {
                    range, ..
                })) => state = range,
                other => panic!("the state reaches the disk, not {other:?}"),
            }
        }
        let report = call(
            game,
            HostCall::from(mod_api::calls::ClientWorldEventsPoll { events: log }),
        );
        let ended = matches!(
            &report,
            HostRet::ClientEvents(Some(report)) if report.phase == ClientEventsPhase::Ended
        );
        if ended && state.is_some() {
            assert!(
                matches!(&report, HostRet::ClientEvents(Some(r)) if r.frames > 0 && r.error.is_none()),
                "the log ran and wrote its frames: {report:?}"
            );
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the capture never reached the disk"
        );
        live.frame();
    }
    let folder = petramond::modding::client::pack_files_dir_for_test(MOD).join(dir);
    let world = std::fs::read(folder.join("world.pmc")).unwrap();
    let events = std::fs::read(folder.join("events.pmc")).unwrap();
    let state = state.expect("checked above");
    let record = &world[state[0] as usize..(state[0] + state[1]) as usize];
    let ClientEnvelope::State(envelope) = record_envelope(record).unwrap() else {
        panic!("a state record");
    };
    let piece = |kind| {
        let info = envelope.pieces.iter().find(|p| p.kind == kind).unwrap();
        [state[0] + info.range[0], info.range[1]]
    };
    let tables = piece(ClientPieceKind::Tables);
    let [at, len] = piece(ClientPieceKind::Clock);
    let clock: ClientCapturedClock =
        decode_documented(&world[at as usize..(at + len) as usize]).unwrap();
    let whole = records(&events, 0)
        .collect::<Result<Vec<_>, _>>()
        .expect("every record whole")
        .len();
    assert!(whole > 10, "the log holds the frames it ran: {whole}");
    let first_batch = records(&events, 0).find_map(|record| {
        let (at, head) = record.unwrap();
        match record_envelope(&events[at as usize..(at + head.len) as usize]).unwrap() {
            ClientEnvelope::Frame(frame) => frame.batches.first().copied(),
            ClientEnvelope::State(_) => None,
        }
    });
    assert_eq!(
        first_batch,
        Some(clock.tick + 1),
        "the log starts with the batch right after the state's"
    );
    assert!(parse_record_head(&events).is_ok());
    Captured {
        dir,
        state,
        tables,
        tick: clock.tick,
        events: [0, events.len() as u64],
        seed: live.game.replica.world.data().seed,
        eye: live.game.local.player.eye().to_array(),
    }
}

fn present(shell: ClientModRuntime, captured: &Captured) -> Game {
    let mut shell = shell;
    let opened = shell.call_as_for_test(
        MOD,
        None,
        HostCall::from(mod_api::calls::ClientPresentationOpen {
            tables: ClientFileRange {
                scope: ClientStorageScope::Pack,
                path: format!("{}/world.pmc", captured.dir),
                offset: captured.tables[0],
                len: captured.tables[1],
            },
            seed: captured.seed,
            mods: Vec::new(),
            viewer: Some(ClientPose {
                pos: captured.eye,
                yaw: 0.0,
                pitch: 0.0,
            }),
        }),
    );
    assert_eq!(opened, Some(HostRet::Bool(true)), "opens from the shell");
    let Some(Request::Open {
        tables,
        seed,
        mods,
        viewer,
        ..
    }) = shell.presented().lock().presentation.take_open()
    else {
        panic!("the desk asks for the presentation");
    };
    let tables = petramond::capture::present::open_tables(&tables)
        .recv()
        .unwrap()
        .expect("the tables piece checks");
    let enabled: BTreeSet<String> = mods.into_iter().collect();
    Game::new_presentation(
        Camera::new(
            WorldPos::new(captured.eye[0], captured.eye[1], captured.eye[2]),
            16.0 / 9.0,
        ),
        2,
        seed,
        tables,
        viewer,
        shell.host_presentation(seed, &enabled),
    )
}

fn seek(game: &mut Game, captured: &Captured, ahead: f64) -> u64 {
    let HostRet::Ticket(id) = call(
        game,
        HostCall::from(mod_api::calls::ClientPresentationApply {
            state: vec![ClientFileRanges {
                scope: ClientStorageScope::Pack,
                path: format!("{}/world.pmc", captured.dir),
                ranges: vec![captured.state],
            }],
            events: vec![ClientFileRanges {
                scope: ClientStorageScope::Pack,
                path: format!("{}/events.pmc", captured.dir),
                ranges: vec![captured.events],
            }],
            at: captured.tick as f64 + ahead,
        }),
    ) else {
        panic!("the owner applies");
    };
    let input = GameInput::default();
    let deadline = Instant::now() + petramond_util::test_time::TEST_HARD_DEADLINE;
    loop {
        game.tick(DT, &input);
        let HostRet::ClientPresentationState(state) = call(
            game,
            HostCall::from(mod_api::calls::ClientPresentationState),
        ) else {
            panic!("the state answers");
        };
        assert_eq!(state.error, None);
        if state.applied == id {
            return id;
        }
        assert!(
            Instant::now() < deadline,
            "the apply never landed: {state:?}"
        );
    }
}

#[test]
fn a_presentation_opened_on_the_shell_presents_mod_files_and_never_touches_a_save() {
    std::env::set_var(
        "PETRAMOND_DATA_DIR",
        petramond_util::test_dirs::test_process_data_dir(),
    );
    let scratch = petramond_util::test_dirs::TestScratchDir::new("client-presentation");
    let world = format!("presented-{}", std::process::id());
    let input = GameInput::default();

    let captured = {
        let mut live = Live::new(&world);
        for _ in 0..20 {
            live.frame();
        }
        capture(&mut live, "presented", 60)
    };
    let save = petramond::save::world_dir(&world);
    assert!(save.is_dir(), "the captured world has a save");
    let before = listing(&save);

    let shell = ClientModRuntime::launch_module_for_test(MOD, &inert_guest(&scratch))
        .expect("the guest starts on the shell");
    let mut game = present(shell, &captured);
    assert!(game.in_presentation());
    let events = game.tick(DT, &input);
    assert!(events.presented_world_replaced);
    seek(&mut game, &captured, 2.25);
    assert!(
        !game.replica.world.data().sections.is_empty(),
        "the capture restores its world"
    );
    let HostRet::ClientPresentationState(landed) = call(
        &mut game,
        HostCall::from(mod_api::calls::ClientPresentationState),
    ) else {
        panic!()
    };
    let through = landed.released_through.expect("a batch is presented");
    assert_eq!(
        call(
            &mut game,
            HostCall::from(mod_api::calls::ClientPresentationTime {
                at: captured.tick as f64 + 20.25
            })
        ),
        HostRet::Bool(true)
    );
    for _ in 0..3 {
        game.tick(DT, &input);
    }
    let HostRet::ClientPresentationState(played) = call(
        &mut game,
        HostCall::from(mod_api::calls::ClientPresentationState),
    ) else {
        panic!()
    };
    assert!(
        played.released_through > Some(through),
        "time plays the queued frames on: {played:?}"
    );
    game.save_all();

    call(
        &mut game,
        HostCall::from(mod_api::calls::ClientPresentationClose),
    );
    game.tick(DT, &input);
    assert!(game.take_presentation_closed(), "closing ends the client");
    let shell = game.into_shell().expect("the launched instance survives");
    assert_eq!(shell.launched(), Some(MOD), "and is the shell's again");
    assert_eq!(
        shell.presented().lock().context,
        mod_api::ClientContext::Shell
    );

    let mut game = present(shell, &captured);
    game.tick(DT, &input);
    let shell = game
        .leave_presentation()
        .expect("the launched instance survives");
    let state = shell.presented().lock().presentation.state();
    assert!(
        state.owner.is_none() && !state.open && !state.opening,
        "a presentation left without its owner is over for it too: {state:?}"
    );

    assert_eq!(
        listing(&save),
        before,
        "the captured world's save is untouched"
    );
    assert!(
        !petramond::modding::client::client_storage_dir_for_test(&world, MOD)
            .join("files")
            .exists(),
        "no session storage was made for a session that does not exist"
    );
    let _ = std::fs::remove_dir_all(&save);
    let _ = std::fs::remove_dir_all(
        petramond::modding::client::pack_files_dir_for_test(MOD).join(captured.dir),
    );
}

#[test]
fn the_captured_players_view_and_hud_present_as_they_saw_them() {
    use petramond::modding::client::view::ViewFold;

    std::env::set_var(
        "PETRAMOND_DATA_DIR",
        petramond_util::test_dirs::test_process_data_dir(),
    );
    const LOOK: f32 = 0.777;
    let scratch = petramond_util::test_dirs::TestScratchDir::new("client-presentation-view");
    let world = format!("presented-view-{}", std::process::id());
    let mut live = Live::new(&world);
    for _ in 0..20 {
        live.frame();
    }
    let health = live.game.replica.self_view.health;
    let slots = *live.game.replica.self_view.inventory.raw_slots();
    let captured_player = mod_api::PlayerId(live.game.replica.entities.self_id().0);
    live.look = Some(LOOK);
    let captured = capture(&mut live, "seen", 60);
    assert!(
        (live.game.local.player.yaw - LOOK).abs() > 0.1,
        "the body never looked where the eye was captured looking"
    );
    drop(live);
    let _ = std::fs::remove_dir_all(petramond::save::world_dir(&world));

    let shell = ClientModRuntime::launch_module_for_test(MOD, &inert_guest(&scratch))
        .expect("the guest starts on the shell");
    let mut game = present(shell, &captured);
    seek(&mut game, &captured, 30.25);
    game.present_captured_view();
    game.apply_view_fold(ViewFold {
        subject: Some(captured_player),
        ..Default::default()
    });
    assert!(
        (game.render_camera().yaw - LOOK).abs() < 1e-4,
        "the eye is the captured one: {}",
        game.render_camera().yaw
    );
    assert!(game.subject_hands().is_some());
    let hud = game.hud_view().expect("the captured player's HUD");
    assert_eq!(hud.health, health);
    assert_eq!(*hud.inventory.raw_slots(), slots);
    let _ = std::fs::remove_dir_all(
        petramond::modding::client::pack_files_dir_for_test(MOD).join(captured.dir),
    );
}
