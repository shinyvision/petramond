use crate::world::remote::payload::SectionPayloadExt;
use crate::world::WorldData;
use std::sync::Arc;

use petramond_world::block::Block;
use petramond_world::block_state::{LogAxis, SlabSplit, StairHalf, StairState};
use petramond_world::chunk::{Chunk, ChunkPos, SectionPos, CHUNK_SX, CHUNK_SZ};
use petramond_math::facing::Facing;
use petramond_math::math::IVec3;
use crate::net::protocol::BlockDelta;
use petramond_world::section::Section;
use petramond_world::slab::SlabSlot;
use petramond_world::torch::TorchPlacement;
use crate::worker::JobPool;
use crate::world::store::{LoadTarget, World, WorldRole};

/// A flat-floored source world (Combined runs the same content paths the
/// headless server will) and a fresh replica, sharing ONE job pool — the
/// in-process (singleplayer / listen-host) topology.
fn server_and_replica() -> (World, World) {
    let pool = Arc::new(JobPool::new(2));
    let mut server = World::new_with_pool(0, 1, WorldRole::Combined, pool.clone());
    for cz in -1..=1 {
        for cx in -1..=1 {
            let mut c = Chunk::new(cx, cz);
            for z in 0..CHUNK_SZ {
                for x in 0..CHUNK_SX {
                    c.set_block(x, 64, z, Block::Stone);
                }
            }
            server.insert_chunk_for_test(ChunkPos::new(cx, cz), c);
        }
    }
    let replica = World::new_with_pool(0, 1, WorldRole::ClientReplica, pool);
    (server, replica)
}

/// The replica convergence contract: everything a client can SEE —
/// A furnace lighting (or going out) after join must flip the replica's
/// front texture. Lit is the block ID (`furnace` ⇄ `furnace_lit`), so the
/// flip rides an ordinary block delta — no bespoke lit lane — and the front
/// FACING survives the swap through the delta's cell state.
#[test]
fn furnace_lit_flip_reaches_the_replica_through_a_delta() {
    let (mut server, mut replica) = server_and_replica();
    let pos = IVec3::new(4, 65, 4);
    assert!(server.set_block_world(pos.x, pos.y, pos.z, Block::Furnace));
    server.insert_furnace(pos, Facing::South);

    for cp in server.columns.keys().copied().collect::<Vec<_>>() {
        replica.install_remote_column(server.column_payload(cp).unwrap());
    }
    for s in server.sections.values() {
        replica.install_remote_section(s.to_payload());
    }
    let replica_block = |r: &World| Block::from_id(r.chunk_block(4, 65, 4));
    assert_eq!(
        replica_block(&replica),
        Block::Furnace,
        "fixture: joins unlit"
    );

    // The furnace lights; `tick_furnaces` swaps the skin row through the
    // block-write lanes, which record the delta.
    server.set_replication_capture(true);
    {
        let (furnace, _) = server.furnace_parts_mut(pos).unwrap();
        furnace.burn_remaining = 50;
        furnace.burn_max = 100;
    }
    server.game_tick(&petramond_world::crafting::Recipes::default());
    assert_eq!(
        Block::from_id(server.chunk_block(4, 65, 4)),
        Block::FurnaceLit,
        "fixture: the server swapped the lit row"
    );
    for d in server.take_block_deltas() {
        replica.apply_remote_delta(d);
    }
    assert_eq!(
        replica_block(&replica),
        Block::FurnaceLit,
        "the lit flip must reach the replica's mesher"
    );
    assert_eq!(
        replica
            .section_at_world_for_test(4, 65, 4)
            .expect("furnace section")
            .entity_facing(4, 1, 4),
        Facing::South,
        "the front facing survives the row swap on the wire"
    );

    // ...and the flame going out flips it back.
    {
        let (furnace, _) = server.furnace_parts_mut(pos).unwrap();
        furnace.burn_remaining = 1;
    }
    server.game_tick(&petramond_world::crafting::Recipes::default());
    for d in server.take_block_deltas() {
        replica.apply_remote_delta(d);
    }
    assert_eq!(
        replica_block(&replica),
        Block::Furnace,
        "the extinguish must reach it too"
    );
}

#[test]
fn column_payload_keeps_visible_glass_separate_from_sky_cover() {
    let (mut server, mut replica) = server_and_replica();
    let cp = ChunkPos::new(0, 0);

    assert!(server.set_block_world(8, 80, 8, Block::Glass));
    replica.install_remote_column(server.column_payload(cp).unwrap());

    let column = &replica.columns[&cp];
    assert_eq!(column.surface_y(8, 8), 80);
    assert_eq!(
        column.sky_cover_y(8, 8),
        64,
        "replication must not collapse the two maps back into one"
    );
}

