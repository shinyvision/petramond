use super::*;
use petramond_math::facing::Facing;
use petramond_math::math::Vec3;
use petramond_world::block::Block;
use petramond_world::block_state::{LogAxis, SlabSplit, SlabState, StairState};
use petramond_world::item::{ItemStack, ItemType};

fn sec(cx: i32, cy: i32, cz: i32) -> Section {
    Section::new(cx, cy, cz)
}

/// Light persistence contract: clean baked light roundtrips byte-exact
/// and loads CLEAN (no re-bake); never-baked or stale (dirty) light is
/// withheld from the record so a reload re-bakes it.
#[test]
fn baked_light_persists_only_when_clean_and_roundtrips() {
    use std::sync::Arc;

    let mut s = sec(0, 4, 0);
    s.set_block(1, 2, 3, Block::Stone);

    // Never baked: no cubes in the record, loads dirty.
    let rec = encode_snapshot(&SectionSnapshot::from_section(&s));
    let (back, ..) = decode_section(SectionPos::new(0, 4, 0), &rec).expect("decodes");
    assert!(!back.has_baked_light(), "no light was persisted");
    assert!(
        back.light_dirty,
        "absent persisted light means bake on load"
    );

    // Clean baked light: roundtrips byte-exact, loads clean.
    let sky: Vec<u8> = (0..SECTION_VOLUME).map(|i| (i % 16) as u8).collect();
    // A COLOURED cell, so the record must carry all three channels through
    // the widened blob rather than a luminance.
    let mut bl = vec![petramond_world::light::LightRgb::ZERO; SECTION_VOLUME];
    bl[100] = petramond_world::light::LightRgb::new(9, 2, 30);
    s.set_skylight(Arc::from(sky.clone().into_boxed_slice()));
    s.set_blocklight(Arc::from(bl.clone().into_boxed_slice()));
    let rec = encode_snapshot(&SectionSnapshot::from_section(&s));
    let (back, ..) = decode_section(SectionPos::new(0, 4, 0), &rec).expect("decodes");
    assert!(
        !back.light_dirty,
        "persisted light loads clean — no re-bake"
    );
    assert_eq!(
        &back.skylight_arc().expect("skylight persisted")[..],
        &sky[..]
    );
    assert_eq!(
        &back.blocklight_arc().expect("blocklight persisted")[..],
        &bl[..]
    );

    // A post-bake edit re-dirties the light: stale cubes must NOT persist.
    s.set_block(2, 2, 2, Block::Dirt);
    let snap = SectionSnapshot::from_section(&s);
    assert!(
        snap.skylight.is_none() && snap.blocklight.is_none(),
        "stale (dirty) light is withheld from the record"
    );
}

#[test]
fn section_record_roundtrips() {
    // A section spans world Y [cy*16 .. cy*16+16); negative cy is in range.
    let mut s = sec(-3, -2, 7);
    s.set_block(1, 4, 2, Block::Stone);
    s.set_block(0, 10, 0, Block::Grass);
    s.set_water(5, 5, 5, Block::Water, 0x23);

    let snap = SectionSnapshot::from_section(&s);
    let blob = encode_snapshot(&snap);
    let (back, entities, mobs) =
        decode_section(SectionPos::new(-3, -2, 7), &blob).expect("decodes");

    assert_eq!((back.cx, back.cy, back.cz), (-3, -2, 7));
    assert_eq!(back.block_raw(1, 4, 2), Block::Stone.id());
    assert_eq!(back.block_raw(0, 10, 0), Block::Grass.id());
    assert_eq!(back.block_raw(5, 5, 5), Block::Water.id());
    assert_eq!(back.water_meta(5, 5, 5), 0x23);
    assert!(!back.modified);
    assert!(entities.is_empty(), "no entities attached");
    assert!(mobs.is_empty(), "no mobs attached");
}

