use super::{app, app_with_grass, cursor_over_slot};
use crate::app::{App, CursorIcon, CursorPolicy};
#[cfg(feature = "audio")] // only the engine-gated ui-click test reads it
use crate::audio::Sound;
use crate::camera::Camera;
use crate::controls::{Control, Modifiers, PointerButton, TextKey, TextShortcut};
use crate::mathh::Vec3;
use crate::net::protocol::{ClientToServer, ServerToClient};
use crate::player::PlayerMode;
use crate::save::WorldInfo;

#[test]
fn app_starts_on_title_without_loading_a_game() {
    let app = App::new(Camera::new(Vec3::new(0.0, 80.0, 0.0), 16.0 / 9.0), 1);

    assert_eq!(app.screen, crate::app::AppScreen::Title);
    assert!(app.game.is_none(), "title screen does not preload a world");
    assert_eq!(
        app.cursor_policy(),
        CursorPolicy {
            grabbed: false,
            visible: true,
            icon: CursorIcon::Default,
        }
    );
}

/// World Settings is gated on a selection (the document binds `has_selection`),
/// and it hosts the delete-world flow: settings → Delete World → confirmation,
/// whose Cancel returns to world select with the selection intact.
#[test]
fn world_settings_requires_selection_and_hosts_the_delete_flow() {
    use crate::gui::GuiKind;
    let mut app = App::new(Camera::new(Vec3::new(0.0, 80.0, 0.0), 16.0 / 9.0), 1);
    app.screen = crate::app::AppScreen::WorldSelect;
    app.worlds = test_worlds(1);
    let screen = (1280, 720);

    // No selection: the (disabled) Settings button routes no click.
    app.drive_doc_ui(GuiKind::WorldSelect, screen, 0.0);
    click_doc_id(&mut app, "settings");
    app.drive_doc_ui(GuiKind::WorldSelect, screen, 0.1);
    assert_eq!(app.screen, crate::app::AppScreen::WorldSelect);

    // With a selection it opens World Settings for that world.
    app.selected_world = Some(0);
    app.drive_doc_ui(GuiKind::WorldSelect, screen, 0.2);
    click_doc_id(&mut app, "settings");
    app.drive_doc_ui(GuiKind::WorldSelect, screen, 0.3);
    assert_eq!(app.screen, crate::app::AppScreen::WorldSettings);
    assert_eq!(
        app.world_settings.as_ref().map(|s| s.world_name.as_str()),
        Some("world-0")
    );

    // Its Delete World button opens the confirmation…
    app.drive_doc_ui(GuiKind::WorldSettings, screen, 0.4);
    click_doc_id(&mut app, "delete_world");
    app.drive_doc_ui(GuiKind::WorldSettings, screen, 0.5);
    assert_eq!(app.screen, crate::app::AppScreen::DeleteWorld);

    // …and Cancel returns to world select without losing the selection.
    app.drive_doc_ui(GuiKind::DeleteWorld, screen, 0.6);
    click_doc_id(&mut app, "cancel");
    app.drive_doc_ui(GuiKind::DeleteWorld, screen, 0.7);
    assert_eq!(app.screen, crate::app::AppScreen::WorldSelect);
    assert_eq!(app.selected_world, Some(0));
}