/// block ids, water meta, and every sparse state map — reads back
/// identically through the public query surface after installing the
/// column payloads, the section payloads, and a tick's coalesced deltas.
#[test]
fn replica_converges_on_payloads_and_deltas() {
    let (mut server, mut replica) = server_and_replica();

    // One of each replicated state, through the normal edit funnels.
    assert!(server.set_block_world(2, 65, 2, Block::Stone));
    assert!(server.cell_kv_set(2, 65, 2, "testmod:heat".into(), vec![7, 1]));
    assert!(server.set_water_world(IVec3::new(3, 65, 3), Block::Water, 0)); // source
    assert!(server.set_water_world(IVec3::new(4, 65, 3), Block::Water, 0x83)); // falling
    assert!(server.place_door(IVec3::new(5, 65, 5), Block::OakDoor, Facing::East));
    assert!(server.place_stair(
        IVec3::new(6, 65, 6),
        Block::OakStairs,
        StairState::new(Facing::South, StairHalf::Top),
    ));
    assert!(server.set_block_world(7, 65, 7, Block::Torch));
    server.insert_torch(IVec3::new(7, 65, 7), TorchPlacement::East);
    assert!(server.place_log(IVec3::new(1, 65, 6), Block::OakLog, LogAxis::X));
    assert!(server.set_block_world(2, 65, 6, Block::OakSapling));
    assert!(server.set_block_world(1, 65, 1, Block::Chest));
    server.insert_chest(IVec3::new(1, 65, 1), Facing::West);
    // A BURNING furnace: the lit skin is the block id (`furnace_lit` row),
    // installed by the tick's row swap — written directly here.
    assert!(server.set_block_world(4, 65, 4, Block::FurnaceLit));
    server.insert_furnace(IVec3::new(4, 65, 4), Facing::South);
    {
        let (furnace, _) = server.furnace_parts_mut(IVec3::new(4, 65, 4)).unwrap();
        furnace.burn_remaining = 50;
        furnace.burn_max = 100;
    }
    assert!(server.place_slab_layer(
        IVec3::new(6, 65, 1),
        Block::CobblestoneSlab,
        SlabSlot {
            split: SlabSplit::Y,
            index: 0,
        },
    ));
    assert!(server.place_model_block_facing(
        IVec3::new(10, 65, 10),
        Block::FurnitureWorkbench,
        Facing::East,
    ));

    // Join-time capture: columns + sections. One deep all-stone section is
    // deliberately withheld so the summaries have to answer for it.
    let held_back = SectionPos::new(0, -2, 0);
    assert!(
        server.sections.contains_key(&held_back),
        "fixture: deep stone loaded"
    );
    let columns: Vec<_> = server
        .columns
        .keys()
        .copied()
        .map(|cp| server.column_payload(cp).expect("column loaded"))
        .collect();
    let sections: Vec<_> = server
        .sections
        .iter()
        .filter(|(sp, _)| **sp != held_back)
        .map(|(_, s)| s.to_payload())
        .collect();

    // Post-join edits ride the delta log.
    server.set_replication_capture(true);
    assert!(server.set_block_world(8, 65, 8, Block::Dirt));
    assert!(server.set_water_world(IVec3::new(9, 65, 9), Block::Water, 0x05));
    let deltas = server.take_block_deltas();
    assert!(!deltas.is_empty());

    for c in columns {
        replica.install_remote_column(c);
    }
    for s in sections {
        replica.install_remote_section(s);
    }
    for d in &deltas {
        replica.apply_remote_delta(d.clone());
    }
    // A delta for a section nobody installed drops silently.
    replica.apply_remote_delta(BlockDelta {
        pos: IVec3::new(200, 65, 200),
        block_id: Block::Stone.id(),
        water: None,
        state: None,
        cell_kv: vec![],
    });
    assert_eq!(replica.chunk_block(200, 65, 200), 0);

    // Raw content converges (blocks + water meta) at every touched cell.
    for (x, y, z) in [
        (2, 65, 2),
        (3, 65, 3),
        (4, 65, 3),
        (5, 65, 5),
        (5, 66, 5),
        (6, 65, 6),
        (7, 65, 7),
        (1, 65, 6),
        (2, 65, 6),
        (1, 65, 1),
        (4, 65, 4),
        (6, 65, 1),
        (8, 65, 8),
        (9, 65, 9),
        (10, 65, 10),
    ] {
        assert_eq!(
            replica.chunk_block(x, y, z),
            server.chunk_block(x, y, z),
            "block id diverged at ({x},{y},{z})"
        );
        assert_eq!(
            replica.water_meta_world(x, y, z),
            server.water_meta_world(x, y, z),
            "water meta diverged at ({x},{y},{z})"
        );
    }
    assert!(replica.is_water_source_world(IVec3::new(3, 65, 3)));

    // Every state map reads back through the public query surface.
    assert_eq!(
        replica.cell_kv_get(2, 65, 2, "testmod:heat"),
        Some(&[7u8, 1][..])
    );
    assert_eq!(
        replica.door_state_at(5, 65, 5),
        server.door_state_at(5, 65, 5)
    );
    assert_eq!(
        replica.door_state_at(5, 66, 5),
        server.door_state_at(5, 66, 5)
    );
    assert_eq!(
        replica.stair_state_at(6, 65, 6),
        server.stair_state_at(6, 65, 6)
    );
    assert_eq!(
        replica.torch_placement(IVec3::new(7, 65, 7)),
        TorchPlacement::East
    );
    assert_eq!(replica.log_axis_at(1, 65, 6), LogAxis::X);
    assert_eq!(
        replica.slab_state_at(6, 65, 1),
        server.slab_state_at(6, 65, 1)
    );
    assert_eq!(
        replica.model_offset_at(11, 65, 10),
        server.model_offset_at(11, 65, 10)
    );
    assert_eq!(replica.model_facing_at(10, 65, 10), Facing::East);
    let mut chests = Vec::new();
    replica.collect_chests(&mut chests);
    assert!(
        chests
            .iter()
            .any(|&(p, f, ..)| p == IVec3::new(1, 65, 1) && f == Facing::West),
        "the chest renders on the replica with its facing"
    );
    assert_eq!(
        replica
            .section_at_world_for_test(4, 65, 4)
            .unwrap()
            .entity_facing(4, 1, 4),
        Facing::South
    );
    assert_eq!(
        Block::from_id(replica.chunk_block(4, 65, 4)),
        Block::FurnaceLit,
        "the lit furnace face replicates as its block row"
    );

    // Absent sections answer physics/placement from the column summaries.
    assert!(!replica.sections.contains_key(&held_back));
    assert_eq!(replica.physics_block(2, -20, 2), Block::Stone);
    assert!(!replica.placement_cell_open(IVec3::new(2, -20, 2)));

    // Authoritative light is server-owned: installs queue MESH work only. A lightless
    // payload installs light-CLEAN (the ship gate only lets one through
    // when it never bakes) — ingest must never queue a replica-side bake.
    assert!(replica.dirty_mesh_count() > 0, "installs queue mesh work");
    assert!(
        !replica
            .section_at_world_for_test(2, 65, 2)
            .unwrap()
            .light_dirty,
        "a replica install never queues a replica-side bake"
    );
}

