use super::*;

fn place(server: &mut ServerGame, pos: IVec3, block: Block) -> Result<(), String> {
    server.place_schematic(
        0,
        &plan(data(block)),
        pos.to_array(),
        0,
        &mut TickEvents::default(),
    )
}

#[test]
fn schematic_placement_and_history_allow_overlapping_players_and_mobs() {
    let mut server = server();
    let cells = [
        IVec3::new(8, 65, 8),
        IVec3::new(10, 65, 8),
        IVec3::new(12, 65, 8),
    ];
    server.sessions[0].player.pos = WorldPos::block_center(cells[0]);
    server.add_session_for_test(crate::player::Player::new(WorldPos::block_center(cells[1])));
    server
        .world
        .spawn_mob(
            crate::mob::Mob::Sheep,
            WorldPos::block_center(cells[2]),
            0.0,
        )
        .unwrap();
    let boxes = server
        .world
        .collision_boxes_at(cells[0].x, cells[0].y - 1, cells[0].z)
        .to_vec();
    for pos in cells {
        assert!(
            server.placement_occupied_by_body(Some(0), pos, &boxes),
            "fixture must overlap an existing body"
        );
        place(&mut server, pos, Block::Stone).unwrap();
        assert_eq!(server.world.snapshot_cell(pos).unwrap().block, Block::Stone);
    }
    let mut events = TickEvents::default();
    for pos in cells.into_iter().rev() {
        server
            .apply_creative(0, CreativeAction::Undo, &mut events)
            .unwrap();
        assert_eq!(server.world.snapshot_cell(pos).unwrap().block, Block::Air);
    }
    for pos in cells {
        server
            .apply_creative(0, CreativeAction::Redo, &mut events)
            .unwrap();
        assert_eq!(server.world.snapshot_cell(pos).unwrap().block, Block::Stone);
    }
}

#[test]
fn redo_restores_overlapping_edits_in_order_and_new_placements_discard_the_branch() {
    let mut server = server();
    let pos = IVec3::new(10, 65, 8);
    let mut events = TickEvents::default();
    for block in [Block::Stone, Block::Dirt, Block::OakPlanks] {
        place(&mut server, pos, block).unwrap();
    }
    for _ in 0..2 {
        for block in [Block::Dirt, Block::Stone, Block::Air] {
            server
                .apply_creative(0, CreativeAction::Undo, &mut events)
                .unwrap();
            assert_eq!(server.world.snapshot_cell(pos).unwrap().block, block);
        }
        assert!(server
            .apply_creative(0, CreativeAction::Undo, &mut events)
            .is_err());
        for block in [Block::Stone, Block::Dirt, Block::OakPlanks] {
            server
                .apply_creative(0, CreativeAction::Redo, &mut events)
                .unwrap();
            assert_eq!(server.world.snapshot_cell(pos).unwrap().block, block);
        }
        assert!(server
            .apply_creative(0, CreativeAction::Redo, &mut events)
            .is_err());
    }
    server
        .apply_creative(0, CreativeAction::Undo, &mut events)
        .unwrap();
    assert!(place(&mut server, pos + IVec3::X * 1000, Block::Stone).is_err());
    server
        .apply_creative(0, CreativeAction::Redo, &mut events)
        .unwrap();
    server
        .apply_creative(0, CreativeAction::Undo, &mut events)
        .unwrap();
    place(&mut server, pos, Block::Stone).unwrap();
    assert!(server
        .apply_creative(0, CreativeAction::Redo, &mut events)
        .is_err());
    assert_eq!(server.world.snapshot_cell(pos).unwrap().block, Block::Stone);
    server
        .apply_creative(0, CreativeAction::Undo, &mut events)
        .unwrap();
    assert_eq!(server.world.snapshot_cell(pos).unwrap().block, Block::Dirt);
}