/// End-to-end plumbing for the document-backed create-world screen: platform
/// text entry points route into the petramond-ui runtime, the focused editor
/// applies them, and the controller mirrors the text into bound state.
/// (Editor semantics themselves are tested in petramond-ui's text_edit suite.)
#[test]
fn create_world_document_input_types_selects_and_uses_clipboard() {
    use crate::gui::GuiKind;
    let mut app = App::new(Camera::new(Vec3::new(0.0, 80.0, 0.0), 16.0 / 9.0), 1);
    app.screen = crate::app::AppScreen::CreateWorld;
    let screen = (1280, 720);
    let shared = std::rc::Rc::new(std::cell::RefCell::new(None::<String>));
    app.ui
        .set_clipboard(Box::new(SharedClipboard(shared.clone())));
    let drive = |app: &mut App, now: f64| app.drive_doc_ui(GuiKind::CreateWorld, screen, now);

    // Solve one frame so the name input has a rect, then click to focus it.
    drive(&mut app, 0.0);
    let rect = app.ui.out().rect("create_name").expect("name input rect");
    app.set_cursor_position((rect.x + rect.w / 2) as f32, (rect.y + rect.h / 2) as f32);
    app.set_pointer_button(PointerButton::Primary, true);
    app.set_pointer_button(PointerButton::Primary, false);
    assert!(app.handle_text_input("abcdef"));
    drive(&mut app, 0.1);
    assert_eq!(
        app.ui.state_mut().get_str("create_name"),
        Some("abcdef"),
        "typed text mirrors into bound state"
    );

    // Shift+arrow selection replaced by typing, same as the legacy editor.
    app.handle_text_key(TextKey::ArrowLeft);
    app.handle_text_key(TextKey::ArrowLeft);
    app.set_modifiers(Modifiers {
        ctrl: false,
        shift: true,
        ..Modifiers::default()
    });
    app.handle_text_key(TextKey::ArrowLeft);
    app.handle_text_key(TextKey::ArrowLeft);
    app.set_modifiers(Modifiers::default());
    assert!(app.handle_text_input("XY"));
    drive(&mut app, 0.2);
    assert_eq!(app.ui.state_mut().get_str("create_name"), Some("abXYef"));

    // Clipboard shortcuts through the injected shared clipboard (AppUi owns
    // the clipboard; the platform threads none through).
    app.set_modifiers(Modifiers {
        ctrl: true,
        shift: false,
        ..Modifiers::default()
    });
    assert!(app.handle_text_shortcut_code(winit::keyboard::KeyCode::KeyA));
    assert!(app.handle_text_shortcut_code(winit::keyboard::KeyCode::KeyC));
    drive(&mut app, 0.3);
    assert_eq!(shared.borrow().as_deref(), Some("abXYef"));

    assert!(app.handle_text_shortcut_code(winit::keyboard::KeyCode::KeyX));
    drive(&mut app, 0.4);
    assert_eq!(app.ui.state_mut().get_str("create_name"), Some(""));

    *shared.borrow_mut() = Some("Pasted $#@!^{}".to_string());
    assert!(app.handle_text_shortcut_code(winit::keyboard::KeyCode::KeyV));
    drive(&mut app, 0.5);
    assert_eq!(
        app.ui.state_mut().get_str("create_name"),
        Some("Pasted $#@!^{}")
    );
}

/// The document shell flow end-to-end through real pointer plumbing:
/// title → (click Start Game) → world select → keyboard select → settings →
/// back — every transition driven by the same App entry points the platform
/// calls, resolved by the document runtime's hit-testing.
#[test]
fn document_shell_screens_flow_via_pointer_and_keys() {
    use crate::gui::GuiKind;
    let mut app = App::new(Camera::new(Vec3::new(0.0, 80.0, 0.0), 16.0 / 9.0), 1);
    let screen = (1280, 720);
    let click_id = click_doc_id;

    assert_eq!(app.doc_ui_kind(), Some(GuiKind::Title));
    app.drive_doc_ui(GuiKind::Title, screen, 0.0);
    click_id(&mut app, "start");
    app.drive_doc_ui(GuiKind::Title, screen, 0.1);
    assert_eq!(app.screen, crate::app::AppScreen::WorldSelect);

    // World select with two stub worlds: keyboard selection enables Play.
    app.worlds = test_worlds(2);
    app.drive_doc_ui(GuiKind::WorldSelect, screen, 0.2);
    app.handle_text_key(TextKey::ArrowDown);
    app.drive_doc_ui(GuiKind::WorldSelect, screen, 0.3);
    assert_eq!(app.selected_world, Some(0));
    app.handle_text_key(TextKey::ArrowDown);
    app.drive_doc_ui(GuiKind::WorldSelect, screen, 0.4);
    assert_eq!(app.selected_world, Some(1));

    // Create → cancel round-trips; Back returns to the title.
    click_id(&mut app, "create");
    app.drive_doc_ui(GuiKind::WorldSelect, screen, 0.5);
    assert_eq!(app.screen, crate::app::AppScreen::CreateWorld);
    app.drive_doc_ui(GuiKind::CreateWorld, screen, 0.6);
    click_id(&mut app, "cancel");
    app.drive_doc_ui(GuiKind::CreateWorld, screen, 0.7);
    assert_eq!(app.screen, crate::app::AppScreen::WorldSelect);
    app.drive_doc_ui(GuiKind::WorldSelect, screen, 0.8);
    click_id(&mut app, "back");
    app.drive_doc_ui(GuiKind::WorldSelect, screen, 0.9);
    assert_eq!(app.screen, crate::app::AppScreen::Title);
}

