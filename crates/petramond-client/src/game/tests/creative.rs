use super::common::game_on_empty_chunk;
use crate::game::GameInput;
use petramond::player::PlayerMode;
use petramond_world::gui_state::GuiKind;
mod extrude;

#[test]
fn creative_mode_inventory_and_double_jump_follow_server_authority() {
    let mut game = game_on_empty_chunk();
    let input = GameInput {
        gameplay_enabled: true,
        ..Default::default()
    };
    game.toggle_creative_mode();
    for _ in 0..3 {
        game.tick(0.05, &input);
    }
    assert_eq!(game.server.sessions[0].player.mode(), PlayerMode::Creative);
    assert!(game.creative_mode());
    assert!(!game.creative_flying());
    game.jump_pressed(10.0);
    game.tick(0.05, &input);
    assert!(!game.creative_flying());
    game.jump_pressed(10.1);
    for _ in 0..3 {
        game.tick(0.05, &input);
    }
    assert!(game.creative_flying());
    game.request_open_inventory();
    for _ in 0..3 {
        game.tick(0.05, &GameInput::default());
    }
    assert_eq!(
        game.server.sessions[0].menu.target().kind(),
        Some(GuiKind::Creative)
    );
}

#[test]
fn wand_removes_selected_air_and_undo_restores_it_without_breaking_world_blocks() {
    use petramond_math::world_pos::WorldPos;
    use petramond_world::item::{ItemStack, ItemType};
    let mut game = game_on_empty_chunk();
    game.server.sessions[0]
        .player
        .set_mode(PlayerMode::Creative);
    game.player.set_mode(PlayerMode::Creative);
    let wand = ItemType::by_name("petramond:schematic_wand").unwrap();
    game.server.sessions[0]
        .player
        .inventory
        .add(ItemStack::new(wand, 1));
    game.sync_self_view_for_test();
    assert!(game.held_world_tool().is_some());
    assert!(game.adjust_tool(1));
    game.cam.pos = WorldPos::new(8.5, 80.5, 0.5);
    game.cam.yaw = 0.0;
    game.cam.pitch = 0.0;
    game.world_tools
        .selection
        .selection
        .region([8, 80, 5], [8, 80, 6], false)
        .unwrap();
    let mut input = GameInput {
        gameplay_enabled: true,
        place_clicked: true,
        use_held: true,
        ..Default::default()
    };
    game.world_tool_input(&mut input);
    assert!(!input.place_clicked && !input.use_held);
    assert_eq!(
        game.world_tools
            .selection
            .selection
            .cells()
            .collect::<Vec<_>>(),
        vec![[8, 80, 6]]
    );
    game.undo_edit();
    assert_eq!(game.world_tools.selection.selection.len(), 2);
    assert!(!game.world_tools.selection.has_pending_corner());
    game.adjust_tool(-1);
    game.world_tool_input(&mut GameInput {
        gameplay_enabled: true,
        place_clicked: true,
        ..Default::default()
    });
    assert_eq!(
        game.world_tools.selection.pending_corner(),
        Some([8, 80, 5])
    );
    game.undo_edit();
    assert!(!game.world_tools.selection.has_pending_corner());
    assert_eq!(
        game.world_tools.selection.selection.len(),
        2,
        "undo cancels an unfinished region first"
    );
}