#[test]
fn section_record_roundtrips_entities() {
    let mut s = sec(2, 4, 2);
    s.set_block(8, 0, 8, Block::Dirt);
    let mut snap = SectionSnapshot::from_section(&s);
    let mut drop = DroppedItem::new(
        Vec3::new(40.5, 65.0, 40.5),
        ItemStack::new(ItemType::Stone, 7),
        1,
    );
    drop.ticks_lived = 1234;
    snap.entities.push(drop);

    let blob = encode_snapshot(&snap);
    let (_back, entities, _mobs) =
        decode_section(SectionPos::new(2, 4, 2), &blob).expect("decodes");
    assert_eq!(entities.len(), 1);
    assert_eq!(entities[0].stack, ItemStack::new(ItemType::Stone, 7));
    assert_eq!(
        entities[0].ticks_lived, 1234,
        "remaining lifetime survives the save"
    );
}

#[test]
fn section_record_roundtrips_mobs() {
    let mut s = sec(-1, 4, 4);
    s.set_block(8, 0, 8, Block::Dirt);
    let mut snap = SectionSnapshot::from_section(&s);
    snap.mobs.push(SavedMob {
        kind: crate::mob::Mob::Owl,
        pos: Vec3::new(-12.5, 65.0, 72.25),
        yaw: 1.75,
        tags: Default::default(),
    });

    let blob = encode_snapshot(&snap);
    let (_back, _entities, mobs) =
        decode_section(SectionPos::new(-1, 4, 4), &blob).expect("decodes");
    assert_eq!(mobs.len(), 1);
    assert_eq!(mobs[0].kind, crate::mob::Mob::Owl);
    assert_eq!(
        mobs[0].pos,
        Vec3::new(-12.5, 65.0, 72.25),
        "position persists"
    );
    assert_eq!(mobs[0].yaw, 1.75, "facing persists");
}

#[test]
fn section_record_roundtrips_furnaces() {
    use petramond_world::furnace::{FURNACE_SLOTS, SLOT_FUEL, SLOT_INPUT};
    let mut s = sec(1, 4, 1);
    s.set_block(2, 1, 3, Block::Furnace);
    s.insert_furnace(
        2,
        1,
        3,
        petramond_world::furnace::Furnace {
            cook_progress: 200,
            burn_remaining: 1000,
            burn_max: 4800,
        },
    );
    let mut container = petramond_world::container::Container::with_len(FURNACE_SLOTS);
    container.slots[SLOT_INPUT] = Some(ItemStack::new(ItemType::RawCopper, 12));
    container.slots[SLOT_FUEL] = Some(ItemStack::new(ItemType::Coal, 1));
    s.insert_container(2, 1, 3, container);
    s.insert_entity_facing(2, 1, 3, petramond_math::facing::Facing::West);

    let blob = encode_snapshot(&SectionSnapshot::from_section(&s));
    let (back, _entities, _mobs) =
        decode_section(SectionPos::new(1, 4, 1), &blob).expect("decodes");

    assert_eq!(back.block_raw(2, 1, 3), Block::Furnace.id());
    let f = back.furnace_at(2, 1, 3).expect("furnace restored");
    assert_eq!(f.cook_progress, 200);
    assert_eq!(f.burn_remaining, 1000);
    assert!(f.is_lit(), "a saved burning furnace reloads lit");
    let c = back.container_at(2, 1, 3).expect("slots restored");
    assert_eq!(
        c.slots[SLOT_INPUT],
        Some(ItemStack::new(ItemType::RawCopper, 12))
    );
    assert_eq!(c.slots[SLOT_FUEL], Some(ItemStack::new(ItemType::Coal, 1)));
    assert_eq!(
        back.entity_facing(2, 1, 3),
        petramond_math::facing::Facing::West,
        "facing persists"
    );
}

#[test]
fn foreign_state_bytes_never_decode_through_another_blocks_reader() {
    // The unified store's ownership gate: state bytes written for one block
    // kind must never decode through a different kind's typed reader — a
    // stray facing byte on a ladder cell (which keeps facing as block
    // identity) reads as the default, not as a front.
    let mut s = sec(0, 4, 0);
    s.set_block(3, 3, 3, Block::Ladder);
    s.insert_entity_facing(3, 3, 3, petramond_math::facing::Facing::East);
    s.set_block(5, 3, 3, Block::Chest);
    s.insert_entity_facing(5, 3, 3, petramond_math::facing::Facing::South);

    let blob = encode_snapshot(&SectionSnapshot::from_section(&s));
    let (back, _entities, _mobs) =
        decode_section(SectionPos::new(0, 4, 0), &blob).expect("decodes");

    assert_eq!(back.block_raw(3, 3, 3), Block::Ladder.id());
    assert_eq!(
        back.entity_facing(3, 3, 3),
        petramond_math::facing::Facing::North,
        "a non-directional block's facing read answers the default"
    );
    assert_eq!(
        back.stair_state(5, 3, 3),
        petramond_world::block_state::StairState::default(),
        "a chest's facing byte never decodes as a stair state"
    );
    assert_eq!(
        back.entity_facing(5, 3, 3),
        petramond_math::facing::Facing::South,
        "genuine chest fronts persist"
    );
}