// Asserts the playback engine's play record; the featureless (headless-server)
// build's silent stub records nothing by design.
#[cfg(feature = "audio")]
#[test]
fn shell_button_and_toggle_activations_play_ui_click_sound() {
    use crate::gui::GuiKind;
    let mut app = App::new(Camera::new(Vec3::new(0.0, 80.0, 0.0), 16.0 / 9.0), 1);
    let screen = (1280, 720);

    app.drive_doc_ui(GuiKind::Title, screen, 0.0);
    app.audio.take_played_for_test();
    click_doc_id(&mut app, "start");
    app.drive_doc_ui(GuiKind::Title, screen, 0.1);
    assert_eq!(app.audio.take_played_for_test(), vec![Sound::UiClick]);

    app.drive_doc_ui(GuiKind::Demo, screen, 0.2);
    app.audio.take_played_for_test();
    click_doc_id(&mut app, "t1");
    app.drive_doc_ui(GuiKind::Demo, screen, 0.3);
    assert_eq!(app.audio.take_played_for_test(), vec![Sound::UiClick]);
}

#[test]
fn document_game_menu_clicks_do_not_play_shell_ui_click_sound() {
    let mut app = app_with_grass();
    app.handle_control(Control::ToggleInventory, true);
    let screen = (1280, 720);
    let (cx, cy) = cursor_over_slot(&mut app, screen, 0);

    app.audio.take_played_for_test();
    app.set_cursor_position(cx, cy);
    assert!(app.click_screen_for_test(screen, 0.0));
    assert!(app.audio.take_played_for_test().is_empty());
}

/// Renaming a world changes ONLY its display name; playing it must open the
/// original save directory. (Regression: play once keyed on display name,
/// silently starting a fresh world after a rename.)
#[test]
fn play_after_rename_opens_the_original_save_directory() {
    let dir_name = "rename-regress-test";
    let _ = crate::save::delete_world(dir_name);
    crate::save::write_world_metadata(dir_name).expect("create world dir");
    crate::save::rename_world(dir_name, "Renamed Display Name").expect("rename");

    let mut app = App::new(Camera::new(Vec3::new(0.0, 80.0, 0.0), 16.0 / 9.0), 1);
    app.refresh_worlds();
    let idx = app
        .worlds
        .iter()
        .position(|w| w.dir_name == dir_name)
        .expect("renamed world listed");
    assert_eq!(app.worlds[idx].name, "Renamed Display Name");
    app.selected_world = Some(idx);
    app.play_selected_world();
    assert!(app.game.is_some(), "world opened");
    app.save_on_exit();
    drop(app); // joins the save I/O thread; everything queued hits disk

    assert!(
        crate::save::world_dir(dir_name).join("level.dat").exists(),
        "the ORIGINAL directory received the save"
    );
    assert!(
        !crate::save::world_dir("Renamed Display Name").exists(),
        "no fresh world appeared under the display name"
    );
    let _ = crate::save::delete_world(dir_name);
}

/// In-memory clipboard shared with the app's document UI (tests never touch
/// the OS clipboard).
struct SharedClipboard(std::rc::Rc<std::cell::RefCell<Option<String>>>);

impl petramond_ui::TextClipboard for SharedClipboard {
    fn get_text(&mut self) -> Option<String> {
        self.0.borrow().clone()
    }
    fn set_text(&mut self, text: &str) -> bool {
        *self.0.borrow_mut() = Some(text.to_string());
        true
    }
}

