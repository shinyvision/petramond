//! Multiplayer UI flows: the Connect to Server screen, the
//! ModsMissing refusal, the Disconnected screen, and the pause menu's
//! host/remote split — all driven through the real documents (rects resolved
//! from the solved frame, never pinned pixels).

use super::app;
use super::controls::click_doc_id;
use crate::app::connect::ConnectPhase;
use crate::app::{App, AppScreen};
use crate::camera::Camera;
use crate::controls::{Control, TextKey, TextShortcut};
use crate::game::{Game, GameEvents};
use crate::gui::GuiKind;
use crate::mathh::Vec3;
use crate::net::protocol::{JoinData, ModEntry, SelfRestore};
use crate::server::handle::ServerHandle;
use crate::server::player::PlayerId;
use petramond_ui::UiValue;

const SCREEN: (u32, u32) = (1280, 720);

fn shell_app() -> App {
    App::new(Camera::new(Vec3::new(0.0, 80.0, 0.0), 16.0 / 9.0), 1)
}

/// A minimal remote join, the shape `client_handshake` returns (health > 0 so
/// adopting the game lands on the gameplay screen, not the death screen).
fn join_data() -> Box<JoinData> {
    Box::new(JoinData {
        player_id: PlayerId(2),
        seed: 11,
        clock: 6000,
        tables: crate::net::remap::local_name_tables(),
        self_restore: SelfRestore {
            transform: crate::net::protocol::Transform {
                pos: Vec3::new(2.5, 80.0, 2.5),
                vel: Vec3::ZERO,
                yaw: 0.0,
                pitch: 0.0,
            },
            mode: 0,
            health: 20,
            bed_spawn: None,
            effects: Vec::new(),
            inventory: vec![None; 37],
            active_slot: 0,
            craft_craftable_only: false,
        },
        crafting_recipes: Vec::new(),
        players: vec![(PlayerId(0), "Host".to_string())],
    })
}

/// Title → Connect to Server; typed edits mirror into bound state; the
/// Connect button gates on both fields being non-empty.
#[test]
fn connect_screen_opens_from_title_mirrors_edits_and_gates_connect() {
    let mut app = shell_app();

    app.drive_doc_ui(GuiKind::Title, SCREEN, 0.0);
    click_doc_id(&mut app, "connect");
    app.drive_doc_ui(GuiKind::Title, SCREEN, 0.1);
    assert_eq!(app.screen, AppScreen::ConnectServer);

    // The player name prefills from the resolved identity (never empty).
    app.drive_doc_ui(GuiKind::ConnectServer, SCREEN, 0.2);
    let name = app
        .ui
        .state_mut()
        .get_str("player_name")
        .unwrap_or("")
        .to_owned();
    assert!(!name.is_empty(), "player name prefilled");

    // The address input opens focused: select-all + type replaces whatever
    // the prefill was, and the controller mirrors it into bound state.
    app.handle_text_shortcut(TextShortcut::SelectAll);
    assert!(app.handle_text_input("192.168.0.5:7434"));
    app.drive_doc_ui(GuiKind::ConnectServer, SCREEN, 0.3);
    assert_eq!(
        app.ui.state_mut().get_str("server_addr"),
        Some("192.168.0.5:7434"),
        "typed address mirrors into bound state"
    );
    app.drive_doc_ui(GuiKind::ConnectServer, SCREEN, 0.4);
    assert_eq!(app.ui.state_mut().get_bool("can_connect"), Some(true));

    // Emptying the address disables Connect again.
    app.handle_text_shortcut(TextShortcut::SelectAll);
    app.handle_text_key(TextKey::Delete);
    app.drive_doc_ui(GuiKind::ConnectServer, SCREEN, 0.5);
    app.drive_doc_ui(GuiKind::ConnectServer, SCREEN, 0.6);
    assert_eq!(app.ui.state_mut().get_str("server_addr"), Some(""));
    assert_eq!(app.ui.state_mut().get_bool("can_connect"), Some(false));
}