#[test]
fn section_record_roundtrips_chests() {
    let mut s = sec(4, 4, -2);
    s.set_block(9, 2, 1, Block::Chest);
    let mut chest =
        petramond_world::container::Container::with_len(crate::world::chest::CHEST_SLOTS);
    chest.slots[0] = Some(ItemStack::new(ItemType::Stone, 64));
    chest.slots[26] = Some(ItemStack::new(ItemType::OakLog, 5));
    s.insert_container(9, 2, 1, chest);
    s.insert_entity_facing(9, 2, 1, petramond_math::facing::Facing::South);

    let blob = encode_snapshot(&SectionSnapshot::from_section(&s));
    let (back, _entities, _mobs) =
        decode_section(SectionPos::new(4, 4, -2), &blob).expect("decodes");

    assert_eq!(back.block_raw(9, 2, 1), Block::Chest.id());
    let got = back.container_at(9, 2, 1).expect("chest restored");
    assert_eq!(got.slots[0], Some(ItemStack::new(ItemType::Stone, 64)));
    assert_eq!(got.slots[26], Some(ItemStack::new(ItemType::OakLog, 5)));
    assert_eq!(got.slots[5], None);
    assert_eq!(
        back.entity_facing(9, 2, 1),
        petramond_math::facing::Facing::South,
        "facing persists"
    );
}

#[test]
fn section_record_roundtrips_torches() {
    use petramond_world::torch::TorchPlacement;
    let mut s = sec(6, 4, 6);
    s.set_block(3, 3, 4, Block::Torch);
    s.insert_torch(3, 3, 4, TorchPlacement::East);
    s.set_block(3, 4, 4, Block::Torch);
    s.insert_torch(3, 4, 4, TorchPlacement::Floor);

    let blob = encode_snapshot(&SectionSnapshot::from_section(&s));
    let (back, _entities, _mobs) =
        decode_section(SectionPos::new(6, 4, 6), &blob).expect("decodes");

    assert_eq!(back.block_raw(3, 3, 4), Block::Torch.id());
    assert_eq!(
        back.torch_placement(3, 3, 4),
        TorchPlacement::East,
        "wall mount persists"
    );
    assert_eq!(
        back.torch_placement(3, 4, 4),
        TorchPlacement::Floor,
        "floor mount persists"
    );
    // A cell with no torch reads the Floor default.
    assert_eq!(back.torch_placement(0, 0, 0), TorchPlacement::Floor);
}

#[test]
fn section_record_roundtrips_model_cells() {
    // A placed multi-block records authored footprint offsets and per-cell facing;
    // both must survive a save/load so the block reloads as one object.
    let mut s = sec(2, 4, 3);
    s.set_block(5, 0, 5, Block::FurnitureWorkbench);
    s.set_block(6, 0, 5, Block::FurnitureWorkbench);
    s.set_model_offset(6, 0, 5, [1, 0, 0]);
    s.set_model_facing(6, 0, 5, Facing::East);
    s.set_block(5, 1, 5, Block::FurnitureWorkbench);
    s.set_model_offset(5, 1, 5, [0, 1, 0]);
    s.set_model_facing(5, 1, 5, Facing::East);
    s.set_model_facing(5, 0, 5, Facing::East);

    let blob = encode_snapshot(&SectionSnapshot::from_section(&s));
    let (back, _entities, _mobs) =
        decode_section(SectionPos::new(2, 4, 3), &blob).expect("decodes");

    assert_eq!(back.block_raw(6, 0, 5), Block::FurnitureWorkbench.id());
    assert_eq!(back.model_offset(6, 0, 5), [1, 0, 0], "x-offset persists");
    assert_eq!(back.model_offset(5, 1, 5), [0, 1, 0], "y-offset persists");
    assert_eq!(back.model_facing(6, 0, 5), Facing::East, "facing persists");
    assert_eq!(back.model_facing(5, 1, 5), Facing::East, "facing persists");
    assert_eq!(
        back.model_facing(5, 0, 5),
        Facing::East,
        "origin facing persists"
    );
    // The origin cell stores no offset and reads the [0,0,0] default.
    assert_eq!(back.model_offset(5, 0, 5), [0, 0, 0]);
}