#[test]
fn ctrl_y_toggles_player_mode_once_per_chord() {
    let mut app = app();
    assert_eq!(app.game().player_mode(), PlayerMode::Survival);

    app.handle_control(Control::Sprint, true);
    app.handle_control(Control::TogglePlayerMode, true);
    assert_eq!(app.game().player_mode(), PlayerMode::Spectator);

    app.handle_control(Control::TogglePlayerMode, true);
    app.handle_control(Control::Sprint, true);
    assert_eq!(app.game().player_mode(), PlayerMode::Spectator);

    app.handle_control(Control::TogglePlayerMode, false);
    app.handle_control(Control::TogglePlayerMode, true);
    assert_eq!(app.game().player_mode(), PlayerMode::Survival);

    app.handle_control(Control::Sprint, false);
    app.handle_control(Control::TogglePlayerMode, false);
    app.handle_control(Control::TogglePlayerMode, true);
    assert_eq!(app.game().player_mode(), PlayerMode::Survival);
}

#[test]
fn inventory_toggle_is_once_per_press() {
    let mut app = app();
    assert!(!app.screen.inventory_open());

    app.handle_control(Control::ToggleInventory, true);
    assert!(app.screen.inventory_open());
    app.handle_control(Control::ToggleInventory, true);
    assert!(app.screen.inventory_open());

    app.handle_control(Control::ToggleInventory, false);
    app.handle_control(Control::ToggleInventory, true);
    assert!(!app.screen.inventory_open());
}

#[test]
fn opening_inventory_releases_grab() {
    let mut app = app();
    app.pointer.grab_for_gameplay();
    app.handle_control(Control::ToggleInventory, true);
    assert!(app.screen.inventory_open());
    assert!(!app.pointer.is_grabbing());
}

#[test]
fn opening_inventory_clears_held_pointer_buttons() {
    let mut app = app();
    app.set_pointer_button(PointerButton::Primary, true);

    app.handle_control(Control::ToggleInventory, true);
    let game_input = app.take_game_input();

    assert!(!game_input.break_held);
    assert!(!game_input.attack_clicked);
}

#[test]
fn focus_loss_clears_held_pointer_buttons() {
    let mut app = app();
    app.set_pointer_button(PointerButton::Primary, true);

    app.release_pointer_buttons();
    let game_input = app.take_game_input();

    assert!(!game_input.break_held);
    assert!(!game_input.attack_clicked);
}

#[test]
fn escape_closes_open_inventory_and_regrabs() {
    let mut app = app();
    app.handle_control(Control::ToggleInventory, true);
    assert!(app.screen.inventory_open());
    assert!(!app.pointer.is_grabbing());

    assert!(app.handle_control(Control::CloseScreen, true));
    assert!(!app.screen.inventory_open());
    assert!(app.pointer.is_grabbing());
}

#[test]
fn escape_with_inventory_closed_opens_pause() {
    let mut app = app();
    assert!(!app.screen.inventory_open());
    assert!(app.handle_control(Control::CloseScreen, true));
    assert!(!app.screen.inventory_open());
    assert_eq!(app.screen, crate::app::AppScreen::Pause);
    assert!(!app.pointer.is_grabbing());
}

#[test]
fn escape_on_pause_resumes_gameplay_and_regrabs() {
    let mut app = app();
    app.handle_control(Control::CloseScreen, true);
    assert_eq!(app.screen, crate::app::AppScreen::Pause);

    assert!(app.handle_control(Control::CloseScreen, true));

    assert_eq!(app.screen, crate::app::AppScreen::Game);
    assert!(app.pointer.is_grabbing());
}

#[test]
fn save_and_quit_returns_to_title_and_drops_game() {
    let mut app = app();
    app.handle_control(Control::CloseScreen, true);
    assert_eq!(app.screen, crate::app::AppScreen::Pause);

    app.save_and_quit_to_title();

    assert_eq!(app.screen, crate::app::AppScreen::Title);
    assert!(app.game.is_none());
}

#[test]
fn digit_controls_select_hotbar_slot() {
    let mut app = app();
    app.handle_control(Control::SelectHotbar(4), true);
    assert_eq!(app.game().active_hotbar(), 4);
    app.handle_control(Control::SelectHotbar(0), true);
    assert_eq!(app.game().active_hotbar(), 0);
    app.handle_control(Control::SelectHotbar(8), true);
    assert_eq!(app.game().active_hotbar(), 8);
}