/// Deltas carry the cell's OPAQUE unified block state verbatim, and a fresh
/// replica applying them converges on every stateful kind — placements whose
/// state lands AFTER the announcing block write (chest facing, torch
/// placement) included, because the drain re-reads the store.
#[test]
fn deltas_carry_cell_state_and_replicas_converge_on_it() {
    let (mut server, mut replica) = server_and_replica();
    // Converge on the pristine floor first (the delta path needs installed
    // sections on the replica).
    let columns: Vec<_> = server
        .columns
        .keys()
        .copied()
        .map(|cp| server.column_payload(cp).expect("column loaded"))
        .collect();
    let sections: Vec<_> = server.sections.values().map(|s| s.to_payload()).collect();
    for c in columns {
        replica.install_remote_column(c);
    }
    for s in sections {
        replica.install_remote_section(s);
    }

    server.set_replication_capture(true);
    let stair = IVec3::new(2, 65, 2);
    let torch = IVec3::new(3, 65, 3);
    let door = IVec3::new(4, 65, 4);
    let slab = IVec3::new(6, 65, 2);
    let log = IVec3::new(7, 65, 3);
    let model = IVec3::new(10, 65, 10);
    let chest = IVec3::new(1, 65, 1);
    assert!(server.place_stair(
        stair,
        Block::OakStairs,
        petramond_world::block_state::StairState::new(Facing::South, petramond_world::block_state::StairHalf::Top),
    ));
    assert!(server.set_block_world(torch.x, torch.y, torch.z, Block::Torch));
    server.insert_torch(torch, TorchPlacement::East);
    assert!(server.place_door(door, Block::OakDoor, Facing::East));
    assert!(server.place_slab_layer(
        slab,
        Block::CobblestoneSlab,
        SlabSlot {
            split: SlabSplit::Y,
            index: 0,
        },
    ));
    assert!(server.place_log(log, Block::OakLog, LogAxis::X));
    assert!(server.place_model_block_facing(model, Block::FurnitureWorkbench, Facing::East));
    assert!(server.set_block_world(chest.x, chest.y, chest.z, Block::Chest));
    server.insert_chest(chest, Facing::West);

    let deltas = server.take_block_deltas();
    let state_at = |pos: IVec3| {
        deltas
            .iter()
            .find(|d| d.pos == pos)
            .unwrap_or_else(|| panic!("delta logged at {pos:?}"))
            .state
    };
    let stair_state = state_at(stair).expect("stair delta carries state");
    assert_eq!(
        <petramond_world::block_state::StairState as petramond_world::block::CellView>::from_cell(stair_state),
        petramond_world::block_state::StairState::new(Facing::South, petramond_world::block_state::StairHalf::Top),
        "placed bits ride the delta"
    );
    assert_ne!(
        stair_state.byte(1),
        0,
        "the refined corner byte rides along (the edit cascade ran server-side)"
    );
    assert_eq!(
        state_at(torch),
        Some(petramond_world::block::CellCodec::to_cell(&TorchPlacement::East))
    );
    assert!(state_at(door).is_some());
    assert!(state_at(door + IVec3::Y).is_some());
    let slab_state = state_at(slab).expect("slab delta carries state");
    assert_eq!(
        (slab_state.id_at(1), slab_state.id_at(3)),
        (Block::CobblestoneSlab.id(), Block::Air.id()),
        "slab layers ride as raw block ids"
    );
    assert_eq!(
        slab_state.id_mask(),
        0b0_1010,
        "each layer's TWO id bytes are declared id references for the transport remap"
    );
    assert!(state_at(log).is_some());
    assert!(state_at(model).is_some());
    assert!(
        state_at(chest).is_some(),
        "the chest facing inserted AFTER set_block_world still rides the delta"
    );

    for d in &deltas {
        replica.apply_remote_delta(d.clone());
    }
    assert_eq!(
        replica.stair_state_at(stair.x, stair.y, stair.z),
        server.stair_state_at(stair.x, stair.y, stair.z)
    );
    assert_eq!(replica.torch_placement(torch), TorchPlacement::East);
    assert_eq!(
        replica.door_state_at(door.x, door.y, door.z),
        server.door_state_at(door.x, door.y, door.z)
    );
    assert_eq!(
        replica.door_state_at(door.x, door.y + 1, door.z),
        server.door_state_at(door.x, door.y + 1, door.z)
    );
    assert_eq!(
        replica.slab_state_at(slab.x, slab.y, slab.z),
        server.slab_state_at(slab.x, slab.y, slab.z)
    );
    assert_eq!(replica.log_axis_at(log.x, log.y, log.z), LogAxis::X);
    assert_eq!(
        replica.model_offset_at(model.x + 1, model.y, model.z),
        server.model_offset_at(model.x + 1, model.y, model.z)
    );
    assert_eq!(
        replica.model_facing_at(model.x, model.y, model.z),
        Facing::East
    );
    let mut chests = Vec::new();
    replica.collect_chests(&mut chests);
    assert!(
        chests
            .iter()
            .any(|&(p, f, ..)| p == chest && f == Facing::West),
        "the chest placed post-join renders on the replica with its facing"
    );

    // Breaking the stair clears the replicated state too (state: None).
    server.set_replication_capture(true);
    assert!(server.set_block_world(stair.x, stair.y, stair.z, Block::Air));
    for d in server.take_block_deltas() {
        replica.apply_remote_delta(d);
    }
    assert_eq!(
        replica.stair_state_at(stair.x, stair.y, stair.z),
        petramond_world::block_state::StairState::default(),
        "a cleared cell reads the default state again"
    );
}

