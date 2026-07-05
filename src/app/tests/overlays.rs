//! Screen wiring for the gameplay overlays (sleep fade, death screen): they
//! open from tick events, keep the simulation classification (not shell, not a
//! slot menu), and close only the way they're meant to.

use super::app;
use crate::app::screen::AppScreen;
use crate::controls::Control;
use crate::game::GameEvents;

fn events() -> GameEvents {
    GameEvents::default()
}

#[test]
fn sleep_overlay_opens_from_the_tick_event_and_esc_cancels_it() {
    let mut app = app();
    assert!(matches!(app.screen, AppScreen::Game));

    let mut ev = events();
    ev.open_sleep = true;
    app.handle_open_screen_events(&ev);
    assert!(matches!(app.screen, AppScreen::Sleeping));
    assert!(
        app.doc_overlay_kind().is_some(),
        "the sleep screen drives the overlay path (sim keeps ticking)"
    );
    assert!(
        app.doc_shell_kind().is_none(),
        "the sleep screen must NOT take the shell path (that would freeze the sim)"
    );

    assert!(app.handle_control(Control::CloseScreen, true));
    assert!(
        matches!(app.screen, AppScreen::Game),
        "ESC cancels the sleep back to gameplay"
    );
}

#[test]
fn sleep_overlay_closes_when_the_tick_reports_the_sleep_ended() {
    let mut app = app();
    let mut ev = events();
    ev.open_sleep = true;
    app.handle_open_screen_events(&ev);

    let mut ev = events();
    ev.sleep_ended = true;
    app.handle_open_screen_events(&ev);
    assert!(matches!(app.screen, AppScreen::Game));
}

#[test]
fn death_opens_the_death_screen_and_only_respawn_leaves_it() {
    let mut app = app();
    let mut ev = events();
    ev.player_died = true;
    app.handle_open_screen_events(&ev);
    assert!(matches!(app.screen, AppScreen::Dead));
    assert!(app.doc_overlay_kind().is_some());

    // ESC is swallowed: death cannot be escaped.
    assert!(app.handle_control(Control::CloseScreen, true));
    assert!(matches!(app.screen, AppScreen::Dead));
    // The inventory key must not open a menu over the death screen.
    assert!(app.handle_control(Control::ToggleInventory, true));
    assert!(matches!(app.screen, AppScreen::Dead));

    let mut ev = events();
    ev.respawned = true;
    app.handle_open_screen_events(&ev);
    assert!(matches!(app.screen, AppScreen::Game));
}

#[test]
fn death_while_a_container_menu_is_open_closes_the_menu_first() {
    let mut app = app();
    assert!(app.handle_control(Control::ToggleInventory, true));
    assert!(app.screen.ui_open());

    let mut ev = events();
    ev.player_died = true;
    app.handle_open_screen_events(&ev);
    assert!(matches!(app.screen, AppScreen::Dead));
}