#[test]
fn chat_opens_from_t_sends_entered_message_via_server_echo() {
    let mut app = app();
    assert_eq!(app.screen, crate::app::AppScreen::Game);

    app.handle_control(Control::OpenChat, true);
    assert_eq!(app.screen, crate::app::AppScreen::Chat);
    assert_eq!(
        app.cursor_policy(),
        CursorPolicy {
            grabbed: false,
            visible: true,
            icon: CursorIcon::Default,
        }
    );

    assert!(app.handle_text_input("hello"));
    assert!(app.handle_text_key(TextKey::Enter));
    assert_eq!(app.screen, crate::app::AppScreen::Game);

    let msgs = app
        .game
        .as_mut()
        .expect("test app has a game")
        .take_outbox_for_test();
    assert!(msgs
        .iter()
        .any(|msg| matches!(msg, ClientToServer::ChatSend { text } if text == "hello")));

    for msg in msgs {
        app.server.apply_message(0, msg);
    }
    let mut inbox = Vec::new();
    let out = app.server.pump(0.0, &mut inbox);
    let chat = out.msgs.iter().find_map(|msg| match msg {
        ServerToClient::ChatLine(line) => Some(line),
        _ => None,
    });
    assert!(
        chat.is_some_and(|line| line.spans.iter().any(|span| span.text.contains("> hello"))),
        "server echoed the formatted chat line"
    );
}

#[test]
fn slash_opens_chat_with_a_command_prefix() {
    let mut app = app();
    app.handle_control(Control::OpenCommandChat, true);
    assert_eq!(app.screen, crate::app::AppScreen::Chat);

    assert!(app.handle_text_input("time set night"));
    assert!(app.handle_text_key(TextKey::Enter));

    let msgs = app
        .game
        .as_mut()
        .expect("test app has a game")
        .take_outbox_for_test();
    assert!(msgs
        .iter()
        .any(|msg| matches!(msg, ClientToServer::ChatSend { text } if text == "/time set night")));
}

#[test]
fn chat_input_uses_shared_text_editor_selection_and_clipboard() {
    let mut app = app();
    let shared = std::rc::Rc::new(std::cell::RefCell::new(None::<String>));
    app.ui
        .set_clipboard(Box::new(SharedClipboard(shared.clone())));

    app.handle_control(Control::OpenChat, true);
    assert!(app.handle_text_input("abcdef"));
    app.handle_text_key(TextKey::ArrowLeft);
    app.handle_text_key(TextKey::ArrowLeft);
    app.set_modifiers(Modifiers {
        ctrl: false,
        shift: true,
        ..Modifiers::default()
    });
    app.handle_text_key(TextKey::ArrowLeft);
    app.handle_text_key(TextKey::ArrowLeft);
    app.set_modifiers(Modifiers::default());
    assert!(app.handle_text_input("XY"));

    app.handle_text_shortcut(TextShortcut::SelectAll);
    app.handle_text_shortcut(TextShortcut::Copy);
    assert_eq!(shared.borrow().as_deref(), Some("abXYef"));

    app.handle_text_shortcut(TextShortcut::Cut);
    *shared.borrow_mut() = Some("pasted".to_owned());
    app.handle_text_shortcut(TextShortcut::Paste);
    assert!(app.handle_text_key(TextKey::Enter));

    let msgs = app
        .game
        .as_mut()
        .expect("test app has a game")
        .take_outbox_for_test();
    assert!(msgs
        .iter()
        .any(|msg| matches!(msg, ClientToServer::ChatSend { text } if text == "pasted")));
}

#[test]
fn digit_controls_ignored_while_inventory_open() {
    let mut app = app();
    app.handle_control(Control::SelectHotbar(2), true);
    assert_eq!(app.game().active_hotbar(), 2);
    app.handle_control(Control::ToggleInventory, true);
    app.handle_control(Control::SelectHotbar(6), true);
    assert_eq!(app.game().active_hotbar(), 2);
}

fn test_worlds(count: usize) -> Vec<WorldInfo> {
    (0..count)
        .map(|i| WorldInfo {
            name: format!("world-{i}"),
            dir_name: format!("world-{i}"),
            has_level: true,
        })
        .collect()
}