/// A door TOGGLE flips the door map with no
/// block-id write — it must still log deltas (state carries the open bit)
/// and the replica's door map must follow, so collision + the resting
/// swing angle are right.
#[test]
fn door_toggles_replicate_the_open_bit_without_a_block_change() {
    let (mut server, mut replica) = server_and_replica();
    let base = IVec3::new(5, 65, 5);
    assert!(server.place_door(base, Block::OakDoor, Facing::East));
    let columns: Vec<_> = server
        .columns
        .keys()
        .copied()
        .map(|cp| server.column_payload(cp).expect("column loaded"))
        .collect();
    let sections: Vec<_> = server.sections.values().map(|s| s.to_payload()).collect();
    for c in columns {
        replica.install_remote_column(c);
    }
    for s in sections {
        replica.install_remote_section(s);
    }
    assert!(!replica.door_state_at(base.x, base.y, base.z).unwrap().open);

    server.set_replication_capture(true);
    assert_eq!(server.toggle_door(base), Some(base));
    let deltas = server.take_block_deltas();
    assert_eq!(deltas.len(), 2, "both door cells log a delta on toggle");
    for d in deltas {
        replica.apply_remote_delta(d);
    }
    for cell in [base, base + IVec3::Y] {
        let got = replica.door_state_at(cell.x, cell.y, cell.z).unwrap();
        assert!(got.open, "the replica's door map opened at {cell:?}");
        assert_eq!(
            Some(got),
            server.door_state_at(cell.x, cell.y, cell.z),
            "replica and server door state agree"
        );
    }

    // And back closed.
    assert_eq!(server.toggle_door(base), Some(base));
    for d in server.take_block_deltas() {
        replica.apply_remote_delta(d);
    }
    assert!(!replica.door_state_at(base.x, base.y, base.z).unwrap().open);
}

/// A hand-built column payload: flat maps, all-unknown summaries, and the
/// given deep band floor — the minimum a replica needs to classify deep.
fn column_payload_fixture(pos: ChunkPos, deep_band_lo: i32) -> crate::net::protocol::ColumnPayload {
    use petramond_world::chunk::SECTION_SIZE;
    use crate::net::protocol::{ColumnPayload, SectionBytes};
    let flat = |n: usize| SectionBytes(Arc::from(vec![0u8; n].into_boxed_slice()));
    ColumnPayload {
        pos,
        biomes: flat(SECTION_SIZE * SECTION_SIZE),
        mesh_biomes: flat(20 * 20),
        surface_heightmap: vec![64; SECTION_SIZE * SECTION_SIZE],
        sky_cover: vec![64; SECTION_SIZE * SECTION_SIZE],
        summaries: vec![0u8; WorldData::column_section_range().count()],
        deep_band_lo,
    }
}

/// The replica classifies deep from the replicated band floor — and a
/// section that lands BEFORE its column (an ordering regression the sender
/// currently prevents) must still be re-classified when the column
/// arrives, not silently stay meshable forever.
#[test]
fn replica_deep_classification_heals_out_of_order_column_installs() {
    let deep_pos = SectionPos::new(0, -2, 0);
    let solid = {
        let mut s = Section::new(deep_pos.cx, deep_pos.cy, deep_pos.cz);
        s.blocks_mut().fill(Block::Stone.id());
        s.recompute_opaque_count();
        s
    };
    let make_replica = || {
        let mut r = World::new_with_role(0, 4, WorldRole::ClientReplica);
        // View centre far above the section so the always-mesh near ring
        // doesn't mask the classification.
        r.set_replica_view_center(0, 10, 0);
        r
    };

    // Normal order: column (with its band floor) before the section.
    let mut replica = make_replica();
    replica.install_remote_column(column_payload_fixture(deep_pos.chunk_pos(), 2));
    replica.install_remote_section(solid.to_payload());
    assert!(
        replica.terrain.deep_sections.contains(&deep_pos),
        "a below-band section installed after its column classifies deep"
    );

    // Regressed order: section first — the column landing must heal it.
    let mut replica = make_replica();
    replica.install_remote_section(solid.to_payload());
    assert!(
        !replica.terrain.deep_sections.contains(&deep_pos),
        "without a band floor the section stays (safely) non-deep"
    );
    replica.install_remote_column(column_payload_fixture(deep_pos.chunk_pos(), 2));
    assert!(
        replica.terrain.deep_sections.contains(&deep_pos),
        "the column install must re-classify already-installed sections"
    );
}

