use super::*;
use crate::events::PostEvent;
use crate::schematic::{CellData, CreativeReply, ResolvedCell, Schematic, SchematicCell};
use crate::world::cells::CellPolicy;
use petramond_math::math::IVec3;
use petramond_math::{facing::Facing, world_pos::WorldPos};
use petramond_world::block::Block;
use petramond_world::{
    block::{CellCodec, ShapeState},
    block_state::EntityFront,
    container::Container,
    item::{ItemStack, ItemType},
};

mod applicator;
mod capture;
mod history;
mod updates;

fn server() -> ServerGame {
    let mut server = crate::server::session_build::build_server_inline("", 1, 1);
    server.sessions[0].player.pos = WorldPos::new(8.5, 65.0, 8.5);
    server.sessions[0]
        .player
        .set_mode(crate::player::PlayerMode::Creative);
    for x in 7..=12 {
        for y in 64..=69 {
            for z in 7..=12 {
                server.world.set_block_world(
                    x,
                    y,
                    z,
                    if y == 64 { Block::Stone } else { Block::Air },
                );
            }
        }
    }
    server
}
/// Run the session's multi-tick edit to completion.
fn finish_edit(server: &mut ServerGame) {
    let mut events = TickEvents::default();
    while server.sessions[0].creative.job.is_some() {
        server.tick_creative(0, &mut events);
    }
}
fn data(block: Block) -> ResolvedCell {
    ResolvedCell {
        block,
        state: ShapeState::NONE,
        fluid: 0,
        kv: Default::default(),
        container: None,
        furnace: None,
    }
}
fn plan(cell: ResolvedCell) -> Schematic {
    Schematic::from_cells(
        "Chest copy".into(),
        [1; 3],
        vec![SchematicCell {
            pos: [0; 3],
            data: CellData::capture(&cell),
        }],
    )
    .unwrap()
}

#[test]
fn paste_copies_chest_data_and_undo_restores_the_overwritten_cell() {
    let mut server = server();
    let pos = IVec3::new(10, 65, 8);
    server
        .world
        .set_block_world(pos.x, pos.y, pos.z, Block::Dirt);
    let before = server.world.snapshot_cell(pos).unwrap();
    let mut chest = data(Block::Chest);
    chest.state = EntityFront(Facing::North).to_cell();
    chest.kv.insert("fixture:lock".into(), vec![5, 4, 3]);
    chest.container = Some(Container {
        slots: vec![Some(ItemStack::new(ItemType::Stone, 11)), None],
    });
    let mut events = TickEvents::default();
    server
        .place_schematic(0, &plan(chest.clone()), pos.to_array(), 1, &mut events)
        .unwrap();
    let pasted = server.world.snapshot_cell(pos).unwrap();
    assert_eq!(pasted.container, chest.container);
    assert_eq!(pasted.kv, chest.kv);
    assert_ne!(pasted.state, chest.state);
    for _ in 0..2 {
        server
            .apply_creative(0, CreativeAction::Undo, &mut events)
            .unwrap();
        assert_eq!(server.world.snapshot_cell(pos).unwrap(), before);
        server
            .apply_creative(0, CreativeAction::Redo, &mut events)
            .unwrap();
        assert_eq!(server.world.snapshot_cell(pos).unwrap(), pasted);
    }
    assert_eq!(
        crate::world::schematic::capture::Capture::new(
            &server.world,
            "Copy".into(),
            vec![crate::schematic::SelectionBox::between(pos.to_array(), pos.to_array()).unwrap()],
            false,
            false,
        )
        .unwrap()
        .run()
        .unwrap()
        .cells()
        .next()
        .unwrap()
        .data
        .clone(),
        CellData::capture(&pasted)
    );
    server
        .apply_creative(0, CreativeAction::Undo, &mut events)
        .unwrap();
    assert_eq!(server.world.snapshot_cell(pos).unwrap(), before);
    assert!(server
        .apply_creative(0, CreativeAction::Undo, &mut events)
        .is_err());
}

#[test]
fn invalid_or_unloaded_paste_has_no_partial_writes() {
    let mut server = server();
    let pos = IVec3::new(10, 65, 8);
    let before = server.world.snapshot_cell(pos).unwrap();
    let mut schematic = plan(data(Block::Stone));
    schematic.size = [2, 1, 1];
    let mut missing = schematic.cells().next().unwrap().to_owned();
    missing.pos = [1, 0, 0];
    missing.data.block = "absent:block".into();
    schematic = Schematic::from_cells(
        schematic.name.clone(),
        schematic.size,
        schematic.cells().map(|c| c.to_owned()).chain([missing]),
    )
    .unwrap();
    assert!(server
        .place_schematic(0, &schematic, pos.to_array(), 0, &mut TickEvents::default())
        .is_err());
    assert_eq!(server.world.snapshot_cell(pos).unwrap(), before);
    assert!(server
        .world
        .apply_cells(
            vec![
                (pos, data(Block::Stone)),
                (IVec3::new(10000, 65, 10000), data(Block::Stone)),
            ],
            CellPolicy::default(),
        )
        .is_err());
    assert_eq!(server.world.snapshot_cell(pos).unwrap(), before);
}

