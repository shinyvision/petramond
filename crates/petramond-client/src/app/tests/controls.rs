use super::{app, app_with_grass, cursor_over_slot};
use crate::app::{App, CursorIcon, CursorPolicy};
use petramond::net::protocol::{ClientToServer, ServerToClient};
use petramond::player::PlayerMode;
use petramond::save::WorldInfo;
use petramond_input::controls::{Control, Modifiers, TextKey, TextShortcut};
use petramond_math::world_pos::WorldPos;
use petramond_render::camera::Camera;
use petramond_world::gui_state::PointerButton;
#[cfg(feature = "audio")]
use petramond_world::sound_registry::Sound;

#[test]
fn app_starts_on_title_without_loading_a_game() {
    let app = App::new(Camera::new(WorldPos::new(0.0, 80.0, 0.0), 16.0 / 9.0), 1);

    assert_eq!(app.screen, crate::app::AppScreen::Title);
    assert!(!app.has_session(), "title screen does not preload a world");
    assert_eq!(
        app.cursor_policy(),
        CursorPolicy {
            grabbed: false,
            visible: true,
            icon: CursorIcon::Default,
        }
    );
}

#[test]
fn world_settings_requires_selection_and_hosts_the_delete_flow() {
    use petramond_world::gui_state::GuiKind;
    let mut app = App::new(Camera::new(WorldPos::new(0.0, 80.0, 0.0), 16.0 / 9.0), 1);
    app.screen = crate::app::AppScreen::WorldSelect;
    app.shell.set_worlds_for_test(test_worlds(1));
    let screen = (1280, 720);

    app.drive_doc_ui(GuiKind::WorldSelect, screen, 0.0);
    click_doc_id(&mut app, "settings");
    app.drive_doc_ui(GuiKind::WorldSelect, screen, 0.1);
    assert_eq!(app.screen, crate::app::AppScreen::WorldSelect);

    app.shell.select_world(Some(0));
    app.drive_doc_ui(GuiKind::WorldSelect, screen, 0.2);
    click_doc_id(&mut app, "settings");
    app.drive_doc_ui(GuiKind::WorldSelect, screen, 0.3);
    assert_eq!(app.screen, crate::app::AppScreen::WorldSettings);
    assert_eq!(
        app.shell.world_settings().map(|s| s.world_name.as_str()),
        Some("world-0")
    );

    app.drive_doc_ui(GuiKind::WorldSettings, screen, 0.4);
    click_doc_id(&mut app, "delete_world");
    app.drive_doc_ui(GuiKind::WorldSettings, screen, 0.5);
    assert_eq!(app.screen, crate::app::AppScreen::DeleteWorld);

    app.drive_doc_ui(GuiKind::DeleteWorld, screen, 0.6);
    click_doc_id(&mut app, "cancel");
    app.drive_doc_ui(GuiKind::DeleteWorld, screen, 0.7);
    assert_eq!(app.screen, crate::app::AppScreen::WorldSelect);
    assert_eq!(app.shell.selected_world(), Some(0));
}

#[test]
fn create_world_document_input_types_selects_and_uses_clipboard() {
    use petramond_world::gui_state::GuiKind;
    let mut app = App::new(Camera::new(WorldPos::new(0.0, 80.0, 0.0), 16.0 / 9.0), 1);
    app.screen = crate::app::AppScreen::CreateWorld;
    let screen = (1280, 720);
    let shared = std::rc::Rc::new(std::cell::RefCell::new(None::<String>));
    app.ui
        .set_clipboard(Box::new(SharedClipboard(shared.clone())));
    let drive = |app: &mut App, now: f64| app.drive_doc_ui(GuiKind::CreateWorld, screen, now);

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

    app.set_modifiers(Modifiers {
        ctrl: true,
        shift: false,
        ..Modifiers::default()
    });
    assert!(app.handle_text_shortcut_code(petramond_input::keycode::KeyCode::KeyA));
    assert!(app.handle_text_shortcut_code(petramond_input::keycode::KeyCode::KeyC));
    drive(&mut app, 0.3);
    assert_eq!(shared.borrow().as_deref(), Some("abXYef"));

    assert!(app.handle_text_shortcut_code(petramond_input::keycode::KeyCode::KeyX));
    drive(&mut app, 0.4);
    assert_eq!(app.ui.state_mut().get_str("create_name"), Some(""));

    *shared.borrow_mut() = Some("Pasted $#@!^{}".to_string());
    assert!(app.handle_text_shortcut_code(petramond_input::keycode::KeyCode::KeyV));
    drive(&mut app, 0.5);
    assert_eq!(
        app.ui.state_mut().get_str("create_name"),
        Some("Pasted $#@!^{}")
    );

    assert!(app.handle_text_key(TextKey::Tab));
    drive(&mut app, 0.6);
    assert!(app.handle_text_input("42"));
    drive(&mut app, 0.7);
    assert_eq!(app.ui.state_mut().get_str("create_seed"), Some("42"));
    assert_eq!(
        app.ui.state_mut().get_str("create_name"),
        Some("Pasted $#@!^{}"),
        "name field must keep its text after tabbing away"
    );
}

