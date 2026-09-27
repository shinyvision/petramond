use super::app;
use crate::app::screen::AppScreen;
use crate::app::App;
use petramond_input::controls::Control;
use petramond_math::world_pos::WorldPos;
use petramond_render::camera::Camera;
use petramond_world::gui_state::{MenuSlot, PointerButton};

fn shell_app() -> App {
    App::new(Camera::new(WorldPos::new(0.0, 80.0, 0.0), 16.0 / 9.0), 1)
}

#[test]
fn options_stack_returns_to_its_origin_and_clears_category_state() {
    let mut app = shell_app();
    app.push_screen(AppScreen::Options);
    app.push_screen(AppScreen::OptionsControls);
    app.options.begin_remap("jump");
    app.options.view_distance_preview = Some(12);
    assert_eq!(
        app.screens_under,
        vec![AppScreen::Title, AppScreen::Options]
    );

    assert!(app.close_screen());
    assert_eq!(app.screen, AppScreen::OptionsControls);
    assert!(app.options.remap().is_none());
    assert!(app.close_screen());
    assert_eq!(app.screen, AppScreen::Options);
    assert!(app.options.view_distance_preview.is_none());
    assert!(app.close_screen());
    assert_eq!(app.screen, AppScreen::Title);
    assert!(app.screens_under.is_empty());
    assert!(!app.cursor_policy().grabbed);
}

#[test]
fn a_screen_change_drops_the_old_click_streak_and_pointer_hold() {
    let mut app = app();
    assert!(!app.gui_router.doc_gather(
        MenuSlot::Inventory(0),
        PointerButton::Primary,
        false,
        1.0,
        false,
    ));
    app.controls
        .pointer
        .set_gameplay_button(PointerButton::Primary, true);
    app.open_pause();
    assert_eq!(app.screen, AppScreen::Pause);
    assert!(!app.controls.pointer.is_grabbing());
    app.resume_game();
    assert_eq!(app.screen, AppScreen::Game);
    assert!(app.controls.pointer.is_grabbing());
    assert!(
        !app.gui_router.doc_gather(
            MenuSlot::Inventory(0),
            PointerButton::Primary,
            false,
            1.1,
            true,
        ),
        "a click before the pause must not become a double click afterward"
    );
}

#[test]
fn ending_a_session_drops_its_ui_state_and_keeps_the_reconnect_cache() {
    let mut current = app();
    current.sess_mut().library_form.name = "old draft".into();
    current.sess_mut().lan_port = Some(7434);
    current.sess_mut().chat.insert_text("unsent", 0.0);
    current.enter_connection_lost("connection closed".into());

    assert!(!current.has_session());
    assert_eq!(current.screen, AppScreen::ConnectionLost);
    assert!(current.retained_section_cache.is_some());
    assert!(current.renderer_world_clear_pending);
    assert!(!current.cursor_policy().grabbed);

    current.restart_session();
    assert_eq!(current.screen, AppScreen::Game);
    assert!(current.sess().library_form.name.is_empty());
    assert!(current.sess().lan_port.is_none());
    assert!(current.sess_mut().chat.submit_or_close(0.0).is_none());
    current.handle_control(Control::CloseScreen, true);
    assert_eq!(current.screen, AppScreen::Pause);
}