#[test]
fn replication_log_coalesces_latest_wins_and_respects_capture() {
    let mut w = crate::world::testutil::flat_world();
    assert!(w.set_block_world(2, 70, 2, Block::Stone));
    assert!(w.take_block_deltas().is_empty(), "capture off logs nothing");

    w.set_replication_capture(true);
    assert!(w.set_block_world(3, 70, 3, Block::Stone));
    assert!(w.set_block_world(3, 70, 3, Block::Dirt)); // same cell, same tick
    assert!(w.set_water_world(IVec3::new(4, 70, 4), Block::Water, 0x83));
    let deltas = w.take_block_deltas();
    assert_eq!(deltas.len(), 2, "one delta per cell per take");
    let cell = deltas
        .iter()
        .find(|d| d.pos == IVec3::new(3, 70, 3))
        .expect("edited cell logged");
    assert_eq!(cell.block_id, Block::Dirt.id(), "latest write wins");
    assert_eq!(cell.water, None);
    let water = deltas
        .iter()
        .find(|d| d.pos == IVec3::new(4, 70, 4))
        .expect("water cell logged");
    assert_eq!(water.block_id, Block::Water.id());
    assert_eq!(water.water, Some(0x83), "water meta rides the delta");
    assert!(w.take_block_deltas().is_empty(), "take drains the log");
}

/// The per-connection send plan: wanted loaded+FINAL sections ship (both
/// finality gates: in-flight streaming AND light not yet baked), sent
/// terrain leaving the keep shape (or the server) plans an unload, and the
/// send key is stable across pumps that change nothing — the
/// incrementality gate.
#[test]
fn terrain_send_plan_gates_finality_and_unloads_the_keep_shape_exit() {
    use petramond_world::chunk::SECTION_VOLUME;
    use petramond_world::section::Section;
    use crate::world::store::LoadAnchor;
    use rustc_hash::{FxHashMap, FxHashSet};

    let sky = || Arc::from(vec![0u8; SECTION_VOLUME].into_boxed_slice());
    let mut w = World::new(0, 2);
    let sp = SectionPos::new(0, 4, 0);
    let mut section = Section::new(0, 4, 0);
    section.set_block(0, 0, 0, Block::Stone);
    w.insert_section_for_test(sp, section);
    let anchor = |cx: i32| LoadAnchor {
        cx,
        cy: 4,
        cz: 0,
        radius: 64,
    };

    let mut sent_columns: FxHashSet<ChunkPos> = FxHashSet::default();
    let mut sent_sections: FxHashSet<SectionPos> = FxHashSet::default();
    let mut sent_by_column: FxHashMap<ChunkPos, Vec<i32>> = FxHashMap::default();
    let note_sent = |sp: SectionPos,
                     sent_sections: &mut FxHashSet<SectionPos>,
                     sent_by_column: &mut FxHashMap<ChunkPos, Vec<i32>>| {
        if sent_sections.insert(sp) {
            sent_by_column
                .entry(sp.chunk_pos())
                .or_default()
                .push(sp.cy);
        }
    };
    let forget_sent = |sp: SectionPos,
                       sent_sections: &mut FxHashSet<SectionPos>,
                       sent_by_column: &mut FxHashMap<ChunkPos, Vec<i32>>| {
        if !sent_sections.remove(&sp) {
            return;
        }
        if let Some(cys) = sent_by_column.get_mut(&sp.chunk_pos()) {
            cys.retain(|&cy| cy != sp.cy);
            if cys.is_empty() {
                sent_by_column.remove(&sp.chunk_pos());
            }
        }
    };
    // Light gates shipping: a never-baked (non-opaque) section is not
    // presentable — the replica can't bake it, so the server holds it.
    let plan = w.plan_terrain_send(
        anchor(0),
        &sent_columns,
        &sent_sections,
        &sent_by_column,
        128,
    );
    assert!(
        !plan.sections.contains(&sp),
        "a lightless section is held back by the ship gate"
    );
    w.section_at_world_mut_for_test(0, 64, 0)
        .unwrap()
        .set_skylight(sky());
    let plan = w.plan_terrain_send(
        anchor(0),
        &sent_columns,
        &sent_sections,
        &sent_by_column,
        128,
    );
    assert!(
        plan.sections.contains(&sp),
        "the loaded, lit, wanted section ships"
    );
    sent_columns.insert(sp.chunk_pos());
    note_sent(sp, &mut sent_sections, &mut sent_by_column);

    // The send key: stable while nothing moved; re-keyed by new content
    // and by an anchor chunk move.
    let k = w.terrain_send_key(anchor(0));
    assert_eq!(k, w.terrain_send_key(anchor(0)));
    assert_ne!(k, w.terrain_send_key(anchor(1)), "a chunk move re-keys");
    let mut other = Section::new(1, 4, 0);
    other.set_block(0, 0, 0, Block::Stone);
    other.set_skylight(sky());
    w.insert_section_for_test(SectionPos::new(1, 4, 0), other);
    assert_ne!(k, w.terrain_send_key(anchor(0)), "new content re-keys");

    // A loaded section whose saved overlay is still in flight is NOT
    // final: it must not ship until the overlay resolves.
    w.gen.awaited_overlays.insert(SectionPos::new(1, 4, 0));
    w.note_stream_nonfinal(SectionPos::new(1, 4, 0));
    let plan = w.plan_terrain_send(
        anchor(0),
        &sent_columns,
        &sent_sections,
        &sent_by_column,
        128,
    );
    assert!(
        !plan.sections.contains(&SectionPos::new(1, 4, 0)),
        "an in-flight section must not be sent (its base would lie)"
    );
    w.gen.awaited_overlays.clear();
    w.rebuild_stream_nonfinal();
    let plan = w.plan_terrain_send(
        anchor(0),
        &sent_columns,
        &sent_sections,
        &sent_by_column,
        128,
    );
    assert!(plan.sections.contains(&SectionPos::new(1, 4, 0)));

    // A sent section the server evicted (vertical exit) unloads even while
    // its column is kept.
    let gone = SectionPos::new(0, 9, 0);
    note_sent(gone, &mut sent_sections, &mut sent_by_column);
    let plan = w.plan_terrain_send(
        anchor(0),
        &sent_columns,
        &sent_sections,
        &sent_by_column,
        128,
    );
    assert!(plan.drop_sections.contains(&gone));
    assert!(plan.drop_columns.is_empty());
    forget_sent(gone, &mut sent_sections, &mut sent_by_column);

    // The whole column leaving the keep shape plans a ColumnUnload (its
    // sections drop with it — no per-section messages).
    let plan = w.plan_terrain_send(
        anchor(20),
        &sent_columns,
        &sent_sections,
        &sent_by_column,
        128,
    );
    assert!(plan.drop_columns.contains(&sp.chunk_pos()));
    assert!(!plan.drop_sections.contains(&sp));
}