/// A refused join's mod list fills the ModsMissing rows; Back returns to the
/// connect screen with the attempted address preserved.
#[test]
fn refused_mod_list_populates_missing_rows_and_back_preserves_address() {
    let mut app = shell_app();
    app.connect.addr = "192.168.1.9:7434".to_owned();
    app.connect.name = "Rachel".to_owned();
    app.connect.missing = vec![
        ModEntry {
            id: "kitchen".to_owned(),
            version: "1.0".to_owned(),
        },
        ModEntry {
            id: "wheel".to_owned(),
            version: String::new(),
        },
    ];
    app.screen = AppScreen::ModsMissing;

    app.drive_doc_ui(GuiKind::ModsMissing, SCREEN, 0.0);
    let rows = app
        .ui
        .state_mut()
        .get_list("missing_rows")
        .cloned()
        .expect("missing rows bound");
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].get("id"), Some(&UiValue::Str("kitchen".to_owned())));
    assert_eq!(rows[0].get("has_version"), Some(&UiValue::Bool(true)));
    assert_eq!(rows[1].get("id"), Some(&UiValue::Str("wheel".to_owned())));
    assert_eq!(rows[1].get("has_version"), Some(&UiValue::Bool(false)));

    click_doc_id(&mut app, "back");
    app.drive_doc_ui(GuiKind::ModsMissing, SCREEN, 0.1);
    assert_eq!(app.screen, AppScreen::ConnectServer);
    app.drive_doc_ui(GuiKind::ConnectServer, SCREEN, 0.2);
    assert_eq!(
        app.ui.state_mut().get_str("server_addr"),
        Some("192.168.1.9:7434"),
        "the attempted address survives the round-trip"
    );
}

/// An unparseable address fails inline — phase Failed, danger label bound —
/// without ever spawning a worker thread.
#[test]
fn a_bad_address_fails_inline_without_spawning_a_worker() {
    let mut app = shell_app();
    app.open_connect_server();
    let state = app.ui.state_mut();
    state.set("server_addr", UiValue::Str("host:notaport".to_owned()));
    state.set("player_name", UiValue::Str("Rachel".to_owned()));

    app.begin_connect();

    assert!(matches!(app.connect.phase, ConnectPhase::Failed { .. }));
    assert!(!app.connect.has_worker(), "no thread for a parse failure");
    app.drive_doc_ui(GuiKind::ConnectServer, SCREEN, 0.0);
    assert_eq!(app.ui.state_mut().get_bool("has_status"), Some(true));
    assert_eq!(app.ui.state_mut().get_bool("connecting"), Some(false));
}

/// A `connection_lost` event tears the session down (no game left) onto the
/// Disconnected screen showing the reason; OK returns to the title.
#[test]
fn connection_lost_event_tears_down_to_the_disconnected_screen() {
    let mut app = app();
    let events = GameEvents {
        connection_lost: Some("The server closed the connection".to_owned()),
        ..Default::default()
    };

    app.handle_open_screen_events(&events);

    assert_eq!(app.screen, AppScreen::ConnectionLost);
    assert!(app.game.is_none(), "the dead session is dropped, unsaved");
    assert_eq!(app.disconnect_message, "The server closed the connection");

    app.drive_doc_ui(GuiKind::ConnectionLost, SCREEN, 0.0);
    assert_eq!(
        app.ui.state_mut().get_str("disconnect_message"),
        Some("The server closed the connection")
    );
    click_doc_id(&mut app, "ok");
    app.drive_doc_ui(GuiKind::ConnectionLost, SCREEN, 0.1);
    assert_eq!(app.screen, AppScreen::Title);
}

/// The host's pause menu offers Open to LAN until a port is bound, then the
/// open-port label; Disconnect never shows and Save and Quit stays.
#[test]
fn pause_menu_shows_lan_controls_for_host() {
    let mut app = app();
    app.handle_control(Control::CloseScreen, true);
    assert_eq!(app.screen, AppScreen::Pause);

    app.drive_doc_ui(GuiKind::Pause, SCREEN, 0.0);
    assert_eq!(app.ui.state_mut().get_bool("is_host"), Some(true));
    assert_eq!(app.ui.state_mut().get_bool("lan_closed"), Some(true));
    assert_eq!(app.ui.state_mut().get_bool("lan_open"), Some(false));
    assert!(app.ui.out().rect("open_lan").is_some());
    assert!(app.ui.out().rect("save_quit").is_some());
    assert!(app.ui.out().rect("disconnect").is_none());

    // A bound port flips the button into the status label.
    app.lan_port = Some(7434);
    app.drive_doc_ui(GuiKind::Pause, SCREEN, 0.1);
    assert_eq!(app.ui.state_mut().get_bool("lan_open"), Some(true));
    assert_eq!(app.ui.state_mut().get_bool("lan_closed"), Some(false));
    assert!(app.ui.out().rect("open_lan").is_none());
}