#[test]
fn schematic_preview_and_placement_share_the_rotated_footprint_center_and_height() {
    use petramond::{
        net::protocol::{ClientToServer, PlayerAction},
        schematic::{CellData, CreativeAction, ResolvedCell, Schematic, SchematicCell},
    };
    use petramond_math::{math::IVec3, world_pos::WorldPos};
    use petramond_world::{
        block::{Block, ShapeState},
        chunk::ChunkPos,
    };
    use std::sync::Arc;
    let mut game = game_on_empty_chunk();
    game.player.set_mode(PlayerMode::Creative);
    game.game
        .replica
        .insert_empty_column_for_test(ChunkPos::new(0, 0));
    game.game.replica.set_block_world(8, 80, 6, Block::Stone);
    game.cam.pos = WorldPos::new(8.5, 80.5, 0.5);
    game.cam.yaw = 0.0;
    game.cam.pitch = 0.0;
    let data = CellData::capture(&ResolvedCell {
        block: Block::OakPlanks,
        state: ShapeState::NONE,
        fluid: 0,
        kv: Default::default(),
        container: None,
        furnace: None,
    });
    for (size, origins) in [
        ([5, 1, 5], [[6, 80, 3], [6, 80, 3]]),
        ([5, 1, 3], [[6, 80, 4], [7, 80, 3]]),
        ([4, 1, 2], [[6, 80, 4], [7, 80, 3]]),
    ] {
        game.cancel_world_tools();
        game.schematic_preview.begin_paste(Arc::new(
            Schematic::from_cells(
                "Footprint".into(),
                size,
                (0..size[0])
                    .flat_map(|x| (0..size[2]).map(move |z| [x, 0, z]))
                    .map(|pos| SchematicCell {
                        pos,
                        data: data.clone(),
                    }),
            )
            .unwrap(),
        ));
        let mut height = 0;
        for turns in 0..4 {
            game.world_tool_input(&mut GameInput::default());
            let scene = game.schematic_preview.scene().cloned().unwrap();
            assert!(game.raise_schematic_preview(if turns % 2 == 0 { 3 } else { -5 }));
            height += if turns % 2 == 0 { 3 } else { -5 };
            let mut expected = origins[turns % 2];
            expected[1] += height;
            game.take_outbox_for_test();
            game.world_tool_input(&mut GameInput {
                gameplay_enabled: true,
                place_clicked: true,
                ..Default::default()
            });
            assert_eq!(game.schematic_preview.origin(), Some(expected));
            assert!(
                Arc::ptr_eq(&scene, game.schematic_preview.scene().unwrap()),
                "height changes must reuse the mesh scene"
            );
            // The paste names its design; the archive follows only once the
            // server says the world lacks it.
            use petramond::schematic::share::{BlobReceiver, SchematicNotice, SchematicRequest};
            let deadline = std::time::Instant::now();
            let mut placed_at = None;
            let mut receiver: Option<BlobReceiver> = None;
            let (schematic, origin, rotation) = loop {
                game.poll_schematic_share();
                let mut notices = Vec::new();
                for action in game.take_outbox_for_test() {
                    match action {
                        ClientToServer::Action(PlayerAction::Creative(CreativeAction::Place {
                            digest,
                            origin,
                            turns,
                        })) => {
                            placed_at = Some((digest, origin, turns));
                            notices.push(SchematicNotice::Want { digest });
                        }
                        ClientToServer::Action(PlayerAction::Schematic(
                            SchematicRequest::Blob(packet),
                        )) => match receiver.as_mut() {
                            None => {
                                let (digest, ..) = placed_at.expect("a blob follows its request");
                                receiver =
                                    Some(BlobReceiver::begin(&packet, digest, u64::MAX).unwrap());
                            }
                            Some(receiver) => receiver.receive(packet).unwrap(),
                        },
                        _ => {}
                    }
                }
                if let Some(receiver) = receiver.as_mut() {
                    notices.extend(receiver.take_credit().map(SchematicNotice::Blob));
                    if let Some(bytes) = receiver.finish() {
                        let (_, origin, turns) = placed_at.unwrap();
                        let schematic =
                            petramond::schematic::archive::decode(&bytes.unwrap()).unwrap();
                        break (schematic, origin, turns);
                    }
                }
                game.receive_schematic_notices(notices);
                assert!(deadline.elapsed().as_secs() < 3, "the paste stalled");
                std::thread::yield_now();
            };
            assert_eq!(origin, expected);
            assert_eq!(rotation, turns as u8);
            let placed = schematic
                .placed_cells(IVec3::from_array(origin), rotation)
                .unwrap();
            for (local, data) in &scene.cells {
                assert!(placed.iter().any(|(p, d)| *p
                    == *local - scene.origin + IVec3::from_array(expected)
                    && d == data));
            }
            assert!(game.rotate_schematic_preview());
            assert_eq!(game.schematic_preview.vertical_offset(), height);
        }
        game.cam.yaw = std::f32::consts::PI;
        game.world_tool_input(&mut GameInput::default());
        assert!(
            game.schematic_preview.origin().is_none(),
            "losing the target clears the preview position"
        );
        assert_eq!(game.schematic_preview.vertical_offset(), height);
        game.cam.yaw = 0.0;
    }
    game.cancel_world_tools();
    assert_eq!(game.schematic_preview.vertical_offset(), 0);
    assert!(!game.raise_schematic_preview(1));
}