/// Queue a primary click on the document instance `id`, using the last solved
/// frame's rect. The next `drive_doc_ui` frame resolves it.
pub(super) fn click_doc_id(app: &mut App, id: &str) {
    let r = app
        .ui
        .out()
        .rect(id)
        .unwrap_or_else(|| panic!("no rect for '{id}'"));
    app.set_cursor_position((r.x + r.w / 2) as f32, (r.y + r.h / 2) as f32);
    app.set_pointer_button(PointerButton::Primary, true);
    app.set_pointer_button(PointerButton::Primary, false);
}

#[test]
fn options_opens_from_title_and_esc_walks_back_out() {
    use crate::gui::GuiKind;
    let mut app = App::new(Camera::new(Vec3::new(0.0, 80.0, 0.0), 16.0 / 9.0), 1);
    let screen = (1280, 720);

    app.drive_doc_ui(GuiKind::Title, screen, 0.0);
    click_doc_id(&mut app, "options");
    app.drive_doc_ui(GuiKind::Title, screen, 0.1);
    assert_eq!(app.screen, crate::app::AppScreen::Options);

    app.drive_doc_ui(GuiKind::Options, screen, 0.2);
    click_doc_id(&mut app, "graphics");
    app.drive_doc_ui(GuiKind::Options, screen, 0.3);
    assert_eq!(app.screen, crate::app::AppScreen::OptionsGraphics);

    // ESC: category → root → title (the flow began there).
    app.handle_control(Control::CloseScreen, true);
    assert_eq!(app.screen, crate::app::AppScreen::Options);
    app.handle_control(Control::CloseScreen, true);
    assert_eq!(app.screen, crate::app::AppScreen::Title);
}

#[test]
fn options_opened_from_pause_returns_to_pause() {
    use crate::gui::GuiKind;
    let mut app = app();
    let screen = (1280, 720);
    app.handle_control(Control::CloseScreen, true); // pause

    app.drive_doc_ui(GuiKind::Pause, screen, 0.0);
    click_doc_id(&mut app, "options");
    app.drive_doc_ui(GuiKind::Pause, screen, 0.1);
    assert_eq!(app.screen, crate::app::AppScreen::Options);

    app.drive_doc_ui(GuiKind::Options, screen, 0.2);
    click_doc_id(&mut app, "back");
    app.drive_doc_ui(GuiKind::Options, screen, 0.3);
    assert_eq!(
        app.screen,
        crate::app::AppScreen::Pause,
        "Back returns to the pause menu the flow came from"
    );
}

/// The remap loop: click a binding button to arm it, the next raw key becomes
/// the binding, and the rebound key drives the control. ESC cancels an armed
/// remap; clicking a different action's button switches the armed action.
#[test]
fn controls_screen_remaps_a_key_and_esc_or_reclick_cancels() {
    use crate::controls::{BindableAction, Binding, BoundInput};
    use crate::gui::GuiKind;
    use winit::keyboard::KeyCode;

    let mut app = App::new(Camera::new(Vec3::new(0.0, 80.0, 0.0), 16.0 / 9.0), 1);
    let screen = (1280, 720);
    app.screen = crate::app::AppScreen::OptionsControls;

    // Arm Jump for remapping (rows near the top — the scroll viewport clips
    // lower rows out of click reach).
    app.drive_doc_ui(GuiKind::OptionsControls, screen, 0.0);
    click_bind_row(&mut app, "jump");
    app.drive_doc_ui(GuiKind::OptionsControls, screen, 0.1);
    assert_eq!(app.remap.as_deref(), Some("jump"));

    // ESC cancels without touching the binding.
    assert!(app.remap_capture_key(KeyCode::Escape, true));
    assert_eq!(app.remap, None);
    assert_eq!(
        app.settings.bindings.binding(BindableAction::Jump),
        Binding::key(KeyCode::Space)
    );

    // Clicking one action then another switches the armed remap.
    app.drive_doc_ui(GuiKind::OptionsControls, screen, 0.2);
    click_bind_row(&mut app, "jump");
    app.drive_doc_ui(GuiKind::OptionsControls, screen, 0.3);
    click_bind_row(&mut app, "strafe_left");
    app.drive_doc_ui(GuiKind::OptionsControls, screen, 0.4);
    assert_eq!(app.remap.as_deref(), Some("strafe_left"));

    // Capture K (no chord): binding lands, remap disarms.
    assert!(app.remap_capture_key(KeyCode::KeyK, true));
    assert_eq!(app.remap, None);
    assert_eq!(
        app.settings.bindings.binding(BindableAction::StrafeLeft),
        Binding::key(KeyCode::KeyK)
    );

    // The rebound key drives the control through the raw-key path...
    assert!(app.handle_raw_key(KeyCode::KeyK, true));
    assert!(app.take_game_input().movement.left);
    assert!(app.handle_raw_key(KeyCode::KeyK, false));
    assert!(!app.take_game_input().movement.left);
    // ...and the old default no longer does (StrafeLeft moved off A).
    assert!(app.handle_raw_key(KeyCode::KeyA, true));
    assert!(!app.take_game_input().movement.left);
    let _ = app.handle_raw_key(KeyCode::KeyA, false);

    // A tapped modifier binds ITSELF (chord starters bind on release).
    app.remap = Some("sprint".to_string());
    assert!(app.remap_capture_key(KeyCode::AltLeft, true));
    assert_eq!(app.remap.as_deref(), Some("sprint"), "hold = chord start");
    assert!(app.remap_capture_key(KeyCode::AltLeft, false));
    assert_eq!(
        app.settings.bindings.binding(BindableAction::Sprint).input,
        BoundInput::Key(KeyCode::AltLeft)
    );
}