#[test]
fn document_shell_screens_flow_via_pointer_and_keys() {
    use petramond_world::gui_state::GuiKind;
    let mut app = App::new(Camera::new(WorldPos::new(0.0, 80.0, 0.0), 16.0 / 9.0), 1);
    let screen = (1280, 720);
    let click_id = click_doc_id;

    assert_eq!(app.doc_ui_kind(), Some(GuiKind::Title));
    app.drive_doc_ui(GuiKind::Title, screen, 0.0);
    click_id(&mut app, "start");
    app.drive_doc_ui(GuiKind::Title, screen, 0.1);
    assert_eq!(app.screen, crate::app::AppScreen::WorldSelect);

    app.shell.set_worlds_for_test(test_worlds(2));
    app.drive_doc_ui(GuiKind::WorldSelect, screen, 0.2);
    app.handle_text_key(TextKey::ArrowDown);
    app.drive_doc_ui(GuiKind::WorldSelect, screen, 0.3);
    assert_eq!(app.shell.selected_world(), Some(0));
    app.handle_text_key(TextKey::ArrowDown);
    app.drive_doc_ui(GuiKind::WorldSelect, screen, 0.4);
    assert_eq!(app.shell.selected_world(), Some(1));

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

#[cfg(feature = "audio")]
#[test]
fn shell_button_and_toggle_activations_play_ui_click_sound() {
    use petramond_world::gui_state::GuiKind;
    let mut app = App::new(Camera::new(WorldPos::new(0.0, 80.0, 0.0), 16.0 / 9.0), 1);
    let screen = (1280, 720);

    app.drive_doc_ui(GuiKind::Title, screen, 0.0);
    app.sound.take_played_for_test();
    click_doc_id(&mut app, "start");
    app.drive_doc_ui(GuiKind::Title, screen, 0.1);
    assert_eq!(app.sound.take_played_for_test(), vec![Sound::UiClick]);

    app.drive_doc_ui(GuiKind::Demo, screen, 0.2);
    app.sound.take_played_for_test();
    click_doc_id(&mut app, "t1");
    app.drive_doc_ui(GuiKind::Demo, screen, 0.3);
    assert_eq!(app.sound.take_played_for_test(), vec![Sound::UiClick]);
}

#[test]
fn document_game_menu_clicks_do_not_play_shell_ui_click_sound() {
    let mut app = app_with_grass();
    app.handle_control(Control::ToggleInventory, true);
    let screen = (1280, 720);
    let (cx, cy) = cursor_over_slot(&mut app, screen, 0);

    app.sound.take_played_for_test();
    app.set_cursor_position(cx, cy);
    assert!(app.click_screen_for_test(screen, 0.0));
    assert!(app.sound.take_played_for_test().is_empty());
}

#[test]
fn play_after_rename_opens_the_original_save_directory() {
    let dir_name = "rename-regress-test";
    let _ = petramond::save::delete_world(dir_name);
    petramond::save::write_world_metadata(dir_name).expect("create world dir");
    petramond::save::rename_world(dir_name, "Renamed Display Name").expect("rename");

    let mut app = App::new(Camera::new(WorldPos::new(0.0, 80.0, 0.0), 16.0 / 9.0), 1);
    app.shell.refresh_worlds();
    let idx = app
        .shell
        .worlds()
        .iter()
        .position(|w| w.dir_name == dir_name)
        .expect("renamed world listed");
    assert_eq!(app.shell.worlds()[idx].name, "Renamed Display Name");
    app.shell.select_world(Some(idx));
    app.play_selected_world();
    assert!(app.has_session(), "world opened");
    app.save_on_exit();
    drop(app);

    assert!(
        petramond::save::world_dir(dir_name)
            .join("level.dat")
            .exists(),
        "the ORIGINAL directory received the save"
    );
    assert!(
        !petramond::save::world_dir("Renamed Display Name").exists(),
        "no fresh world appeared under the display name"
    );
    let _ = petramond::save::delete_world(dir_name);
}

#[test]
fn world_settings_tabs_swap_pages() {
    use crate::app::shell_state::SettingsTab;
    use petramond_world::gui_state::GuiKind;
    let mut app = App::new(Camera::new(WorldPos::new(0.0, 80.0, 0.0), 16.0 / 9.0), 1);
    let screen = (1280, 720);
    app.screen = crate::app::AppScreen::WorldSelect;
    app.shell.set_worlds_for_test(test_worlds(1));
    app.shell.select_world(Some(0));
    assert!(app.shell.open_world_settings());
    app.screen = crate::app::AppScreen::WorldSettings;

    app.drive_doc_ui(GuiKind::WorldSettings, screen, 0.0);
    assert!(app.ui.out().rect("delete_world").is_some());
    assert!(
        app.ui.out().rect("mod_scroll").is_none(),
        "the Mods page is dropped while the World tab is active"
    );

    app.handle_text_key(TextKey::ArrowRight);
    app.drive_doc_ui(GuiKind::WorldSettings, screen, 0.1);
    assert_eq!(
        app.shell.world_settings().map(|s| s.tab),
        Some(SettingsTab::Mods)
    );
    app.drive_doc_ui(GuiKind::WorldSettings, screen, 0.2);
    assert!(app.ui.out().rect("mod_scroll").is_some());
    assert!(
        app.ui.out().rect("delete_world").is_some(),
        "the footer is shared chrome on every tab"
    );

    let bar = app.ui.out().rect("tabs").expect("tab bar solves");
    app.set_cursor_position((bar.x + 10) as f32, (bar.y + bar.h / 2) as f32);
    app.set_pointer_button(PointerButton::Primary, true);
    app.set_pointer_button(PointerButton::Primary, false);
    app.drive_doc_ui(GuiKind::WorldSettings, screen, 0.3);
    assert_eq!(
        app.shell.world_settings().map(|s| s.tab),
        Some(SettingsTab::World)
    );
    app.drive_doc_ui(GuiKind::WorldSettings, screen, 0.4);
    assert!(app.ui.out().rect("mod_scroll").is_none());
}

#[test]
fn create_world_writes_buffered_settings_at_create() {
    use petramond_world::gui_state::GuiKind;
    let name = "tabbed-create-regress-test";
    let dir_name = petramond::save::dir_name_for(name);
    let _ = petramond::save::delete_world(&dir_name);

    let mut app = App::new(Camera::new(WorldPos::new(0.0, 80.0, 0.0), 16.0 / 9.0), 1);
    let screen = (1280, 720);
    app.screen = crate::app::AppScreen::WorldSelect;
    app.drive_doc_ui(GuiKind::WorldSelect, screen, 0.0);
    click_doc_id(&mut app, "create");
    app.drive_doc_ui(GuiKind::WorldSelect, screen, 0.1);
    assert_eq!(app.screen, crate::app::AppScreen::CreateWorld);
    assert!(
        app.shell.create_world().is_some(),
        "create opens with a session"
    );

    app.drive_doc_ui(GuiKind::CreateWorld, screen, 0.2);
    let r = app.ui.out().rect("create_name").expect("name input rect");
    app.set_cursor_position((r.x + r.w / 2) as f32, (r.y + r.h / 2) as f32);
    app.set_pointer_button(PointerButton::Primary, true);
    app.set_pointer_button(PointerButton::Primary, false);
    assert!(app.handle_text_input(name));
    app.drive_doc_ui(GuiKind::CreateWorld, screen, 0.3);
    app.drive_doc_ui(GuiKind::CreateWorld, screen, 0.4);

    app.shell
        .create_world_mut()
        .expect("session while the screen is open")
        .settings
        .disabled_mods
        .insert("testpack".into());

    click_doc_id(&mut app, "create");
    app.drive_doc_ui(GuiKind::CreateWorld, screen, 0.5);
    assert!(app.has_session(), "Create started the world");
    let settings = petramond::save::read_world_settings(&dir_name);
    assert!(
        settings.disabled_mods.contains("testpack"),
        "the buffered choices landed in the new world's settings.json"
    );
    app.save_on_exit();
    drop(app);
    let _ = petramond::save::delete_world(&dir_name);
}

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
    app.controls.pointer.grab_for_gameplay();
    app.handle_control(Control::ToggleInventory, true);
    assert!(app.screen.inventory_open());
    assert!(!app.controls.pointer.is_grabbing());
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
    assert!(!app.controls.pointer.is_grabbing());

    assert!(app.handle_control(Control::CloseScreen, true));
    assert!(!app.screen.inventory_open());
    assert!(app.controls.pointer.is_grabbing());
}