/// Deep sections below the column surface band stay off the wire until the
/// connection's vertical window (or near ring) needs them — they would park
/// on the replica without meshing anyway.
#[test]
fn terrain_send_defers_deep_sections_outside_the_anchor_window() {
    use petramond_world::chunk::SECTION_VOLUME;
    use petramond_world::section::Section;
    use crate::world::store::LoadAnchor;
    use rustc_hash::{FxHashMap, FxHashSet};

    let sky = || Arc::from(vec![0u8; SECTION_VOLUME].into_boxed_slice());
    let mut w = World::new(0, 8);
    let cp = ChunkPos::new(0, 0);
    let gen = petramond_worldgen::driver::ChunkGenerator::new(0).generate_column_gen(cp.cx, cp.cz);
    let band_lo = *World::surface_window_for_column(&gen, 0).start();
    w.set_column_gen(cp, Arc::new(gen));

    // Deepest legal cy: a surface anchor at band_lo+2 has vwin down to
    // band_lo-3, so SECTION_MIN_CY must sit below that for the deferral
    // case to be distinguishable (true for any band_lo ≥ SECTION_MIN_CY+4).
    let deep_cy = petramond_world::chunk::SECTION_MIN_CY;
    assert!(
        deep_cy < band_lo,
        "seed 0 column must have a surface band above world floor (band_lo={band_lo})"
    );
    let deep = SectionPos::new(0, deep_cy, 0);
    let mut section = Section::new(deep.cx, deep.cy, deep.cz);
    section.set_block(0, 0, 0, Block::Stone);
    section.set_skylight(sky());
    w.insert_section_for_test(deep, section);
    assert!(
        w.section_light_final(deep) && w.stream_writable(deep),
        "fixture must be ship-final"
    );

    // Place the surface anchor so its vertical window (radius 5) and near
    // ring both miss SECTION_MIN_CY.
    let surface_cy = (deep_cy + 6).max(band_lo + 2);
    assert!(!World::vertical_window(surface_cy, 0).contains(&deep_cy));
    let plan = w.plan_terrain_send(
        LoadAnchor {
            cx: 0,
            cy: surface_cy,
            cz: 0,
            radius: 64,
        },
        &FxHashSet::default(),
        &FxHashSet::default(),
        &FxHashMap::default(),
        128,
    );
    assert!(
        !plan.sections.contains(&deep),
        "deep section outside the surface anchor window must not ship yet"
    );

    let plan = w.plan_terrain_send(
        LoadAnchor {
            cx: 0,
            cy: deep.cy,
            cz: 0,
            radius: 64,
        },
        &FxHashSet::default(),
        &FxHashSet::default(),
        &FxHashMap::default(),
        128,
    );
    assert!(
        plan.sections.contains(&deep),
        "the same deep section ships once the anchor window covers it"
    );
}

#[test]
fn sealed_mixed_section_is_not_final_without_light() {
    let mut world = World::new(0, 16);
    let center = SectionPos::new(0, 0, 0);
    let mut cavity = Section::new(0, 0, 0);
    cavity.blocks_mut().fill(Block::Stone.id());
    cavity.recompute_opaque_count();
    cavity.set_block(8, 8, 8, Block::Air);
    world.insert_section_for_test(center, cavity);
    for (dx, dy, dz) in [
        (1, 0, 0),
        (-1, 0, 0),
        (0, 1, 0),
        (0, -1, 0),
        (0, 0, 1),
        (0, 0, -1),
    ] {
        let pos = SectionPos::new(center.cx + dx, center.cy + dy, center.cz + dz);
        let mut section = Section::new(pos.cx, pos.cy, pos.cz);
        section.blocks_mut().fill(Block::Stone.id());
        section.recompute_opaque_count();
        world.insert_section_for_test(pos, section);
    }
    world.last_load_target = Some(LoadTarget::new(8, 0, 0, 16));

    assert!(world.section_sealed_by_loaded_neighbors(center));
    assert!(
        !world.section_light_final(center),
        "a mixed section needs real light before replication can call it final"
    );
}

