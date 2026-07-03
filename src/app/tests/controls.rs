use super::app;
use crate::app::TextClipboard;
use crate::app::{App, CursorIcon, CursorPolicy};
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
        app.cursor_policy((1280, 720)),
        CursorPolicy {
            grabbed: false,
            visible: true,
            icon: CursorIcon::Default,
        }
    );
}

#[test]
fn world_select_scrollbar_appears_only_when_worlds_overflow() {
    let mut app = App::new(Camera::new(Vec3::new(0.0, 80.0, 0.0), 16.0 / 9.0), 1);
    app.screen = crate::app::AppScreen::WorldSelect;
    app.worlds = test_worlds(5);

    let screen = (1280, 720);
    let snapshot = app.shell_ui_snapshot(screen, (0.0, 0.0));
    assert!(snapshot.scrollbars.is_empty());

    app.worlds = test_worlds(6);
    let snapshot = app.shell_ui_snapshot(screen, (0.0, 0.0));
    assert_eq!(snapshot.scrollbars.len(), 1);
    assert_eq!(snapshot.rows.len(), 5);
}

#[test]
fn world_select_scrollbar_track_click_scrolls_to_valid_end() {
    let mut app = App::new(Camera::new(Vec3::new(0.0, 80.0, 0.0), 16.0 / 9.0), 1);
    app.screen = crate::app::AppScreen::WorldSelect;
    app.worlds = test_worlds(12);

    let screen = (1280, 720);
    let snapshot = app.shell_ui_snapshot(screen, (0.0, 0.0));
    let scrollbar = snapshot
        .scrollbars
        .first()
        .expect("overflowing world list should expose a scrollbar");
    app.set_cursor_position(
        scrollbar.track.x + scrollbar.track.w * 0.5,
        scrollbar.track.y + scrollbar.track.h - 0.5,
    );
    assert!(app.route_shell_click(screen, 0.0));

    let visible = snapshot.rows.len();
    assert_eq!(app.world_scroll, app.worlds.len().saturating_sub(visible));
    let snapshot = app.shell_ui_snapshot(screen, (0.0, 0.0));
    assert_eq!(
        snapshot.rows.first().map(|r| r.label.as_str()),
        Some("world-7")
    );
}

#[test]
fn world_select_delete_requires_a_selected_world() {
    let mut app = App::new(Camera::new(Vec3::new(0.0, 80.0, 0.0), 16.0 / 9.0), 1);
    app.screen = crate::app::AppScreen::WorldSelect;
    app.worlds = test_worlds(1);

    let screen = (1280, 720);
    let snapshot = app.shell_ui_snapshot(screen, (0.0, 0.0));
    let delete = shell_button(&snapshot, "Delete World");
    assert!(!delete.enabled);

    app.selected_world = Some(0);
    let snapshot = app.shell_ui_snapshot(screen, (0.0, 0.0));
    let delete = shell_button(&snapshot, "Delete World");
    assert!(delete.enabled);
}

#[test]
fn world_select_delete_opens_confirmation_window() {
    let mut app = App::new(Camera::new(Vec3::new(0.0, 80.0, 0.0), 16.0 / 9.0), 1);
    app.screen = crate::app::AppScreen::WorldSelect;
    app.worlds = test_worlds(1);
    app.selected_world = Some(0);

    let screen = (1280, 720);
    click_shell_button(&mut app, screen, "Delete World");

    assert_eq!(app.screen, crate::app::AppScreen::DeleteWorld);
    let snapshot = app.shell_ui_snapshot(screen, (0.0, 0.0));
    assert!(snapshot
        .buttons
        .iter()
        .any(|button| button.label == "Cancel"));
    assert!(snapshot.texts.iter().any(|text| text.text == "world-0"));
}

#[test]
fn delete_world_confirmation_cancel_returns_to_world_select() {
    let mut app = App::new(Camera::new(Vec3::new(0.0, 80.0, 0.0), 16.0 / 9.0), 1);
    app.screen = crate::app::AppScreen::DeleteWorld;
    app.worlds = test_worlds(1);
    app.selected_world = Some(0);

    let screen = (1280, 720);
    click_shell_button(&mut app, screen, "Cancel");

    assert_eq!(app.screen, crate::app::AppScreen::WorldSelect);
    assert_eq!(app.selected_world, Some(0));
}