/// Queue a primary click on the binding button of the controls-list row for
/// `action_id`, resolved through the same row list the controller uses (the
/// list interleaves category headers, so indexes are never hardcoded).
fn click_bind_row(app: &mut App, action_id: &str) {
    let index = crate::app::shell_docs::controls_action_row_index(&app.action_table, action_id)
        .unwrap_or_else(|| panic!("no controls row for '{action_id}'"));
    let rect = app
        .ui
        .out()
        .named
        .iter()
        .find(|(key, _)| key.id == "bind" && key.item == Some(index as u32))
        .map(|(_, r)| *r)
        .unwrap_or_else(|| panic!("no solved rect for bind row {index} ('{action_id}')"));
    app.set_cursor_position((rect.x + rect.w / 2) as f32, (rect.y + rect.h / 2) as f32);
    app.set_pointer_button(PointerButton::Primary, true);
    app.set_pointer_button(PointerButton::Primary, false);
}

/// Attack/interact are rebindable: the default mouse buttons land in the
/// pointer break/use state through the raw-mouse path, and a key rebind
/// drives the same state.
#[test]
fn attack_rebinds_from_mouse_to_key() {
    use crate::controls::{BindableAction, Binding};
    use winit::keyboard::KeyCode;

    let mut app = app();
    assert!(app.screen.gameplay_enabled());

    app.handle_raw_mouse(winit::event::MouseButton::Left, true);
    let input = app.take_game_input();
    assert!(input.break_held && input.attack_clicked);
    app.handle_raw_mouse(winit::event::MouseButton::Left, false);
    app.pointer.clear_edges();

    app.settings
        .bindings
        .set(BindableAction::Attack, Binding::key(KeyCode::KeyF));
    assert!(app.handle_raw_key(KeyCode::KeyF, true));
    let input = app.take_game_input();
    assert!(
        input.break_held,
        "the rebound key mines like the button did"
    );
    assert!(app.handle_raw_key(KeyCode::KeyF, false));
    let input = app.take_game_input();
    assert!(!input.break_held);
    // The unbound left button no longer mines...
    app.pointer.clear_edges();
    app.handle_raw_mouse(winit::event::MouseButton::Left, true);
    let input = app.take_game_input();
    assert!(
        !input.break_held,
        "left click moved off Attack; it must not mine"
    );
    app.handle_raw_mouse(winit::event::MouseButton::Left, false);
}

/// Mod-registered key actions join the remappable table under their pack's
/// category (the bundled minimap registers two), and their rows resolve
/// through the same list the engine actions use.
#[test]
fn mod_key_actions_join_the_controls_table_with_their_own_category() {
    let app = app();
    let table = &app.action_table;
    let row = table
        .row("minimap:open_map")
        .expect("minimap's registered action is in the table");
    assert_eq!(row.label, "Open World Map");
    assert_eq!(row.category, "Minimap");
    assert!(table.row("minimap:add_waypoint").is_some());
    assert!(
        crate::app::shell_docs::controls_action_row_index(table, "minimap:open_map").is_some(),
        "the controls list has a row for the mod action"
    );
}