#[test]
fn server_headless_never_queues_mesh_work() {
    let mut headless = World::new_with_role(0, 1, WorldRole::ServerHeadless);
    headless.insert_empty_column_for_test(ChunkPos::new(0, 0));
    assert!(headless.set_block_world(8, 64, 8, Block::Stone));
    assert_eq!(
        headless.dirty_mesh_count(),
        0,
        "nobody pumps a headless world's meshes; the queue must stay empty"
    );
    // Light bookkeeping still runs — the server keeps light current.
    assert!(
        headless
            .section_at_world_for_test(8, 64, 8)
            .unwrap()
            .light_dirty
    );

    let mut combined = World::new(0, 1);
    combined.insert_empty_column_for_test(ChunkPos::new(0, 0));
    assert!(combined.set_block_world(8, 64, 8, Block::Stone));
    assert!(
        combined.dirty_mesh_count() > 0,
        "the combined world still queues meshes as before"
    );
}

/// Per-connection view distance: the send shape follows the anchor's own
/// radius, clamped by the server world's budget — a client may shrink its
/// stream but never widen it past the server setting.
#[test]
fn send_target_clamps_anchor_radius_to_the_world_budget() {
    use crate::world::LoadAnchor;
    let w = World::new_with_role(0, 4, WorldRole::ServerHeadless);
    let key = |radius| {
        w.terrain_target_key(LoadAnchor {
            cx: 0,
            cy: 4,
            cz: 0,
            radius,
        })
    };
    assert_eq!(key(64), key(4), "requests above the budget clamp to it");
    assert_ne!(key(2), key(4), "smaller requests shrink the send shape");
}

/// The live cell-KV delta lane: a server-side KV write/delete on a loaded
/// section is captured (behind the replication gate), drains coalesced, and
/// converges the replica — including the ordering contract with block
/// deltas: a same-tick block flip WIPES the cell's KV on both sides, so the
/// KV delta must apply after the block delta to survive.
#[test]
fn cell_kv_deltas_replicate_and_apply_after_block_deltas() {
    let (mut server, mut replica) = server_and_replica();
    for cp in server.columns.keys().copied().collect::<Vec<_>>() {
        replica.install_remote_column(server.column_payload(cp).unwrap());
    }
    for s in server.sections.values() {
        replica.install_remote_section(s.to_payload());
    }

    // Capture OFF: writes replicate nothing (the SP-no-clients fast path).
    assert!(server.cell_kv_set(2, 65, 2, "testmod:color".into(), vec![1]));
    assert!(server.take_cell_kv_deltas().is_empty());

    server.set_replication_capture(true);
    // The fill shape: block flip first, then the cell's KV — one tick. The
    // covering block delta's drain-time KV snapshot carries the value; no
    // separate KV delta is logged (it would be redundant).
    assert!(server.set_block_world(2, 65, 2, Block::Stone));
    assert!(server.cell_kv_set(2, 65, 2, "testmod:color".into(), vec![200, 30, 40]));
    let blocks = server.take_block_deltas();
    assert!(
        server.take_cell_kv_deltas().is_empty(),
        "a covering block delta subsumes the cell's KV deltas"
    );
    for d in blocks {
        replica.apply_remote_delta(d);
    }
    assert_eq!(
        replica.cell_kv_get(2, 65, 2, "testmod:color"),
        Some(&[200u8, 30, 40][..]),
        "the KV rides the block delta's snapshot"
    );

    // A value-only change (no block flip) and a delete both converge.
    assert!(server.cell_kv_set(2, 65, 2, "testmod:color".into(), vec![9]));
    for kv in server.take_cell_kv_deltas() {
        replica.apply_remote_cell_kv(kv);
    }
    assert_eq!(
        replica.cell_kv_get(2, 65, 2, "testmod:color"),
        Some(&[9u8][..])
    );
    assert!(server.cell_kv_remove(2, 65, 2, "testmod:color"));
    for kv in server.take_cell_kv_deltas() {
        replica.apply_remote_cell_kv(kv);
    }
    assert_eq!(replica.cell_kv_get(2, 65, 2, "testmod:color"), None);

    // A CORRECTIVE delta — `block_delta_at`'s snapshot of an UNCHANGED cell,
    // shipped whenever a click resolves to nothing — must not erase replica
    // KV the server still holds (the gray-dye bug, 2026-07-23): the delta
    // carries the cell's KV map and re-installs it after its own wipe.
    assert!(server.cell_kv_set(2, 65, 2, "testmod:color".into(), vec![5, 6, 7]));
    for kv in server.take_cell_kv_deltas() {
        replica.apply_remote_cell_kv(kv);
    }
    let corrective = server.block_delta_at(IVec3::new(2, 65, 2)).unwrap();
    replica.apply_remote_delta(corrective);
    assert_eq!(
        replica.cell_kv_get(2, 65, 2, "testmod:color"),
        Some(&[5u8, 6, 7][..]),
        "a corrective snapshot must carry the cell's KV across its wipe"
    );

    // The GHOST-KV hazard, reversed order: a KV write logged BEFORE a
    // same-tick block flip is stale — the flip wiped the cell's KV — so the
    // block delta's capture must scrub it. Without the scrub the stale KV
    // delta replays AFTER the block delta and resurrects a key on the
    // replica that the server holds clean.
    assert!(server.cell_kv_set(2, 65, 2, "testmod:ghost".into(), vec![1]));
    assert!(server.set_block_world(2, 65, 2, Block::Dirt));
    assert_eq!(server.cell_kv_get(2, 65, 2, "testmod:ghost"), None);
    let blocks = server.take_block_deltas();
    let kvs = server.take_cell_kv_deltas();
    assert!(
        kvs.is_empty(),
        "the wiping block write scrubs the cell's pending KV deltas"
    );
    for d in blocks {
        replica.apply_remote_delta(d);
    }
    for kv in kvs {
        replica.apply_remote_cell_kv(kv);
    }
    assert_eq!(
        replica.cell_kv_get(2, 65, 2, "testmod:ghost"),
        None,
        "no ghost KV survives on the replica after the wipe"
    );
}