#[test]
fn section_record_roundtrips_doors() {
    // A placed door's facing + open + which-half state must reload exactly. State is
    // set AFTER the block.
    use petramond_math::facing::Facing;
    use petramond_world::door::DoorState;
    let mut s = sec(3, 4, 7);
    s.set_block(4, 0, 5, Block::OakDoor);
    s.set_door_state(
        4,
        0,
        5,
        DoorState {
            facing: Facing::East,
            open: true,
            top: false,
        },
    );
    s.set_block(4, 1, 5, Block::OakDoor);
    s.set_door_state(
        4,
        1,
        5,
        DoorState {
            facing: Facing::East,
            open: true,
            top: true,
        },
    );

    let blob = encode_snapshot(&SectionSnapshot::from_section(&s));
    let (back, _entities, _mobs) =
        decode_section(SectionPos::new(3, 4, 7), &blob).expect("decodes");

    assert_eq!(back.block_raw(4, 0, 5), Block::OakDoor.id());
    assert_eq!(
        back.door_state(4, 0, 5),
        Some(DoorState {
            facing: Facing::East,
            open: true,
            top: false
        })
    );
    assert_eq!(
        back.door_state(4, 1, 5).map(|s| s.top),
        Some(true),
        "the upper half persists its top bit"
    );
    // A non-door cell carries no door state.
    assert_eq!(back.door_state(0, 0, 0), None);
}

#[test]
fn section_record_roundtrips_stair_states() {
    let mut s = sec(7, 4, 1);
    s.set_block(2, 0, 3, Block::OakStairs);
    s.set_stair_facing(2, 0, 3, Facing::West);
    s.set_block(9, 5, 1, Block::StoneStairs);
    s.set_stair_state(
        9,
        5,
        1,
        StairState::new(Facing::South, petramond_world::block_state::StairHalf::Top),
    );

    let blob = encode_snapshot(&SectionSnapshot::from_section(&s));
    let (back, _entities, _mobs) =
        decode_section(SectionPos::new(7, 4, 1), &blob).expect("decodes");

    assert_eq!(back.block_raw(2, 0, 3), Block::OakStairs.id());
    assert_eq!(back.stair_facing(2, 0, 3), Facing::West);
    assert_eq!(back.block_raw(9, 5, 1), Block::StoneStairs.id());
    assert_eq!(
        back.stair_state(9, 5, 1),
        StairState::new(Facing::South, petramond_world::block_state::StairHalf::Top)
    );
    assert_eq!(back.stair_state(0, 0, 0), StairState::default());
}

#[test]
fn section_record_roundtrips_slab_states() {
    let mut s = sec(7, 4, 2);
    let state = SlabState {
        split: SlabSplit::Y,
        layers: [Block::DirtSlab, Block::CobblestoneSlab],
    };
    s.set_block(4, 2, 4, Block::CobblestoneSlab);
    s.set_slab_state(4, 2, 4, state);

    let blob = encode_snapshot(&SectionSnapshot::from_section(&s));
    let (back, _entities, _mobs) =
        decode_section(SectionPos::new(7, 4, 2), &blob).expect("decodes");

    assert_eq!(back.block_raw(4, 2, 4), Block::CobblestoneSlab.id());
    assert_eq!(back.slab_state(4, 2, 4), state);
    assert_eq!(back.slab_state(0, 0, 0), SlabState::EMPTY);
}

