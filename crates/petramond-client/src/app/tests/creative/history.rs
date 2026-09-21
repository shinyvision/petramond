use super::*;
use petramond::net::protocol::{ClientToServer, PlayerAction};
use petramond::schematic::CreativeAction;
use petramond_world::{controls::Modifiers, keycode::KeyCode};

fn shortcut(app: &mut TestApp, redo: bool) {
    app.set_modifiers(Modifiers {
        ctrl: true,
        shift: redo,
        ..Default::default()
    });
    app.handle_raw_key(KeyCode::KeyZ, true);
    app.handle_raw_key(KeyCode::KeyZ, true);
    app.handle_raw_key(KeyCode::KeyZ, false);
    app.set_modifiers(Modifiers::default());
}

#[test]
fn redo_chord_replays_selection_once_and_respects_menu_and_wand_routing() {
    let mut app = creative_app();
    app.close_screen();
    let wand = ItemType::by_name("petramond:schematic_wand").unwrap();
    app.add_to_inventory(ItemStack::new(wand, 1));
    let slot = (0..9)
        .find(|i| app.inventory().slot(*i).is_some_and(|s| s.item == wand))
        .unwrap();
    let game = app.game.as_mut().unwrap();
    game.set_active_hotbar(slot as u8);
    for x in 0..3 {
        game.world_tools
            .selection
            .selection
            .region([x, 0, 0], [x, 0, 0], false)
            .unwrap();
    }
    shortcut(&mut app, false);
    shortcut(&mut app, false);
    assert_eq!(app.game().world_tools.selection.selection.len(), 1);
    shortcut(&mut app, true);
    assert_eq!(
        app.game().world_tools.selection.selection.len(),
        2,
        "redo beats undo and ignores key repeat"
    );
    app.handle_control(Control::ToggleInventory, true);
    app.handle_control(Control::ToggleInventory, false);
    shortcut(&mut app, true);
    assert_eq!(
        app.game().world_tools.selection.selection.len(),
        2,
        "menus consume gameplay shortcuts"
    );
    app.close_screen();
    shortcut(&mut app, true);
    assert_eq!(app.game().world_tools.selection.selection.len(), 3);

    app.game
        .as_mut()
        .unwrap()
        .world_tools
        .selection
        .set_pending_corner([5; 3]);
    shortcut(&mut app, false);
    assert!(!app.game().world_tools.selection.has_pending_corner());
    assert_eq!(app.game().world_tools.selection.selection.len(), 3);
    shortcut(&mut app, true);
    assert!(
        !app.game().world_tools.selection.has_pending_corner(),
        "unfinished corners are cancellation, not edits"
    );

    for preview in [false, true] {
        let game = app.game.as_mut().unwrap();
        game.set_active_hotbar(if preview { slot as u8 } else { 0 });
        if preview {
            game.schematic_preview
                .begin_paste(std::sync::Arc::new(schematic_fixture()));
        }
        game.take_outbox_for_test();
        shortcut(&mut app, true);
        let sent = app.game.as_mut().unwrap().take_outbox_for_test();
        assert_eq!(
            sent,
            vec![ClientToServer::Action(PlayerAction::Creative(
                CreativeAction::Redo
            ))]
        );
    }
    app.server.sessions[0].player.set_mode(PlayerMode::Survival);
    let state = app.server.build_self_state(0);
    app.game
        .as_mut()
        .unwrap()
        .apply_tick_update(Box::new(petramond::net::protocol::TickUpdate {
            self_state: Some(state),
            ..Default::default()
        }));
    assert!(!app.game().creative_mode());
    shortcut(&mut app, true);
    assert!(app.game.as_mut().unwrap().take_outbox_for_test().is_empty());
}