/// A draw set is RETAINED state, and the delta lane only carries changes — so
/// a machine that last redrew itself before a player joined must still arrive,
/// on the section, or it is invisible to everyone but the placer.
#[test]
fn retained_draw_sets_ride_the_section_to_a_late_joiner() {
    use mod_api::DrawPrim;

    let (mut server, mut replica) = server_and_replica();
    let pos = IVec3::new(4, 65, 4);
    assert!(server.set_block_world(pos.x, pos.y, pos.z, Block::Furnace));
    server.set_block_draw(
        pos,
        vec![DrawPrim::Cuboid {
            min: [0.2, 0.0, 0.2],
            max: [0.8, 0.5, 0.8],
            tile: "stone".into(),
            tint: [255, 0, 0],
            emissive: true,
        }]
        .into(),
    );
    // Everything the delta lane had to say has already been said.
    let _ = server.take_block_draw_deltas();

    for cp in server.columns.keys().copied().collect::<Vec<_>>() {
        replica.install_remote_column(server.column_payload(cp).unwrap());
    }
    for sp in server.sections.keys().copied().collect::<Vec<_>>() {
        replica.install_remote_section(server.section_payload(sp).unwrap());
    }

    let set = replica
        .block_draw_at(pos)
        .expect("the joiner sees the machine's drawing");
    assert_eq!(set.wire.as_slice().len(), 1);
    assert_eq!(set.resolved.len(), 1, "names resolved on the replica");
}

/// The two halves of a draw set's lifetime, both owned by the ENGINE because
/// every mod-side memo of them is wrong on reload or unload.
///
/// Resubmitting the same picture is the every-tick idiom, so it must cost no
/// delta at all; and a cell that becomes something ELSE must lose its set,
/// on the replica too. Nothing else can clear it — `SetBlockDraw` is gated on
/// owning the block that just stopped existing — so a set that outlives its
/// block draws over a cell forever and rides the section payload to every
/// player who joins afterwards.
#[test]
fn a_draw_set_costs_nothing_to_resubmit_and_dies_with_its_block() {
    use mod_api::DrawPrim;

    let (mut server, mut replica) = server_and_replica();
    let pos = IVec3::new(4, 65, 4);
    assert!(server.set_block_world(pos.x, pos.y, pos.z, Block::Furnace));
    let picture = || {
        crate::world::draw::DrawPrims::from(vec![DrawPrim::Cuboid {
            min: [0.2, 0.0, 0.2],
            max: [0.8, 0.5, 0.8],
            tile: "stone".into(),
            tint: [255, 0, 0],
            emissive: false,
        }])
    };

    server.set_replication_capture(true);
    server.set_block_draw(pos, picture());
    assert_eq!(
        server.take_block_draw_deltas().len(),
        1,
        "the first submission is a change"
    );
    server.set_block_draw(pos, picture());
    assert!(
        server.take_block_draw_deltas().is_empty(),
        "a machine at rest redraws itself every tick; that must not replicate"
    );

    for cp in server.columns.keys().copied().collect::<Vec<_>>() {
        replica.install_remote_column(server.column_payload(cp).unwrap());
    }
    for sp in server.sections.keys().copied().collect::<Vec<_>>() {
        replica.install_remote_section(server.section_payload(sp).unwrap());
    }
    assert!(replica.block_draw_at(pos).is_some(), "fixture: shipped");

    // The cell becomes something else. Not a BREAK — a plain block write, the
    // path whose own rule is that per-cell state dies with the block.
    assert!(server.set_block_world(pos.x, pos.y, pos.z, Block::Stone));
    assert!(
        server.block_draw_at(pos).is_none(),
        "the drawing went with the block that owned it"
    );
    for delta in server.take_block_draw_deltas() {
        replica.apply_remote_block_draw(delta.pos, delta.prims);
    }
    assert!(
        replica.block_draw_at(pos).is_none(),
        "and the clear reached the replica"
    );
}

/// A mod computing geometry can produce a non-finite corner (a divide by a
/// zero-length pour). Those must not reach the vertex buffer.
#[test]
fn non_finite_draw_geometry_is_dropped_at_the_boundary() {
    use mod_api::DrawPrim;

    let (mut server, _replica) = server_and_replica();
    let pos = IVec3::new(4, 65, 4);
    server.set_block_draw(
        pos,
        vec![
            DrawPrim::Cuboid {
                min: [0.0, 0.0, 0.0],
                max: [1.0, f32::INFINITY, 1.0],
                tile: "stone".into(),
                tint: [255, 255, 255],
                emissive: false,
            },
            DrawPrim::Cuboid {
                min: [0.0, 0.0, 0.0],
                max: [1.0, 1.0, 1.0],
                tile: "stone".into(),
                tint: [255, 255, 255],
                emissive: false,
            },
        ]
        .into(),
    );
    let set = server.block_draw_at(pos).unwrap();
    assert_eq!(
        set.wire.as_slice().len(),
        2,
        "the wire keeps what the mod said"
    );
    assert_eq!(set.resolved.len(), 1, "the renderer only gets the sane box");
}