#[test]
fn section_record_roundtrips_log_axes() {
    let mut s = sec(7, 4, 1);
    s.set_block(2, 0, 3, Block::OakLog);
    s.set_log_axis(2, 0, 3, LogAxis::X);
    s.set_block(9, 5, 1, Block::SpruceLog);
    s.set_log_axis(9, 5, 1, LogAxis::Z);

    let blob = encode_snapshot(&SectionSnapshot::from_section(&s));
    let (back, _entities, _mobs) =
        decode_section(SectionPos::new(7, 4, 1), &blob).expect("decodes");

    assert_eq!(back.log_axis(2, 0, 3), LogAxis::X);
    assert_eq!(back.log_axis(9, 5, 1), LogAxis::Z);
    assert_eq!(back.log_axis(0, 0, 0), LogAxis::Y);
}

#[test]
fn section_record_roundtrips_cell_kv() {
    let mut s = sec(1, 4, 1);
    s.set_block(2, 3, 4, Block::Stone);
    s.cell_kv_set(2, 3, 4, "farm:moisture".into(), vec![7]);
    s.cell_kv_set(0, 0, 0, "othermod:tag".into(), Vec::new());

    let blob = encode_snapshot(&SectionSnapshot::from_section(&s));
    let (back, _entities, _mobs) =
        decode_section(SectionPos::new(1, 4, 1), &blob).expect("decodes");

    assert_eq!(back.cell_kv_get(2, 3, 4, "farm:moisture"), Some(&[7u8][..]));
    assert_eq!(
        back.cell_kv_get(0, 0, 0, "othermod:tag"),
        Some(&[][..]),
        "empty values are values"
    );
    assert_eq!(back.cell_kv_get(2, 3, 4, "farm:missing"), None);
    assert_eq!(back.cell_kv_get(9, 9, 9, "farm:moisture"), None);
}

/// The preservation contract: a record carrying cell KV nobody reads
/// (the owning mod is absent) must survive a load → save cycle BYTE-EXACT —
/// unknown keys are never dropped and the encoding is deterministic.
#[test]
fn cell_kv_is_preserved_byte_exact_through_load_and_save() {
    let mut s = sec(0, 4, 0);
    s.set_block(1, 1, 1, Block::Dirt);
    s.cell_kv_set(1, 1, 1, "ghostmod:data".into(), vec![1, 2, 3, 4]);
    s.cell_kv_set(1, 1, 1, "ghostmod:aaa".into(), vec![5]);
    s.cell_kv_set(5, 5, 5, "ghostmod:other".into(), vec![9]);

    let blob1 = encode_snapshot(&SectionSnapshot::from_section(&s));
    let (loaded, _, _) = decode_section(SectionPos::new(0, 4, 0), &blob1).expect("decodes");
    let blob2 = encode_snapshot(&SectionSnapshot::from_section(&loaded));
    assert_eq!(blob1, blob2, "an untouched record re-encodes byte-exact");
}

/// The stale-record guard: once the last entry is removed the has-cell-kv
/// flag clears, so a re-saved record is indistinguishable from one that
/// never carried KV — nothing lingers to resurrect.
#[test]
fn emptied_cell_kv_clears_its_record_flag() {
    let clean = {
        let mut s = sec(2, 4, 2);
        s.set_block(3, 3, 3, Block::Stone);
        encode_snapshot(&SectionSnapshot::from_section(&s))
    };
    let mut s = sec(2, 4, 2);
    s.set_block(3, 3, 3, Block::Stone);
    s.cell_kv_set(3, 3, 3, "farm:moisture".into(), vec![1]);
    assert_ne!(
        encode_snapshot(&SectionSnapshot::from_section(&s)),
        clean,
        "the tagged record differs"
    );
    assert!(s.cell_kv_remove(3, 3, 3, "farm:moisture"));
    assert_eq!(
        encode_snapshot(&SectionSnapshot::from_section(&s)),
        clean,
        "removing the last entry restores the untagged encoding"
    );
}

#[test]
fn water_free_section_omits_water() {
    let mut s = sec(0, 4, 0);
    s.set_block(8, 0, 8, Block::Dirt);
    let snap = SectionSnapshot::from_section(&s);
    assert!(snap.water.is_none());
    let blob = encode_snapshot(&snap);
    let (back, _, _) = decode_section(SectionPos::new(0, 4, 0), &blob).expect("decodes");
    assert_eq!(back.water_meta(8, 0, 8), 0);
}