#[test]
fn survival_cannot_use_creative_history_actions() {
    let mut server = server();
    server.sessions[0]
        .player
        .set_mode(crate::player::PlayerMode::Survival);
    let before = server.sessions[0].player.inventory.selected().copied();
    for action in [CreativeAction::Undo, CreativeAction::Redo] {
        server.sessions[0]
            .creative
            .pending
            .push_back(Pending::Action(action));
        server.tick_creative(0, &mut TickEvents::default());
    }
    assert_eq!(
        server.sessions[0].player.inventory.selected().copied(),
        before
    );
    assert!(matches!(
        server.sessions[0].creative.replies.last(),
        Some(CreativeReply::Message(_))
    ));
    assert_eq!(server.sessions[0].creative.replies.len(), 2);
}

#[test]
fn instant_mining_keeps_a_repeat_delay_and_creative_placement_keeps_the_stack() {
    let mut server = server();
    let a = IVec3::new(10, 65, 8);
    let b = a + IVec3::Z;
    for p in [a, b] {
        server.world.set_block_world(p.x, p.y, p.z, Block::Stone);
    }
    let mut events = TickEvents::default();
    let request = |id, pos| super::super::player::PendingBreakFinished {
        request_id: id,
        pos,
        tool_item_id: None,
        predicted: false,
    };
    server.sessions[0]
        .pending_break_finished
        .extend([request(1, a), request(2, b)]);
    server.tick_mining(0, &mut events);
    assert_eq!(server.world.chunk_block(a.x, a.y, a.z), Block::Air.id());
    assert_eq!(server.world.chunk_block(b.x, b.y, b.z), Block::Stone.id());
    server.world.restore_tick(server.world.current_tick() + 10);
    server.sessions[0]
        .pending_break_finished
        .push(request(3, b));
    server.tick_mining(0, &mut events);
    assert_eq!(server.world.chunk_block(b.x, b.y, b.z), Block::Air.id());
    *server.sessions[0].player.inventory.slot_mut(0).unwrap() = Some(ItemStack::new(
        ItemType::Stone,
        ItemType::Stone.max_stack_size(),
    ));
    let count = server.sessions[0]
        .player
        .inventory
        .selected()
        .unwrap()
        .count;
    let placed = server.try_place(
        0,
        Some(crate::net::protocol::TargetRef::face(
            a - IVec3::Y,
            IVec3::Y,
        )),
        &mut events,
    );
    assert_eq!(placed, Some(a));
    assert_eq!(
        server.sessions[0]
            .player
            .inventory
            .selected()
            .unwrap()
            .count,
        count
    );
}

#[test]
fn copied_connection_state_survives_the_bulk_commit() {
    let mut server = server();
    let pos = IVec3::new(10, 65, 8);
    let fence = *Block::all()
        .iter()
        .find(|b| b.shape_family() == petramond_world::block::ShapeFamily::Fence)
        .unwrap();
    let mut d = data(fence);
    d.state = petramond_world::connect::ConnectionMask(
        petramond_world::connect::NORTH | petramond_world::connect::SOUTH,
    )
    .to_cell();
    server
        .world
        .apply_cells(vec![(pos, d.clone())], CellPolicy::default())
        .unwrap();
    assert_eq!(server.world.snapshot_cell(pos).unwrap().state, d.state);
}

#[test]
fn schematic_placement_reaches_distant_loaded_terrain() {
    let mut server = server();
    let pos = IVec3::new(108, 65, 8);
    server
        .world
        .insert_empty_column_for_test(petramond_world::chunk::ChunkPos::new(
            pos.x.div_euclid(16),
            0,
        ));
    server
        .world
        .set_block_world(pos.x, pos.y, pos.z, Block::Air);
    server
        .place_schematic(
            0,
            &plan(data(Block::Stone)),
            pos.to_array(),
            0,
            &mut TickEvents::default(),
        )
        .unwrap();
    assert_eq!(server.world.snapshot_cell(pos).unwrap().block, Block::Stone);
    let beyond = IVec3::new(160, 65, 8);
    server
        .world
        .insert_empty_column_for_test(petramond_world::chunk::ChunkPos::new(
            beyond.x.div_euclid(16),
            0,
        ));
    server
        .world
        .set_block_world(beyond.x, beyond.y, beyond.z, Block::Air);
    assert!(server
        .place_schematic(
            0,
            &plan(data(Block::Stone)),
            beyond.to_array(),
            0,
            &mut TickEvents::default()
        )
        .is_err());
    assert_eq!(
        server.world.snapshot_cell(beyond).unwrap().block,
        Block::Air
    );
}

