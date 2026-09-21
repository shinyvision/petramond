use super::*;
use petramond_world::controls::Control;

#[test]
fn scroll_step_needs_a_full_notch() {
    let mut p = PointerState {
        scroll_delta: 0.4,
        ..Default::default()
    };
    assert_eq!(p.take_scroll_step(), 0);
    p.scroll_delta += 0.4;
    assert_eq!(p.take_scroll_step(), 0);
    p.scroll_delta += 0.4;
    assert_eq!(p.take_scroll_step(), 1);
    assert!((p.scroll_delta - 0.2).abs() < 1e-4);
}

#[test]
fn scroll_step_is_proportional_and_signed() {
    let mut p = PointerState {
        scroll_delta: 3.0,
        ..Default::default()
    };
    assert_eq!(p.take_scroll_step(), 3);
    assert_eq!(p.scroll_delta, 0.0);

    p.scroll_delta = -2.5;
    assert_eq!(p.take_scroll_step(), -2);
    assert!((p.scroll_delta + 0.5).abs() < 1e-4);
}

#[test]
fn scroll_step_carries_remainder_across_frames() {
    let mut p = PointerState::default();
    let mut steps = 0;
    for _ in 0..25 {
        p.scroll_delta += 0.1;
        steps += p.take_scroll_step();
    }
    assert_eq!(steps, 2);
    assert!((p.scroll_delta - 0.5).abs() < 1e-4);
}

#[test]
fn game_input_gates_look_and_scroll_when_gameplay_is_disabled() {
    let mut input = InputController::default();
    input.set_control(Control::MoveForward, true);
    let mut p = PointerState {
        dx: 5.0,
        dy: -2.0,
        grabbing: true,
        left_click: true,
        right_click: true,
        left_held: true,
        scroll_delta: 2.0,
        ..Default::default()
    };

    let game_input = p.take_game_input(&mut input, false);

    assert!(!game_input.gameplay_enabled);
    assert!(game_input.movement.forward);
    assert_eq!(game_input.look_delta, (0.0, 0.0));
    assert_eq!(game_input.hotbar_scroll, 0);
    assert!(game_input.break_held);
    assert!(game_input.attack_clicked);
    assert!(game_input.place_clicked);
    assert_eq!(p.scroll_delta, 0.0);

    let game_input = p.take_game_input(&mut input, true);
    assert_eq!(game_input.look_delta, (0.0, 0.0));
}

#[test]
fn button_edges_and_held_state_feed_game_input() {
    let mut input = InputController::default();
    let mut p = PointerState::default();
    p.grab_for_gameplay();

    p.set_button(PointerButton::Primary, true);
    p.set_button(PointerButton::Primary, false);
    let game_input = p.take_game_input(&mut input, true);
    assert!(p.is_grabbing());
    assert!(!game_input.break_held);
    assert!(game_input.attack_clicked);
    assert!(!game_input.place_clicked);

    p.clear_edges();
    p.set_button(PointerButton::Secondary, true);
    p.set_button(PointerButton::Secondary, false);
    let game_input = p.take_game_input(&mut input, true);
    assert!(game_input.place_clicked);
    assert!(!game_input.attack_clicked);
}