#[test]
fn history_restores_original_snapshots_despite_changed_blocks_and_inventories() {
    let mut server = server();
    let pos = IVec3::new(10, 65, 8);
    let mut chest = data(Block::Chest);
    chest.container = Some(Container {
        slots: vec![Some(ItemStack::new(ItemType::Stone, 11))],
    });
    chest.state = EntityFront(Facing::North).to_cell();
    chest.kv.insert("fixture:lock".into(), vec![1, 2, 3]);
    server
        .world
        .apply_cells(vec![(pos, chest.clone())], CellPolicy::default())
        .unwrap();
    let mut pasted = chest.clone();
    pasted.container.as_mut().unwrap().slots[0] = Some(ItemStack::new(ItemType::Dirt, 7));
    pasted.state = EntityFront(Facing::West).to_cell();
    pasted.kv.insert("fixture:lock".into(), vec![4, 5, 6]);
    let mut events = TickEvents::default();
    server
        .place_schematic(0, &plan(pasted.clone()), pos.to_array(), 0, &mut events)
        .unwrap();
    let mut changed = chest.clone();
    changed.container.as_mut().unwrap().slots[0]
        .as_mut()
        .unwrap()
        .count -= 1;
    changed.kv.clear();
    changed.state = EntityFront(Facing::South).to_cell();
    for changed in [changed, data(Block::Grass), data(Block::Air)] {
        for (action, expected) in [
            (CreativeAction::Undo, &chest),
            (CreativeAction::Redo, &pasted),
        ] {
            server
                .world
                .apply_cells(vec![(pos, changed.clone())], CellPolicy::default())
                .unwrap();
            server.apply_creative(0, action, &mut events).unwrap();
            assert_eq!(&server.world.snapshot_cell(pos).unwrap(), expected);
        }
    }
    // A player moving into the footprint must not invalidate history either.
    server.sessions[0].player.pos = WorldPos::block_center(pos);
    server
        .apply_creative(0, CreativeAction::Undo, &mut events)
        .unwrap();
    assert_eq!(server.world.snapshot_cell(pos).unwrap(), chest);
    server
        .apply_creative(0, CreativeAction::Redo, &mut events)
        .unwrap();
    assert_eq!(server.world.snapshot_cell(pos).unwrap(), pasted);
}

#[test]
fn redo_after_grass_spreads_and_undo_after_replacement_notify_the_actual_removed_blocks() {
    use std::sync::{Arc, Mutex};
    let mut server = server();
    let pos = IVec3::new(10, 65, 8);
    server
        .world
        .apply_cells(vec![(pos, data(Block::Dirt))], CellPolicy::default())
        .unwrap();
    place(&mut server, pos, Block::OakPlanks).unwrap();
    let mut events = TickEvents::default();
    server
        .apply_creative(0, CreativeAction::Undo, &mut events)
        .unwrap();
    server
        .world
        .set_block_world(pos.x, pos.y, pos.z, Block::Grass);
    let broken = Arc::new(Mutex::new(Vec::new()));
    let observed = Arc::clone(&broken);
    server.bus.on_post(
        crate::events::PostEventKind::BlockBroken,
        0,
        move |_, event| {
            if let PostEvent::BlockBroken { pos, block, .. } = event {
                observed.lock().unwrap().push((*pos, *block));
            }
        },
    );
    server
        .apply_creative(0, CreativeAction::Redo, &mut events)
        .unwrap();
    assert_eq!(
        server.world.snapshot_cell(pos).unwrap().block,
        Block::OakPlanks
    );
    assert_eq!(*broken.lock().unwrap(), vec![(pos, Block::Grass)]);
    broken.lock().unwrap().clear();
    server
        .world
        .set_block_world(pos.x, pos.y, pos.z, Block::Stone);
    server
        .apply_creative(0, CreativeAction::Undo, &mut events)
        .unwrap();
    assert_eq!(server.world.snapshot_cell(pos).unwrap().block, Block::Dirt);
    assert_eq!(*broken.lock().unwrap(), vec![(pos, Block::Stone)]);
}

#[test]
fn history_cleans_up_compound_blocks_added_since_the_recorded_edit() {
    let mut server = server();
    let pos = IVec3::new(10, 65, 8);
    place(&mut server, pos, Block::Stone).unwrap();
    for (action, expected) in [
        (CreativeAction::Undo, Block::Air),
        (CreativeAction::Redo, Block::Stone),
    ] {
        let cells: Vec<_> = [false, true]
            .into_iter()
            .map(|top| {
                let mut door = data(Block::OakDoor);
                door.state = petramond_world::door::DoorState {
                    top,
                    ..Default::default()
                }
                .to_cell();
                (pos + IVec3::Y * i32::from(top), door)
            })
            .collect();
        server
            .world
            .apply_cells(cells, CellPolicy::default())
            .unwrap();
        server
            .apply_creative(0, action, &mut TickEvents::default())
            .unwrap();
        assert_eq!(server.world.snapshot_cell(pos).unwrap().block, expected);
        assert_eq!(
            server.world.snapshot_cell(pos + IVec3::Y).unwrap().block,
            Block::Air
        );
    }
}