#[test]
fn creative_catalog_pickup_requires_a_creative_menu_and_authorized_player() {
    let mut server = server();
    let mut events = TickEvents::default();
    let pick = || crate::server::player::PendingMenuAction::CreativeCursor {
        item: Some(ItemType::Stone.registry_name().into()),
        request_id: 1,
    };
    server.sessions[0].pending_menu_actions.push(pick());
    server.tick_menu(0, &mut events);
    assert!(server.sessions[0].player.inventory.cursor().is_none());
    server.sessions[0].pending_menu_actions.push(
        crate::server::player::PendingMenuAction::OpenGui {
            kind: petramond_world::gui_state::GuiKind::Creative,
            anchor: None,
        },
    );
    server.sessions[0].pending_menu_actions.push(pick());
    server.tick_menu(0, &mut events);
    assert_eq!(
        server.sessions[0].player.inventory.cursor().unwrap().item,
        ItemType::Stone
    );
    let discard = || crate::server::player::PendingMenuAction::CreativeCursor {
        item: None,
        request_id: 2,
    };
    server.sessions[0].pending_menu_actions.push(discard());
    server.tick_menu(0, &mut events);
    assert!(server.sessions[0].player.inventory.cursor().is_none());
    server.sessions[0]
        .player
        .set_mode(crate::player::PlayerMode::Survival);
    server.sessions[0].pending_menu_actions.push(pick());
    server.tick_menu(0, &mut events);
    assert!(server.sessions[0].player.inventory.cursor().is_none());
    let held = ItemStack::new(ItemType::Dirt, 7);
    *server.sessions[0].player.inventory.cursor_mut() = Some(held);
    server.sessions[0].pending_menu_actions.push(discard());
    server.tick_menu(0, &mut events);
    assert_eq!(server.sessions[0].player.inventory.cursor(), Some(&held));
}

#[test]
fn grouped_placement_variants_vary_and_replay_without_consuming_the_creative_stack() {
    let mut server = server();
    let item = ItemType::all()
        .iter()
        .copied()
        .find(|i| i.creative_only() && i.placement_variants().len() > 1)
        .unwrap();
    *server.sessions[0].player.inventory.slot_mut(0).unwrap() = Some(ItemStack::new(item, 1));
    let p = IVec3::new(10, 65, 8);
    let mut samples = Vec::new();
    for _ in 0..2 {
        let mut pass = Vec::new();
        for tick in 0..32 {
            server.world.restore_tick(tick);
            server.world.set_block_world(p.x, p.y, p.z, Block::Air);
            assert!(server
                .try_place(
                    0,
                    Some(crate::net::protocol::TargetRef::face(
                        p - IVec3::Y,
                        IVec3::Y
                    )),
                    &mut TickEvents::default()
                )
                .is_some());
            let block = Block::from_id(server.world.chunk_block(p.x, p.y, p.z));
            assert!(item.placement_variants().contains(&block));
            pass.push(block);
        }
        samples.push(pass);
    }
    assert_eq!(samples[0], samples[1]);
    assert!(samples[0].iter().any(|b| *b != samples[0][0]));
    assert_eq!(
        server.sessions[0]
            .player
            .inventory
            .selected()
            .unwrap()
            .count,
        1
    );
}

#[test]
fn a_centered_large_schematic_uses_its_rotated_pivot_for_placement_reach() {
    let mut server = server();
    let mut schematic = plan(data(Block::Stone));
    schematic.size = [255, 1, 251];
    let mut other = schematic.cells().next().unwrap().to_owned();
    other.pos = [254, 0, 250];
    schematic = Schematic::from_cells(
        schematic.name.clone(),
        schematic.size,
        schematic.cells().map(|c| c.to_owned()).chain([other]),
    )
    .unwrap();
    for turns in 0..4 {
        let origin = IVec3::new(8, 68, 8) - schematic.placement_pivot(turns);
        assert!(
            (WorldPos::block_center(origin) - server.sessions[0].player.pos).length()
                > crate::schematic::PLACEMENT_REACH
        );
        let cells = schematic.placed_cells(origin, turns).unwrap();
        for (p, _) in &cells {
            server
                .world
                .insert_empty_column_for_test(petramond_world::chunk::ChunkPos::new(
                    p.x.div_euclid(16),
                    p.z.div_euclid(16),
                ));
        }
        server
            .place_schematic(
                0,
                &schematic.clone(),
                origin.to_array(),
                turns,
                &mut TickEvents::default(),
            )
            .unwrap();
        for (p, data) in cells {
            assert_eq!(server.world.snapshot_cell(p).unwrap(), data);
        }
    }
}
