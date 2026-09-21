use crate::game::{
    selection_tool::SelectionMode,
    tests::common::{game_on_empty_chunk, TestGame},
    GameInput,
};
use petramond::{player::PlayerMode, schematic::SelectionBox};
use petramond_math::world_pos::WorldPos;
use petramond_world::item::{ItemStack, ItemType};

fn wand() -> TestGame {
    let mut game = game_on_empty_chunk();
    game.player.set_mode(PlayerMode::Creative);
    game.server.sessions[0]
        .player
        .set_mode(PlayerMode::Creative);
    game.server.sessions[0].player.inventory.add(ItemStack::new(
        ItemType::by_name("petramond:schematic_wand").unwrap(),
        1,
    ));
    game.sync_self_view_for_test();
    game.adjust_tool(-1);
    assert!(game.world_tools.selection.mode() == SelectionMode::Extrude);
    game.cam.pos = WorldPos::new(8.5, 80.5, 0.0);
    game.cam.yaw = 0.0;
    game.cam.pitch = 0.0;
    game.world_tools
        .selection
        .selection
        .region([8, 80, 10], [10, 82, 12], false)
        .unwrap();
    game
}

fn press(game: &mut TestGame) {
    let mut input = GameInput {
        gameplay_enabled: true,
        break_held: true,
        attack_clicked: true,
        ..Default::default()
    };
    game.world_tool_input(&mut input);
    assert!(!input.break_held && !input.attack_clicked);
    assert!(game.world_tool_holds_camera());
}

fn drag(game: &mut TestGame, delta: (f32, f32), held: bool) {
    let mut input = GameInput {
        gameplay_enabled: true,
        break_held: held,
        look_delta: delta,
        ..Default::default()
    };
    game.apply_camera_input(&input);
    game.world_tool_input(&mut input);
    assert_eq!((game.cam.yaw, game.cam.pitch), (0.0, 0.0));
    assert!(!input.break_held);
}

#[test]
fn creative_face_drag_snaps_only_along_normal_and_commits_on_release() {
    let mut game = wand();
    press(&mut game);
    assert_eq!(game.tool_overlay().and_then(|o| o.face).unwrap().axis, 2);
    drag(&mut game, (500.0, -80.0), true);
    assert_eq!(
        game.world_tools.selection.selection.regions(),
        &[SelectionBox {
            lo: [8, 80, 8],
            hi: [11, 83, 13]
        }]
    );
    assert_eq!(game.tool_overlay().and_then(|o| o.face).unwrap().plane, 8);
    drag(&mut game, (0.0, 160.0), true);
    assert_eq!(
        game.world_tools.selection.selection.regions(),
        &[SelectionBox {
            lo: [8, 80, 12],
            hi: [11, 83, 13]
        }]
    );
    drag(&mut game, (0.0, -120.0), false);
    assert_eq!(
        game.world_tools.selection.selection.regions(),
        &[SelectionBox {
            lo: [8, 80, 9],
            hi: [11, 83, 13]
        }]
    );
    assert!(!game.world_tool_holds_camera());
    game.undo_edit();
    assert_eq!(game.world_tools.selection.selection.len(), 27);
    game.redo_edit();
    assert_eq!(game.world_tools.selection.selection.len(), 36);
    game.undo_edit();
    game.undo_edit();
    assert!(game.world_tools.selection.selection.is_empty());
}

#[test]
fn creative_face_drag_cancels_on_escape_undo_mode_change_or_lost_gameplay() {
    for cancel in 0..6 {
        let mut game = wand();
        let original = game.world_tools.selection.selection.regions().to_vec();
        press(&mut game);
        drag(&mut game, (0.0, -80.0), true);
        match cancel {
            0 => {
                assert!(game.cancel_world_tools());
            }
            1 => game.undo_edit(),
            2 => {
                game.adjust_tool(1);
            }
            3 => game.world_tool_input(&mut GameInput::default()),
            4 => game.world_tool_input(&mut GameInput {
                gameplay_enabled: true,
                place_clicked: true,
                ..Default::default()
            }),
            _ => {
                game.self_view.inventory.set_active(1);
                game.world_tool_input(&mut GameInput {
                    gameplay_enabled: true,
                    ..Default::default()
                });
            }
        }
        assert!(!game.world_tool_holds_camera());
        assert_eq!(game.world_tools.selection.selection.regions(), original);
        assert!(game.tool_overlay().and_then(|o| o.face).is_none());
        assert!(game.world_tools.selection.selection.undo());
        assert!(game.world_tools.selection.selection.is_empty());
    }
}

#[test]
fn creative_face_drag_from_oblique_view_uses_the_projected_axis() {
    let mut game = wand();
    game.cam.pos = WorldPos::new(0.5, 80.5, 2.0);
    game.cam.yaw = std::f32::consts::FRAC_PI_4;
    press(&mut game);
    let axis = game.tool_overlay().and_then(|o| o.face).unwrap().axis;
    let before = game.world_tools.selection.selection.regions()[0];
    game.world_tool_input(&mut GameInput {
        gameplay_enabled: true,
        break_held: true,
        look_delta: (-100.0, 0.0),
        ..Default::default()
    });
    let after = game.world_tools.selection.selection.regions()[0];
    assert_ne!(before, after);
    for other in 0..3 {
        if other != axis {
            assert_eq!(
                (before.lo[other], before.hi[other]),
                (after.lo[other], after.hi[other])
            );
        }
    }
}