#[test]
fn placement_history_stays_bounded_when_records_move_between_undo_and_redo() {
    let mut server = server();
    let pos = IVec3::new(10, 65, 8);
    for i in 0..20 {
        place(
            &mut server,
            pos,
            if i % 2 == 0 {
                Block::Stone
            } else {
                Block::Dirt
            },
        )
        .unwrap();
    }
    let retained = server.sessions[0].edits.undo_len();
    assert!(retained < 20);
    let mut events = TickEvents::default();
    for _ in 0..2 {
        let mut count = 0;
        while server
            .apply_creative(0, CreativeAction::Undo, &mut events)
            .is_ok()
        {
            count += 1;
        }
        assert_eq!(count, retained);
        let mut count = 0;
        while server
            .apply_creative(0, CreativeAction::Redo, &mut events)
            .is_ok()
        {
            count += 1;
        }
        assert_eq!(count, retained);
        assert_eq!(server.world.snapshot_cell(pos).unwrap().block, Block::Dirt);
    }
}

#[test]
fn a_build_beyond_the_old_save_limits_places_and_restores_as_one_history_entry() {
    let mut server = server();
    server.world.clear_world();
    for cx in 0..20 {
        server
            .world
            .insert_empty_column_for_test(petramond_world::chunk::ChunkPos::new(cx, 0));
    }
    let origin = IVec3::new(0, 64, 0);
    server.sessions[0].player.pos = WorldPos::new(8.0, 85.0, 160.0);
    let stone = CellData::capture(&data(Block::Stone));
    let schematic = Schematic::from_cells(
        "Large paste".into(),
        [320, 16, 16],
        (0..320)
            .flat_map(|x| (0..16).flat_map(move |y| (0..16).map(move |z| [x, y, z])))
            .map(|pos| SchematicCell {
                pos,
                data: stone.clone(),
            }),
    )
    .unwrap();
    let mut events = TickEvents::default();
    server
        .place_schematic(0, &schematic, origin.to_array(), 1, &mut events)
        .unwrap_err();
    // The rotated footprint leaves loaded terrain; no section may be partially pasted.
    assert_eq!(
        server.world.snapshot_cell(origin).unwrap().block,
        Block::Air
    );
    server.sessions[0].player.pos = WorldPos::new(160.0, 85.0, 8.0);
    server
        .place_schematic(0, &schematic, origin.to_array(), 0, &mut events)
        .unwrap();
    assert!(
        server.sessions[0].creative.job.is_some(),
        "a large edit spreads over ticks"
    );
    assert_eq!(server.sessions[0].edits.undo_len(), 0);
    finish_edit(&mut server);
    assert_eq!(server.sessions[0].edits.undo_len(), 1);
    for p in schematic.cells().map(|c| IVec3::from_array(c.pos) + origin) {
        assert_eq!(server.world.snapshot_cell(p).unwrap().block, Block::Stone);
    }
    server
        .apply_creative(0, CreativeAction::Undo, &mut events)
        .unwrap();
    finish_edit(&mut server);
    for p in schematic.cells().map(|c| IVec3::from_array(c.pos) + origin) {
        assert_eq!(server.world.snapshot_cell(p).unwrap().block, Block::Air);
    }
    server.world.set_block_world(16, 64, 0, Block::Dirt);
    server
        .apply_creative(0, CreativeAction::Redo, &mut events)
        .unwrap();
    finish_edit(&mut server);
    for p in schematic.cells().map(|c| IVec3::from_array(c.pos) + origin) {
        assert_eq!(server.world.snapshot_cell(p).unwrap().block, Block::Stone);
    }
    assert_eq!(server.sessions[0].edits.undo_len(), 1);
}