/// A remote session's pause menu shows Disconnect (which leaves to the
/// title) and hides both Save and Quit and Open to LAN.
#[test]
fn pause_menu_shows_disconnect_for_remote_and_hides_save_quit() {
    let (handle, _pipe) = ServerHandle::loopback();
    let cam = Camera::new(Vec3::new(0.0, 80.0, 0.0), 16.0 / 9.0);
    let game = Game::new_remote(
        cam,
        join_data(),
        handle,
        1,
        "test-server",
        &std::collections::BTreeSet::new(),
        None,
    );
    assert!(game.is_remote());

    let mut app = shell_app();
    app.adopt_game(game);
    app.handle_control(Control::CloseScreen, true);
    assert_eq!(app.screen, AppScreen::Pause);

    app.drive_doc_ui(GuiKind::Pause, SCREEN, 0.0);
    assert_eq!(app.ui.state_mut().get_bool("is_remote"), Some(true));
    assert_eq!(app.ui.state_mut().get_bool("is_host"), Some(false));
    assert!(app.ui.out().rect("disconnect").is_some());
    assert!(app.ui.out().rect("save_quit").is_none());
    assert!(app.ui.out().rect("open_lan").is_none());

    click_doc_id(&mut app, "disconnect");
    app.drive_doc_ui(GuiKind::Pause, SCREEN, 0.1);
    assert_eq!(app.screen, AppScreen::Title);
    assert!(app.game.is_none());
}

/// Single-player pause freezes the client (no frames reach the sim), but a
/// LAN host's pause menu must NOT: the server ignores `Pause` once opened
/// (`lan_ever_opened`), so a frozen client would be an unpushable,
/// undamageable statue in a world that keeps running. Behind the multiplayer
/// pause menu the client keeps running full frames — observable as its
/// per-frame `PlayerUpdate` still reaching the server channel — while
/// gameplay input stays disabled and the menu stays up.
#[test]
fn multiplayer_pause_menu_does_not_freeze_the_client() {
    use crate::net::protocol::ClientToServer;

    let mut app = super::app();
    app.handle_control(Control::CloseScreen, true); // ESC
    assert_eq!(app.screen, AppScreen::Pause);
    let drain = |app: &mut super::TestApp| {
        let mut updates = 0;
        while let Ok(msg) = app.pipe.inbox.try_recv() {
            if matches!(msg, ClientToServer::PlayerUpdate(_)) {
                updates += 1;
            }
        }
        updates
    };
    drain(&mut app); // the ESC's Pause(true) and any pre-pause frames

    // Single-player: the pause menu freezes the client — no frames run.
    app.update_frame(SCREEN);
    assert_eq!(drain(&mut app), 0, "SP pause: the client sends nothing");

    // The same session once LAN is open (the flag the Open-to-LAN click
    // sets): the pause menu no longer freezes anything.
    app.lan_port = Some(7434);
    app.update_frame(SCREEN);
    assert!(
        drain(&mut app) > 0,
        "LAN-host pause: the client keeps simulating and reporting itself"
    );
    assert_eq!(app.screen, AppScreen::Pause, "the menu itself stays up");
}

/// The real thing over loopback TCP: a spawned host opened to LAN on an
/// ephemeral port, the UI connect worker joining it, and the remote-session
/// disconnect leaving cleanly.
#[test]
fn end_to_end_connect_through_the_ui_joins_a_lan_server() {
    let (server, _bootstrap) = crate::game::session::build_session_inline("", 7, 1);
    let mut host = ServerHandle::spawn(server);
    host.unthrottle_for_test();
    let port = host.open_to_lan(0).expect("bind an ephemeral LAN port");

    let mut app = shell_app();
    app.open_connect_server();
    let state = app.ui.state_mut();
    state.set("server_addr", UiValue::Str(format!("127.0.0.1:{port}")));
    state.set("player_name", UiValue::Str("E2EVisitor".to_owned()));
    app.begin_connect();
    assert!(app.connect.connecting(), "the worker attempt is running");

    let deadline = std::time::Instant::now() + crate::test_time::TEST_HARD_DEADLINE;
    loop {
        app.poll_connect_worker();
        if app.screen == AppScreen::Game {
            break;
        }
        if let ConnectPhase::Failed { message } = &app.connect.phase {
            panic!("connect failed: {message}");
        }
        assert!(
            std::time::Instant::now() < deadline,
            "connect did not complete in time"
        );
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    assert!(
        app.game.as_ref().is_some_and(|g| g.is_remote()),
        "the adopted session is the remote client"
    );

    app.disconnect_to_title();
    assert_eq!(app.screen, AppScreen::Title);
    assert!(app.game.is_none());
    host.shutdown_and_join();
}
