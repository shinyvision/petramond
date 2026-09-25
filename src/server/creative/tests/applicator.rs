use super::*;
use crate::events::Outcome;

fn break_request(id: u32, pos: IVec3) -> crate::server::player::PendingBreakFinished {
    crate::server::player::PendingBreakFinished {
        request_id: id,
        pos,
        tool_item_id: None,
        predicted: false,
    }
}

#[test]
fn single_block_edits_made_in_one_tick_undo_as_one_edit() {
    let mut server = server();
    let placed = IVec3::new(10, 65, 8);
    let broken = IVec3::new(8, 64, 10);
    *server.sessions[0].player.inventory.slot_mut(0).unwrap() =
        Some(ItemStack::new(ItemType::Stone, 1));
    let mut events = TickEvents::default();
    server.sessions[0]
        .pending_break_finished
        .push(break_request(1, broken));
    server.tick_mining(0, &mut events);
    server
        .try_place(
            0,
            Some(crate::net::protocol::TargetRef::face(
                placed - IVec3::Y,
                IVec3::Y,
            )),
            &mut events,
        )
        .unwrap();
    server.world.restore_tick(server.world.current_tick() + 1);
    server.tick_mining(0, &mut events);
    assert_eq!(server.sessions[0].edits.undo_len(), 1);

    server
        .apply_creative(0, CreativeAction::Undo, &mut events)
        .unwrap();
    assert_eq!(
        server.world.snapshot_cell(placed).unwrap().block,
        Block::Air
    );
    assert_eq!(
        server.world.snapshot_cell(broken).unwrap().block,
        Block::Stone
    );
    server
        .apply_creative(0, CreativeAction::Redo, &mut events)
        .unwrap();
    assert_eq!(
        server.world.snapshot_cell(placed).unwrap().block,
        Block::Stone
    );
    assert_eq!(
        server.world.snapshot_cell(broken).unwrap().block,
        Block::Air
    );
}

#[test]
fn a_listener_that_reinitialises_placed_blocks_cannot_replace_copied_instance_data() {
    let mut server = server();
    let pos = IVec3::new(10, 65, 8);
    server.bus.on_post(
        crate::events::PostEventKind::BlockPlaced,
        0,
        |ctx, event| {
            if let PostEvent::BlockPlaced { pos, .. } = event {
                ctx.world
                    .cell_kv_set(pos.x, pos.y, pos.z, "fixture:lock".into(), vec![0]);
            }
        },
    );
    let mut chest = data(Block::Chest);
    chest.state = EntityFront(Facing::North).to_cell();
    chest.kv.insert("fixture:lock".into(), vec![5, 4, 3]);
    server
        .place_schematic(
            0,
            &plan(chest.clone()),
            pos.to_array(),
            0,
            &mut TickEvents::default(),
        )
        .unwrap();
    assert_eq!(server.world.snapshot_cell(pos).unwrap(), chest);
}

#[test]
fn a_cancelled_pre_event_refuses_the_whole_edit() {
    let mut server = server();
    let pos = IVec3::new(10, 65, 8);
    server.bus.on_cells_edit_pre(0, move |_, edit| {
        assert_eq!((edit.min, edit.max, edit.cells), (pos, pos, 1));
        Outcome::Cancel
    });
    assert!(server
        .place_schematic(
            0,
            &plan(data(Block::Stone)),
            pos.to_array(),
            0,
            &mut TickEvents::default()
        )
        .is_err());
    assert_eq!(server.world.snapshot_cell(pos).unwrap().block, Block::Air);
    assert_eq!(server.sessions[0].edits.undo_len(), 0);
}

#[test]
fn a_budgeted_slice_never_splits_a_written_or_an_overwritten_compound() {
    let mut server = server();
    let pos = IVec3::new(10, 65, 8);
    let door = |at: IVec3| -> Vec<(IVec3, ResolvedCell)> {
        [false, true]
            .into_iter()
            .map(|top| {
                let mut half = data(Block::OakDoor);
                half.state = petramond_world::door::DoorState {
                    top,
                    ..Default::default()
                }
                .to_cell();
                (at + IVec3::Y * i32::from(top), half)
            })
            .collect()
    };
    // An old door whose upper half lies outside the new edit.
    server
        .world
        .apply_cells(door(pos + IVec3::X), CellPolicy::default())
        .unwrap();
    let mut target = door(pos);
    target.insert(1, (pos + IVec3::X, data(Block::Stone)));
    let mut edit = server
        .world
        .begin_cells(
            target,
            CellPolicy {
                record: true,
                update_bounds: None,
            },
        )
        .unwrap_or_else(|(_, e)| panic!("{e}"));
    let block = |server: &ServerGame, p: IVec3| server.world.snapshot_cell(p).unwrap().block;
    assert!(!server
        .world
        .step_cells(&mut edit, 1, &mut |_, _| {})
        .unwrap());
    assert_eq!(block(&server, pos), Block::OakDoor);
    assert_eq!(block(&server, pos + IVec3::Y), Block::OakDoor);
    assert_eq!(block(&server, pos + IVec3::X), Block::OakDoor);
    assert!(server
        .world
        .step_cells(&mut edit, 1, &mut |_, _| {})
        .unwrap());
    assert_eq!(block(&server, pos + IVec3::X), Block::Stone);
    assert_eq!(block(&server, pos + IVec3::X + IVec3::Y), Block::Air);
    let receipt = edit.finish();
    assert_eq!(receipt.cleared.len(), 1);
    assert_eq!(receipt.before.len(), 4);
}
