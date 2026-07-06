use super::{app, app_with_grass, cursor_over_slot};
use crate::app::{App, CursorIcon, CursorPolicy};
use crate::audio::Sound;
use crate::camera::Camera;
use crate::controls::{Control, Modifiers, PointerButton, TextKey};
use crate::mathh::Vec3;
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
/// text entry points route into the llama-ui runtime, the focused editor
/// applies them, and the controller mirrors the text into bound state.
/// (Editor semantics themselves are tested in llama-ui's text_edit suite.)
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

impl llama_ui::TextClipboard for SharedClipboard {
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
    assert_eq!(app.game().inventory().active_slot(), 4);
    app.handle_control(Control::SelectHotbar(0), true);
    assert_eq!(app.game().inventory().active_slot(), 0);
    app.handle_control(Control::SelectHotbar(8), true);
    assert_eq!(app.game().inventory().active_slot(), 8);
}

#[test]
fn digit_controls_ignored_while_inventory_open() {
    let mut app = app();
    app.handle_control(Control::SelectHotbar(2), true);
    assert_eq!(app.game().inventory().active_slot(), 2);
    app.handle_control(Control::ToggleInventory, true);
    app.handle_control(Control::SelectHotbar(6), true);
    assert_eq!(app.game().inventory().active_slot(), 2);
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
fn click_doc_id(app: &mut App, id: &str) {
    let r = app
        .ui
        .out()
        .rect(id)
        .unwrap_or_else(|| panic!("no rect for '{id}'"));
    app.set_cursor_position((r.x + r.w / 2) as f32, (r.y + r.h / 2) as f32);
    app.set_pointer_button(PointerButton::Primary, true);
    app.set_pointer_button(PointerButton::Primary, false);
}