/// Pressing a mod action's bound key dispatches to the owning client mod:
/// the bundled minimap opens its world-map canvas on M (and the canvas
/// screen releases the pointer like any modal).
#[test]
fn mod_bound_key_dispatches_to_the_client_mod() {
    use winit::keyboard::KeyCode;
    let mut app = app();
    // A couple of frames so the client mod publishes its canvas scene.
    app.update_frame((1280, 720));
    assert!(app.handle_raw_key(KeyCode::KeyM, true));
    let _ = app.handle_raw_key(KeyCode::KeyM, false);
    app.update_frame((1280, 720));
    assert!(
        app.screen.client_canvas_open(),
        "M reached the minimap mod and opened the world map, got {:?}",
        app.screen
    );
}

/// Wheel travel over an open client canvas routes to the owning mod
/// (coalesced per frame) instead of the gameplay scroll bindings: the bundled
/// minimap zooms around the cursor, which re-anchors its retained scene view.
/// With the cursor off the canvas the travel is dropped.
#[test]
fn canvas_wheel_scroll_reaches_the_client_mod() {
    use winit::keyboard::KeyCode;
    let mut app = app();
    app.update_frame((1280, 720));
    assert!(app.handle_raw_key(KeyCode::KeyM, true));
    let _ = app.handle_raw_key(KeyCode::KeyM, false);
    app.update_frame((1280, 720));
    assert!(app.screen.client_canvas_open());
    let view = |app: &crate::app::App| {
        app.game
            .as_ref()
            .unwrap()
            .client_mod_canvas_view("minimap:full_map")
            .expect("the world map publishes a retained scene")
            .offset
    };
    let before = view(&app);
    // The canvas display rect is computed in the render path; compose once
    // like a drawn frame would so the scroll's inside-rect check can pass.
    app.compose_client_overlays((1280, 720));
    // Off-center cursor inside the canvas: the cursor-anchored zoom must
    // shift the pan, which shows up in the published view offset.
    app.set_cursor_position(420.0, 130.0);
    app.add_scroll_delta(-1.0); // one wheel notch up (app-internal + = down)
    app.update_frame((1280, 720));
    let zoomed = view(&app);
    assert_ne!(before, zoomed, "the wheel notch reached the minimap");
    // Cursor outside the canvas rect: wheel travel is dropped.
    app.set_cursor_position(10.0, 360.0);
    app.add_scroll_delta(-1.0);
    app.update_frame((1280, 720));
    assert_eq!(zoomed, view(&app), "off-canvas wheel travel is dropped");
    // Ride the wheel down to the outermost level (1 px per 2×2 blocks, the
    // HSL-averaging raster) and let the progressive loads and budgeted
    // rasters run: a guest fault would disable the mod and stop publishing.
    app.set_cursor_position(640.0, 360.0);
    for _ in 0..3 {
        app.add_scroll_delta(1.0);
        app.update_frame((1280, 720));
    }
    for _ in 0..12 {
        app.update_frame((1280, 720));
    }
    let _ = view(&app);
}

/// A click whose press lands on a MENU and whose release lands in GAMEPLAY
/// (the screen flips in between — double-clicking a world to join it, or
/// clicking RESUME) must not leave the attack/mine state held: the release
/// routes through the binding engine, which never saw the press, so the
/// gameplay transition itself has to shed menu-held buttons.
#[test]
fn menu_click_that_enters_gameplay_leaves_no_mining_held() {
    let mut app = app();
    app.handle_control(Control::CloseScreen, true); // pause menu
    // Physical press over the menu: recorded in the pointer state, routed to
    // the UI (this is the double-click's second press).
    app.handle_raw_mouse(winit::event::MouseButton::Left, true);
    // The controller flips to gameplay between press and release.
    app.resume_game();
    // The release lands in gameplay and resolves through the binding engine.
    app.handle_raw_mouse(winit::event::MouseButton::Left, false);
    let input = app.take_game_input();
    assert!(
        !input.break_held && !input.attack_clicked,
        "no stale mining from the menu press"
    );
}