#[test]
fn create_world_text_input_moves_selects_and_replaces() {
    let mut app = App::new(Camera::new(Vec3::new(0.0, 80.0, 0.0), 16.0 / 9.0), 1);
    app.screen = crate::app::AppScreen::WorldSelect;
    let screen = (1280, 720);
    click_shell_button(&mut app, screen, "Create New World");

    assert!(app.handle_text_input("abcdef", screen));
    app.handle_text_key(TextKey::ArrowLeft, screen);
    app.handle_text_key(TextKey::ArrowLeft, screen);
    app.set_modifiers(Modifiers {
        ctrl: false,
        shift: true,
    });
    app.handle_text_key(TextKey::ArrowLeft, screen);
    app.handle_text_key(TextKey::ArrowLeft, screen);
    app.set_modifiers(Modifiers::default());

    assert!(app.handle_text_input("XY", screen));

    assert_eq!(app.create_world_name.text(), "abXYef");
}

#[test]
fn create_world_text_shortcuts_use_clipboard() {
    let mut app = App::new(Camera::new(Vec3::new(0.0, 80.0, 0.0), 16.0 / 9.0), 1);
    app.screen = crate::app::AppScreen::WorldSelect;
    let screen = (1280, 720);
    click_shell_button(&mut app, screen, "Create New World");
    app.handle_text_input("Copied World", screen);
    app.set_modifiers(Modifiers {
        ctrl: true,
        shift: false,
    });
    let mut clipboard = MemoryClipboard::default();

    assert!(app.handle_text_shortcut_code(winit::keyboard::KeyCode::KeyA, &mut clipboard, screen));
    assert!(app.handle_text_shortcut_code(winit::keyboard::KeyCode::KeyC, &mut clipboard, screen));
    assert_eq!(clipboard.text.as_deref(), Some("Copied World"));

    assert!(app.handle_text_shortcut_code(winit::keyboard::KeyCode::KeyX, &mut clipboard, screen));
    assert_eq!(app.create_world_name.text(), "");

    clipboard.text = Some("Pasted $#@!^{}".to_string());
    assert!(app.handle_text_shortcut_code(winit::keyboard::KeyCode::KeyV, &mut clipboard, screen));
    assert_eq!(app.create_world_name.text(), "Pasted $#@!^{}");
}

#[test]
fn create_world_input_hover_uses_text_cursor_icon() {
    let mut app = App::new(Camera::new(Vec3::new(0.0, 80.0, 0.0), 16.0 / 9.0), 1);
    app.screen = crate::app::AppScreen::WorldSelect;
    let screen = (1280, 720);
    click_shell_button(&mut app, screen, "Create New World");
    let input = app
        .shell_ui_snapshot(screen, (0.0, 0.0))
        .inputs
        .into_iter()
        .next()
        .expect("create world has an input");
    app.set_cursor_position(input.rect.x + 2.0, input.rect.y + 2.0);

    assert_eq!(app.cursor_policy(screen).icon, CursorIcon::Text);
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

fn shell_button<'a>(
    snapshot: &'a crate::gui::ShellUiSnapshot,
    label: &str,
) -> &'a crate::gui::ShellButton {
    snapshot
        .buttons
        .iter()
        .find(|button| button.label == label)
        .unwrap_or_else(|| panic!("missing shell button '{label}'"))
}

fn click_shell_button(app: &mut App, screen: (u32, u32), label: &str) {
    let (x, y) = {
        let snapshot = app.shell_ui_snapshot(screen, (0.0, 0.0));
        let button = shell_button(&snapshot, label);
        (
            button.rect.x + button.rect.w * 0.5,
            button.rect.y + button.rect.h * 0.5,
        )
    };
    app.set_cursor_position(x, y);
    assert!(app.route_shell_click(screen, 0.0));
}

#[derive(Default)]
struct MemoryClipboard {
    text: Option<String>,
}

impl TextClipboard for MemoryClipboard {
    fn get_text(&mut self) -> Option<String> {
        self.text.clone()
    }

    fn set_text(&mut self, text: &str) -> bool {
        self.text = Some(text.to_string());
        true
    }
}
