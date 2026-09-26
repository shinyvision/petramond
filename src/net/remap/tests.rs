use super::*;
use crate::net::protocol::{
    BlockDelta, ClientToServer, ColumnPayload, ItemSlotWire, MobStateRow, PlayerAction,
    SectionBlocks, SectionBytes, SectionPayload, SectionStatesPayload, ServerToClient, TickUpdate,
    WorldEventMsg,
};
use petramond_math::math::IVec3;
use petramond_math::world_pos::WorldPos;
use petramond_world::block::ShapeState;
use petramond_world::chunk::SectionPos;

fn permuted_map() -> IdRemap {
    IdRemap::assemble(RemapTables {
        blocks: vec![0, 513, 257, 3],
        biomes: vec![0, 3, 2, 1],
        items: vec![Some(0), Some(2), Some(1)],
        mobs: vec![Some(0), None],
        sounds: vec![Some(0), None],
        effects: vec![Some(0), None],
        emitters: vec![Some(0), None],
        conditions: vec![Some(0), None],
        animators: Vec::new(),
    })
}

fn slab_state(a: u16, b: u16) -> ShapeState {
    let [a_lo, a_hi] = ShapeState::id_bytes(a);
    let [b_lo, b_hi] = ShapeState::id_bytes(b);
    ShapeState::with_ids(&[0b0111, a_lo, a_hi, b_lo, b_hi], 0b0_1010)
}

#[test]
fn matching_name_tables_build_an_identity_remap() {
    let names = local_name_tables();
    assert_eq!(names.biomes.first().map(String::as_str), Some(""));
    let map = IdRemap::build(&names);
    assert!(map.is_identity());
}

#[test]
fn whole_ids_and_state_bytes_remap_in_sections_and_tick_deltas() {
    let map = permuted_map();
    assert!(!map.is_identity());
    let mut section = ServerToClient::SectionData(Box::new(SectionPayload {
        pos: SectionPos::new(0, 0, 0),
        blocks: SectionBlocks(vec![0, 1, 2, 3].into()),
        metrics: Default::default(),
        fluid: None,
        skylight: None,
        blocklight: None,
        states: SectionStatesPayload {
            cell_states: vec![(9, slab_state(1, 2))],
            ..Default::default()
        },
    }));
    map.remap_to_client(&mut section);
    let ServerToClient::SectionData(section) = section else {
        unreachable!()
    };
    assert_eq!(&section.blocks.0[..], &[0, 513, 257, 3]);
    assert_eq!(section.states.cell_states, vec![(9, slab_state(513, 257))]);
    assert_eq!(
        section.metrics,
        petramond_world::section::Section::metrics_from_blocks(&section.blocks.0)
    );

    let mut tick = TickUpdate::new(4, 8);
    tick.push_list(vec![BlockDelta {
        pos: IVec3::new(1, 2, 3),
        block_id: 1,
        fluid: None,
        state: Some(slab_state(1, 2)),
        cell_kv: Vec::new(),
    }]);
    let mut msg = ServerToClient::Tick(Box::new(tick));
    map.remap_to_client(&mut msg);
    let ServerToClient::Tick(tick) = msg else {
        unreachable!()
    };
    let delta = &tick.block_deltas().unwrap()[0];
    assert_eq!(delta.block_id, 513);
    assert_eq!(delta.state, Some(slab_state(513, 257)));
}

#[test]
fn break_tool_uses_inverse_item_table() {
    let map = permuted_map();
    let mut msg = ClientToServer::Action(PlayerAction::BreakFinished {
        request_id: 9,
        pos: IVec3::ZERO,
        tool_item_id: Some(2),
        predicted: true,
    });
    map.remap_to_server(&mut msg);
    let ClientToServer::Action(PlayerAction::BreakFinished { tool_item_id, .. }) = msg else {
        unreachable!()
    };
    assert_eq!(tool_item_id, Some(1));
}

#[test]
fn biome_table_preserves_zero_and_maps_unknown_to_fallback() {
    let map = permuted_map();
    assert_eq!(map.biome(0), 0);
    assert_eq!(map.biome(1), 3);
    assert_eq!(map.biome(3), 1);

    let mut msg = ServerToClient::ColumnData(ColumnPayload {
        pos: petramond_world::chunk::ChunkPos::new(0, 0),
        biomes: SectionBytes(vec![0, 1, 2, 3].into()),
        mesh_biomes: SectionBytes(vec![3, 2, 1, 0].into()),
        surface_heightmap: Vec::new(),
        sky_cover: Vec::new(),
        summaries: Vec::new(),
        deep_band_lo: 0,
    });
    map.remap_to_client(&mut msg);
    let ServerToClient::ColumnData(column) = msg else {
        unreachable!()
    };
    assert_eq!(&column.biomes.0[..], &[0, 3, 2, 1]);
    assert_eq!(&column.mesh_biomes.0[..], &[1, 2, 3, 0]);
}

#[test]
fn unknown_items_are_dropped_from_optional_slots() {
    let map = permuted_map();
    let mut slot = Some(ItemSlotWire {
        item_id: 99,
        count: 1,
        data: None,
    });
    slot.remap(&map);
    assert_eq!(slot, None);
}

#[test]
fn unknown_mob_and_sound_events_drop_without_losing_other_rows() {
    let map = permuted_map();
    let mob = |id, kind_id| MobStateRow {
        id,
        kind_id,
        pos: WorldPos::ZERO,
        yaw: 0.0,
        tilt: petramond_math::math::Tilt::LEVEL,
        anim_time: 0.0,
        moving: false,
        idle_anim: None,
        head_yaw: 0.0,
        head_pitch: 0.0,
        hurt_timer: 0.0,
        dead: false,
        shorn: false,
        emitters: Vec::new(),
        conditions: Vec::new(),
        anims: Vec::new(),
        ragdoll: None,
        dig: None,
        held: [None; 2],
        draw: Default::default(),
    };
    let mut tick = TickUpdate::new(1, 2);
    tick.push(crate::net::protocol::MobLane::from(vec![
        mob(10, 0),
        mob(11, 1),
    ]));
    tick.push_list(vec![
        WorldEventMsg::Sound {
            sound_id: 0,
            pos: None,
        },
        WorldEventMsg::Sound {
            sound_id: 1,
            pos: None,
        },
    ]);
    let mut msg = ServerToClient::Tick(Box::new(tick));
    map.remap_to_client(&mut msg);
    let ServerToClient::Tick(tick) = msg else {
        unreachable!()
    };
    let mobs: Vec<_> = tick.mobs().unwrap().iter().collect();
    assert_eq!(mobs.len(), 1);
    assert_eq!(mobs[0].id, 10);
    assert_eq!(tick.events().unwrap().len(), 1);
}