#[test]
fn corrupt_blob_is_none() {
    let p = SectionPos::new(0, 0, 0);
    assert!(decode_section(p, &[1, 2, 3, 4]).is_none());
    assert!(decode_section(p, &[]).is_none());
}

/// The record's block cube must carry ids that do not fit a byte, at both
/// index widths. The palette is EXPLICIT rather than the process-wide active
/// one: this is a statement about the format, and the shipped registry has no
/// id this high to reach it with.
#[test]
fn the_record_block_cube_carries_ids_past_one_byte() {
    let pal = crate::save::palette::Palette::identity();
    let roundtrip = |ids: &[u16]| {
        let mut buf = Vec::new();
        put_block_cube(
            &mut buf,
            &petramond_world::section::BlockCube::from_ids(ids),
            &pal,
        );
        let mut r = Reader::new(&buf);
        let back = get_block_cube(&mut r, &pal).expect("cube decodes");
        assert_eq!(&back[..], ids);
    };

    // Narrow index (≤ 256 distinct) with high ids in the palette.
    let mut ids = vec![3u16; SECTION_VOLUME];
    ids[0] = 300;
    ids[1] = 4095;
    ids[2] = 255;
    roundtrip(&ids);

    // Past the narrow index: every cell distinct, so the wide arm runs.
    let wide: Vec<u16> = (0..SECTION_VOLUME).map(|i| (i % 600) as u16).collect();
    roundtrip(&wide);
}

/// The item slot's disk id must be TWO bytes, or an item registered past 255
/// would decode as a different item. The width is asserted on the encoded
/// record (`u16` id + `u8` count + `u16` blob length) rather than by feeding
/// in a high id, because the id crosses the save palette — which pins only
/// ids the registry actually has.
#[test]
fn an_item_slot_stores_a_two_byte_id() {
    let mut slot = Vec::new();
    put_item_slot(&mut slot, Some(ItemStack::new(ItemType::Stone, 5)));
    assert_eq!(slot.len(), 5, "u16 id + u8 count + u16 blob length");
    let mut r = Reader::new(&slot);
    let back = get_item_slot(&mut r)
        .expect("decodes")
        .expect("non-empty slot");
    assert_eq!((back.item, back.count), (ItemType::Stone, 5));
}

/// A section holding an id past one byte must survive the whole record — the
/// cube, and a slab cell state's id-masked layer bytes, which are the only
/// bytes the codec ever reinterprets.
#[test]
fn a_high_id_survives_the_whole_section_record() {
    const HIGH_A: u16 = 300;
    const HIGH_B: u16 = 1234;

    let mut s = sec(2, -1, 3);
    s.set_block_raw(1, 2, 3, HIGH_A);
    s.set_block_raw(4, 5, 6, HIGH_B);
    s.set_block(7, 8, 9, Block::Stone);
    let [a_lo, a_hi] = petramond_world::block::ShapeState::id_bytes(HIGH_A);
    let [b_lo, b_hi] = petramond_world::block::ShapeState::id_bytes(HIGH_B);
    s.set_cell_state(
        1,
        2,
        3,
        petramond_world::block::ShapeState::with_ids(&[0b0111, a_lo, a_hi, b_lo, b_hi], 0b0_1010),
    );

    // Explicit identity palette, for the same reason as the cube test above:
    // a real save palette only pins ids the registry actually has.
    let pal = crate::save::palette::Palette::identity();
    let snap = SectionSnapshot::from_section(&s);
    let rec = encode_snapshot_with(&snap, &pal);
    let (back, ..) = decode_section_with(SectionPos::new(2, -1, 3), &rec, &pal).expect("decodes");

    assert_eq!(back.block_raw(1, 2, 3), HIGH_A);
    assert_eq!(back.block_raw(4, 5, 6), HIGH_B);
    assert_eq!(back.block_raw(7, 8, 9), Block::Stone.id());
    let state = back
        .cell_states()
        .get(&(petramond_world::chunk::section_idx(1, 2, 3) as u16))
        .copied()
        .expect("cell state persisted");
    assert_eq!(
        (state.id_at(1), state.id_at(3)),
        (HIGH_A, HIGH_B),
        "id-masked state bytes carry whole two-byte block ids"
    );
    assert_eq!(state.byte(0), 0b0111, "non-id state bytes are untouched");
}