#[test]
fn escape_with_inventory_closed_opens_pause() {
    let mut app = app();
    assert!(!app.screen.inventory_open());
    assert!(app.handle_control(Control::CloseScreen, true));
    assert!(!app.screen.inventory_open());
    assert_eq!(app.screen, crate::app::AppScreen::Pause);
    assert!(!app.controls.pointer.is_grabbing());
}

#[test]
fn escape_on_pause_resumes_gameplay_and_regrabs() {
    let mut app = app();
    app.handle_control(Control::CloseScreen, true);
    assert_eq!(app.screen, crate::app::AppScreen::Pause);

    assert!(app.handle_control(Control::CloseScreen, true));

    assert_eq!(app.screen, crate::app::AppScreen::Game);
    assert!(app.controls.pointer.is_grabbing());
}

#[test]
fn save_and_quit_returns_to_title_and_drops_game() {
    let mut app = app();
    app.handle_control(Control::CloseScreen, true);
    assert_eq!(app.screen, crate::app::AppScreen::Pause);

    app.save_and_quit_to_title();

    assert_eq!(app.screen, crate::app::AppScreen::Title);
    assert!(!app.has_session());
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

    let msgs = app.game_mut().take_outbox_for_test();
    assert!(msgs
        .iter()
        .any(|msg| matches!(msg, ClientToServer::ChatSend { text } if text == "hello")));

    let replies = app.send_and_pump(msgs);
    let chat = replies.iter().find_map(|msg| match msg {
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

    let msgs = app.game_mut().take_outbox_for_test();
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

    let msgs = app.game_mut().take_outbox_for_test();
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
    use petramond_world::gui_state::GuiKind;
    let mut app = App::new(Camera::new(WorldPos::new(0.0, 80.0, 0.0), 16.0 / 9.0), 1);
    let screen = (1280, 720);

    app.drive_doc_ui(GuiKind::Title, screen, 0.0);
    click_doc_id(&mut app, "options");
    app.drive_doc_ui(GuiKind::Title, screen, 0.1);
    assert_eq!(app.screen, crate::app::AppScreen::Options);

    app.drive_doc_ui(GuiKind::Options, screen, 0.2);
    click_doc_id(&mut app, "graphics");
    app.drive_doc_ui(GuiKind::Options, screen, 0.3);
    assert_eq!(app.screen, crate::app::AppScreen::OptionsGraphics);

    use petramond::save::client::AntiAliasing;
    for (i, expected) in AntiAliasing::ALL.into_iter().enumerate() {
        let now = 0.4 + i as f64 * 0.3;
        app.drive_doc_ui(GuiKind::OptionsGraphics, screen, now);
        let previous = app.options.settings.anti_aliasing;
        let r = app.ui.out().rect("anti_aliasing").expect("AA slider");
        let fraction = i as f32 / (AntiAliasing::ALL.len() - 1) as f32;
        let x = r.x as f32 + (fraction * r.w as f32).clamp(1.0, r.w as f32 - 1.0);
        app.options.take_renderer_dirty();
        app.set_cursor_position(x, (r.y + r.h / 2) as f32);
        app.set_pointer_button(PointerButton::Primary, true);
        app.drive_doc_ui(GuiKind::OptionsGraphics, screen, now + 0.1);
        assert_eq!(app.options.anti_aliasing_preview, Some(expected));
        assert_eq!(
            app.options.settings.anti_aliasing, previous,
            "dragging must not change renderer settings"
        );
        assert!(!app.options.renderer_dirty());
        app.set_pointer_button(PointerButton::Primary, false);
        app.drive_doc_ui(GuiKind::OptionsGraphics, screen, now + 0.2);
        assert_eq!(app.options.settings.anti_aliasing, expected);
        assert_eq!(app.options.anti_aliasing_preview, None);
        assert!(
            app.options.renderer_dirty(),
            "release must reach the renderer"
        );
    }

    let r = app.ui.out().rect("anti_aliasing").unwrap();
    app.set_cursor_position((r.x + 1) as f32, (r.y + r.h / 2) as f32);
    app.set_pointer_button(PointerButton::Primary, true);
    app.drive_doc_ui(GuiKind::OptionsGraphics, screen, 2.0);
    let applied = app.options.settings.anti_aliasing;
    assert!(app.options.anti_aliasing_preview.is_some());

    app.handle_control(Control::CloseScreen, true);
    assert_eq!(app.options.anti_aliasing_preview, None);
    assert_eq!(app.options.settings.anti_aliasing, applied);
    assert_eq!(app.screen, crate::app::AppScreen::Options);
    app.handle_control(Control::CloseScreen, true);
    assert_eq!(app.screen, crate::app::AppScreen::Title);
}

#[test]
fn options_opened_from_pause_returns_to_pause() {
    use petramond_world::gui_state::GuiKind;
    let mut app = app();
    let screen = (1280, 720);
    app.handle_control(Control::CloseScreen, true);

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

#[test]
fn controls_screen_remaps_a_key_and_esc_or_reclick_cancels() {
    use petramond_input::controls::{BindableAction, Binding, BoundInput};
    use petramond_input::keycode::KeyCode;
    use petramond_world::gui_state::GuiKind;

    let mut app = App::new(Camera::new(WorldPos::new(0.0, 80.0, 0.0), 16.0 / 9.0), 1);
    let screen = (1280, 720);
    app.screen = crate::app::AppScreen::OptionsControls;

    app.drive_doc_ui(GuiKind::OptionsControls, screen, 0.0);
    click_bind_row(&mut app, "strafe_right");
    app.drive_doc_ui(GuiKind::OptionsControls, screen, 0.1);
    assert_eq!(app.options.remap(), Some("strafe_right"));

    assert!(app.remap_capture_key(KeyCode::Escape, true));
    assert_eq!(app.options.remap(), None);
    assert_eq!(
        app.options
            .settings
            .bindings
            .binding(BindableAction::StrafeRight),
        Binding::key(KeyCode::KeyD)
    );

    app.drive_doc_ui(GuiKind::OptionsControls, screen, 0.2);
    click_bind_row(&mut app, "strafe_right");
    app.drive_doc_ui(GuiKind::OptionsControls, screen, 0.3);
    click_bind_row(&mut app, "strafe_left");
    app.drive_doc_ui(GuiKind::OptionsControls, screen, 0.4);
    assert_eq!(app.options.remap(), Some("strafe_left"));

    assert!(app.remap_capture_key(KeyCode::KeyK, true));
    assert_eq!(app.options.remap(), None);
    assert_eq!(
        app.options
            .settings
            .bindings
            .binding(BindableAction::StrafeLeft),
        Binding::key(KeyCode::KeyK)
    );

    assert!(app.handle_raw_key(KeyCode::KeyK, true));
    assert!(app.take_game_input().movement.left);
    assert!(app.handle_raw_key(KeyCode::KeyK, false));
    assert!(!app.take_game_input().movement.left);
    assert!(app.handle_raw_key(KeyCode::KeyA, true));
    assert!(!app.take_game_input().movement.left);
    let _ = app.handle_raw_key(KeyCode::KeyA, false);

    app.options.begin_remap("sprint");
    assert!(app.remap_capture_key(KeyCode::AltLeft, true));
    assert_eq!(app.options.remap(), Some("sprint"), "hold = chord start");
    assert!(app.remap_capture_key(KeyCode::AltLeft, false));
    assert_eq!(
        app.options
            .settings
            .bindings
            .binding(BindableAction::Sprint)
            .input,
        BoundInput::Key(KeyCode::AltLeft)
    );
}

fn click_bind_row(app: &mut App, action_id: &str) {
    let index =
        crate::app::shell_docs::controls_action_row_index(&app.controls.action_table, action_id)
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

#[test]
fn attack_rebinds_from_mouse_to_key() {
    use petramond_input::controls::{BindableAction, Binding};
    use petramond_input::keycode::KeyCode;

    let mut app = app();
    assert!(app.screen.gameplay_enabled());

    app.handle_raw_mouse(petramond_input::keycode::MouseButton::Left, true);
    let input = app.take_game_input();
    assert!(input.break_held && input.attack_clicked);
    app.handle_raw_mouse(petramond_input::keycode::MouseButton::Left, false);
    app.controls.pointer.clear_edges();

    app.options
        .settings
        .bindings
        .set_id(BindableAction::Attack.id(), Binding::key(KeyCode::KeyF));
    assert!(app.handle_raw_key(KeyCode::KeyF, true));
    let input = app.take_game_input();
    assert!(
        input.break_held,
        "the rebound key mines like the button did"
    );
    assert!(app.handle_raw_key(KeyCode::KeyF, false));
    let input = app.take_game_input();
    assert!(!input.break_held);
    app.controls.pointer.clear_edges();
    app.handle_raw_mouse(petramond_input::keycode::MouseButton::Left, true);
    let input = app.take_game_input();
    assert!(
        !input.break_held,
        "left click moved off Attack; it must not mine"
    );
    app.handle_raw_mouse(petramond_input::keycode::MouseButton::Left, false);
}

#[test]
fn mod_key_actions_join_the_controls_table_with_their_own_category() {
    let app = app();
    let table = &app.controls.action_table;
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

#[test]
fn mod_bound_key_dispatches_to_the_client_mod() {
    use petramond_input::keycode::KeyCode;
    let mut app = app();
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

#[test]
fn mod_key_actions_fire_only_in_their_contexts_and_report_their_binding() {
    use petramond_input::controls::Binding;
    use petramond_input::keycode::KeyCode;
    let mut app = app();
    app.update_frame((1280, 720));
    let press = |app: &mut crate::app::App, key| {
        app.handle_raw_key(key, true);
        let _ = app.handle_raw_key(key, false);
        app.update_frame((1280, 720));
    };
    press(&mut app, KeyCode::KeyM);
    assert!(app.screen.client_canvas_open());
    press(&mut app, KeyCode::KeyN);
    assert!(
        app.screen.client_canvas_open(),
        "a gameplay-only action never fires over a screen, got {:?}",
        app.screen
    );
    press(&mut app, KeyCode::KeyM);
    assert!(
        !app.screen.client_canvas_open(),
        "M is registered for the map canvas too, so it closes it"
    );

    let label = |app: &crate::app::App| {
        app.client_mods_now()
            .unwrap()
            .presented()
            .lock()
            .key_labels
            .get("minimap:open_map")
            .cloned()
    };
    assert_eq!(label(&app).as_deref(), Some("M"));
    app.options
        .settings
        .bindings
        .set_id("minimap:open_map", Binding::key(KeyCode::F9));
    app.publish_key_labels();
    assert_eq!(label(&app).as_deref(), Some("F9"));
}

#[test]
fn canvas_wheel_scroll_reaches_the_client_mod() {
    use petramond_input::keycode::KeyCode;
    let mut app = app();
    app.update_frame((1280, 720));
    assert!(app.handle_raw_key(KeyCode::KeyM, true));
    let _ = app.handle_raw_key(KeyCode::KeyM, false);
    app.update_frame((1280, 720));
    assert!(app.screen.client_canvas_open());
    let view = |app: &crate::app::App| {
        app.game()
            .client_mod_canvas_view("minimap:full_map")
            .expect("the world map publishes a retained scene")
            .offset
    };
    let before = view(&app);
    app.compose_client_overlays((1280, 720));
    app.set_cursor_position(420.0, 130.0);
    app.add_scroll_delta(-1.0);
    app.update_frame((1280, 720));
    let zoomed = view(&app);
    assert_ne!(before, zoomed, "the wheel notch reached the minimap");
    app.set_cursor_position(10.0, 360.0);
    app.add_scroll_delta(-1.0);
    app.update_frame((1280, 720));
    assert_eq!(zoomed, view(&app), "off-canvas wheel travel is dropped");
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

#[test]
fn menu_click_that_enters_gameplay_leaves_no_mining_held() {
    let mut app = app();
    app.handle_control(Control::CloseScreen, true);
    app.handle_raw_mouse(petramond_input::keycode::MouseButton::Left, true);
    app.resume_game();
    app.handle_raw_mouse(petramond_input::keycode::MouseButton::Left, false);
    let input = app.take_game_input();
    assert!(
        !input.break_held && !input.attack_clicked,
        "no stale mining from the menu press"
    );
}

#[test]
fn the_screen_shake_checkbox_toggles_the_setting_and_reaches_the_renderer() {
    use petramond_world::gui_state::GuiKind;
    let mut app = App::new(Camera::new(WorldPos::new(0.0, 80.0, 0.0), 16.0 / 9.0), 1);
    let screen = (1280, 720);
    assert!(
        app.options.settings.screen_shake,
        "screen shake defaults on"
    );
    app.screen = crate::app::AppScreen::OptionsGraphics;
    for (i, expected) in [false, true].into_iter().enumerate() {
        let now = i as f64 * 0.3;
        app.drive_doc_ui(GuiKind::OptionsGraphics, screen, now);
        app.options.take_renderer_dirty();
        click_doc_id(&mut app, "screen_shake");
        app.drive_doc_ui(GuiKind::OptionsGraphics, screen, now + 0.1);
        assert_eq!(app.options.settings.screen_shake, expected);
        assert_eq!(app.options.settings.graphics().screen_shake, expected);
        assert!(
            app.options.renderer_dirty(),
            "the toggle must reach the renderer"
        );
    }
}

/// A tool-adjust chord that finds nothing to adjust must step the hotbar the
/// way that same wheel notch always does.
///
/// Sprint defaults to Left Ctrl and tool adjust to Ctrl + wheel, so EVERY
/// notch taken while sprinting resolves to the more specific tool chord. The
/// two pairs are bound independently, so when a player binds tool adjust the
/// opposite way round from the hotbar, the bindings must be resolved
/// independently. They are set explicitly rather than relying on the
/// shipped defaults: the rule under test is "the hotbar's binding decides",
/// not any particular default.
#[test]
fn a_tool_adjust_with_nothing_to_adjust_steps_the_hotbar_the_way_the_wheel_does() {
    use petramond_input::controls::{
        BindMods, BindableAction, Binding, BoundInput, Modifiers, ScrollDir,
    };
    let chord = |dir| Binding {
        mods: BindMods {
            ctrl: true,
            ..Default::default()
        },
        input: BoundInput::Scroll(dir),
    };
    for (adjust_next, adjust_prev) in [
        (ScrollDir::Down, ScrollDir::Up),
        (ScrollDir::Up, ScrollDir::Down),
    ] {
        let mut app = super::app();
        assert!(app.screen.gameplay_enabled());
        app.options.settings.bindings.set_id(
            BindableAction::HotbarNext.id(),
            Binding::scroll(ScrollDir::Down),
        );
        app.options.settings.bindings.set_id(
            BindableAction::HotbarPrev.id(),
            Binding::scroll(ScrollDir::Up),
        );
        app.options
            .settings
            .bindings
            .set_id(BindableAction::AdjustToolNext.id(), chord(adjust_next));
        app.options
            .settings
            .bindings
            .set_id(BindableAction::AdjustToolPrev.id(), chord(adjust_prev));
        app.rebuild_action_table();

        for notch in [-1.0, 1.0] {
            app.set_modifiers(Modifiers::default());
            app.add_scroll_delta(notch);
            let plain = app.controls.input.take_hotbar_steps();
            assert_ne!(plain, 0, "the bare wheel steps the hotbar");

            app.handle_raw_key(petramond_input::keycode::KeyCode::ControlLeft, true);
            app.set_modifiers(Modifiers {
                ctrl: true,
                ..Default::default()
            });
            app.add_scroll_delta(notch);
            let sprinting = app.controls.input.take_hotbar_steps();
            app.handle_raw_key(petramond_input::keycode::KeyCode::ControlLeft, false);

            assert_eq!(
                sprinting, plain,
                "notch {notch} moved the hotbar {sprinting} while sprinting \
                 but {plain} otherwise (tool adjust next = {adjust_next:?})"
            );
        }
    }
}
