use super::worlds::delete_world_at;
use super::*;
use crate::player::Player;
use petramond_math::world_pos::WorldPos;
use petramond_world::block::Block;
use petramond_world::item::{ItemStack, ItemType};

const RACHEL: crate::net::identity::PlayerKey = crate::net::identity::PlayerKey([0x2A; 32]);

/// A pre-identity `players/<name>.dat` moves to the FIRST identity that
/// claims the name; nobody else can adopt it afterwards, and an identity's
/// own file is never overwritten by a legacy one.
#[test]
fn legacy_player_files_are_adopted_once_by_the_first_claimant() {
    let dir = temp_world_dir("legacy-adopt");
    let opened = open_at(dir.clone()).expect("open fresh");
    std::fs::create_dir_all(dir.join("players")).expect("players dir");
    let legacy = |slot| {
        let mut plr = Player::new(WorldPos::new(1.0, 70.0, 2.0));
        plr.inventory.set_active(slot);
        player::encode(&plr, &palette::Palette::identity())
    };
    std::fs::write(dir.join("players/Ann_.dat"), legacy(3)).expect("legacy file");
    std::fs::write(dir.join("players/Bob.dat"), legacy(5)).expect("legacy file");
    let (a, b) = (
        crate::net::identity::PlayerKey([1; 32]),
        crate::net::identity::PlayerKey([2; 32]),
    );
    let active = |data: Option<player::PlayerData>| data.map(|d| d.inventory.active_slot());

    assert_eq!(
        active(
            opened
                .save
                .adopt_legacy_player("Ann ", &a)
                .expect("decodes")
        ),
        Some(3),
        "the name sanitizes to the legacy file's key"
    );
    assert_eq!(
        active(opened.save.load_player(&a).expect("decodes")),
        Some(3)
    );
    assert!(
        !dir.join("players/Ann_.dat").exists(),
        "the legacy file moved"
    );
    assert!(
        matches!(opened.save.adopt_legacy_player("Ann ", &b), Ok(None)),
        "a second claimant finds nothing"
    );
    assert!(
        matches!(opened.save.adopt_legacy_player("Bob", &a), Ok(None)),
        "an identity with its own file never adopts another"
    );
    assert!(dir.join("players/Bob.dat").exists());

    assert_eq!(opened.save.load_player_registry(), None);
    let files = opened.save.player_files();
    assert!(files
        .store_player_registry(2, b"{}")
        .expect("registry writes"));
    assert_eq!(
        opened.save.load_player_registry().as_deref(),
        Some(&b"{}"[..])
    );
    assert!(
        !files
            .store_player_registry(1, br#"{"stale": "x"}"#)
            .expect("a stale generation is not an error"),
        "an older registry snapshot never replaces a newer one"
    );
    assert_eq!(
        opened.save.load_player_registry().as_deref(),
        Some(&b"{}"[..])
    );
    drop(files);
    let mut save = opened.save;
    save.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

fn temp_world_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("petramond-savetest-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// Read `pos` back and decode it; `None` if the read never answers. A
/// record that is absent or unreadable fails the test.
fn load_blocking(
    save: &WorldSave,
    saved: &crate::world::SavedIndex,
    pos: SectionPos,
) -> Option<(Section, Vec<DroppedItem>, Vec<SavedMob>)> {
    save.request_load(saved, pos, true);
    for _ in 0..500 {
        if let Some(l) = save.poll_loaded() {
            return match l.record {
                SectionRecord::Decoded {
                    section,
                    entities,
                    mobs,
                } => Some((*section, entities, mobs)),
                SectionRecord::Absent => panic!("no record for {pos:?}"),
                SectionRecord::Unreadable(u) => panic!("unreadable record for {pos:?}: {u:?}"),
            };
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    None
}

/// Full disk round-trip through the I/O thread: write a modified section (with a
/// resting item entity carrying a partly-elapsed lifetime) + level + a player
/// file in one session, reopen in another, and read it all back. Item entities
/// ride in the section record, so the drop returns when its section loads.
#[test]
fn save_reopen_roundtrips_section_level_entities() {
    let dir = temp_world_dir("roundtrip");
    let pos = SectionPos::new(5, -3, -9); // negative cy: below the old datum

    {
        let mut opened = open_at(dir.clone()).expect("open fresh");
        assert!(opened.level.is_none(), "fresh world has no level.dat");
        assert!(
            matches!(opened.save.load_player(&RACHEL), Ok(None)),
            "fresh world has no player files"
        );
        assert!(!opened.saved.contains(pos));

        let mut section = Section::new(pos.cx, pos.cy, pos.cz);
        section.set_block(3, 0, 7, Block::Stone);
        section.set_fluid(3, 1, 7, Block::Water, 0x12);
        let mut snap = SectionSnapshot::from_section(&section);
        let mut drop = DroppedItem::new(
            WorldPos::new(80.5, 70.0, -39.5),
            ItemStack::new(ItemType::Dirt, 9),
            1,
        );
        drop.ticks_lived = 2500;
        snap.entities.push(drop);
        opened.save.save_sections(&mut opened.saved, vec![snap]);

        opened.save.save_level(level::encode(
            0xABCD,
            4242,
            &Default::default(),
            &Default::default(),
        ));

        // The player rides its own file, keyed by identity.
        let mut plr = Player::new(WorldPos::new(80.0, 70.0, -40.0));
        plr.inventory.set_active(4);
        opened.save.save_player(&RACHEL, &plr);

        opened.save.shutdown(); // flush queued writes + join the I/O thread
    }

    {
        let opened = open_at(dir.clone()).expect("reopen");

        let level = opened.level.expect("level.dat restored");
        assert_eq!(level.seed, 0xABCD);
        assert_eq!(level.tick, 4242, "the world tick persists across sessions");

        let restored = opened
            .save
            .load_player(&RACHEL)
            .expect("player file decodes")
            .expect("player file restored under the same identity");
        assert_eq!(restored.pos, WorldPos::new(80.0, 70.0, -40.0));
        assert_eq!(restored.inventory.active_slot(), 4);

        assert!(opened.saved.contains(pos), "manifest sees saved section");

        let (section, entities, _) =
            load_blocking(&opened.save, &opened.saved, pos).expect("section loads from disk");
        assert_eq!(section.block_raw(3, 0, 7), Block::Stone.id());
        assert_eq!(section.block_raw(3, 1, 7), Block::Water.id());
        assert_eq!(section.fluid_meta(3, 1, 7), 0x12);

        // The item entity comes back with its section, lifetime intact.
        assert_eq!(entities.len(), 1);
        assert_eq!(entities[0].stack, ItemStack::new(ItemType::Dirt, 9));
        assert_eq!(
            entities[0].ticks_lived, 2500,
            "remaining lifetime persisted"
        );
    }

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn explored_cache_does_not_expand_the_authoritative_manifest() {
    let dir = temp_world_dir("explored-cache");
    let cached_pos = SectionPos::new(5, -3, 9);
    let edited_pos = SectionPos::new(5, 4, 9);

    {
        let mut opened = open_at(dir.clone()).expect("open fresh");
        let mut cached = Section::new(cached_pos.cx, cached_pos.cy, cached_pos.cz);
        cached.set_block(2, 3, 4, Block::Stone);
        let mut cached_snap = SectionSnapshot::from_section(&cached);
        cached_snap.cache_only = true;

        let mut edited = Section::new(edited_pos.cx, edited_pos.cy, edited_pos.cz);
        edited.set_block(6, 7, 8, Block::Dirt);
        opened.save.save_sections(
            &mut opened.saved,
            vec![cached_snap, SectionSnapshot::from_section(&edited)],
        );

        assert!(opened.saved.explored_contains(cached_pos));
        assert!(!opened.saved.authoritative_contains(cached_pos));
        assert_eq!(
            opened.saved.sections_in_column(cached_pos.chunk_pos()),
            &[edited_pos.cy],
            "disposable cache sections must not widen the wanted vertical range"
        );
        opened.save.shutdown();
    }

    {
        let opened = open_at(dir.clone()).expect("reopen");
        assert!(opened.saved.explored_contains(cached_pos));
        assert!(!opened.saved.authoritative_contains(cached_pos));
        assert!(opened.saved.authoritative_contains(edited_pos));
        let (section, ..) =
            load_blocking(&opened.save, &opened.saved, cached_pos).expect("cache section loads");
        assert_eq!(section.block_raw(2, 3, 4), Block::Stone.id());
    }

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn seed_text_accepts_numbers_and_hashes_strings() {
    assert_eq!(seed_from_text("12345"), 12345);
    assert_eq!(seed_from_text(" 12345 "), 12345);
    assert_eq!(
        seed_from_text("petramond"),
        seed_from_text("petramond"),
        "string seeds are stable"
    );
    assert_ne!(
        seed_from_text("petramond"),
        seed_from_text("Petramond"),
        "different strings choose different compatible seeds"
    );
}

#[test]
fn delete_world_removes_only_a_single_save_directory() {
    let saves = temp_world_dir("delete-world");
    let world = saves.join("My_World");
    std::fs::create_dir_all(world.join("region")).expect("create world dir");
    std::fs::write(world.join("level.dat"), b"level").expect("write level");

    delete_world_at(&saves, "My_World").expect("delete world");
    assert!(!world.exists(), "selected world directory is removed");

    let invalid = delete_world_at(&saves, "../outside").expect_err("reject nested path");
    assert_eq!(invalid.kind(), std::io::ErrorKind::InvalidInput);

    let _ = std::fs::remove_dir_all(&saves);
}

/// The unload/reload dupe, at the save layer: a section record written with a
/// drop, then re-saved drop-free (the drop was picked up), must not bring the
/// drop back on reload — and `record_holds_entities` must track the transition.
#[test]
fn re_saving_a_drop_free_section_clears_its_stale_record() {
    let dir = temp_world_dir("clear-stale-drops");
    let pos = SectionPos::new(2, 4, -4);

    let mut opened = open_at(dir.clone()).expect("open fresh");

    // Unload-with-item: the record is written carrying one drop.
    let mut section = Section::new(pos.cx, pos.cy, pos.cz);
    section.set_block(1, 0, 1, Block::Stone);
    let mut snap = SectionSnapshot::from_section(&section);
    snap.entities.push(DroppedItem::new(
        WorldPos::new(33.0, 65.0, -63.0),
        ItemStack::new(ItemType::Dirt, 3),
        1,
    ));
    opened.save.save_sections(&mut opened.saved, vec![snap]);
    assert!(
        opened.save.record_holds_entities(pos),
        "record now carries a drop"
    );

    let (_, entities, _) =
        load_blocking(&opened.save, &opened.saved, pos).expect("loads with item");
    assert_eq!(entities.len(), 1, "drop is present before pickup");

    // Pickup-then-unload: the section is re-saved with no drops. The channel is
    // ordered, so this write lands before the load below reads it back.
    let empty = SectionSnapshot::from_section(&section); // entities default to empty
    opened.save.save_sections(&mut opened.saved, vec![empty]);
    assert!(
        !opened.save.record_holds_entities(pos),
        "rewrite cleared the flag"
    );

    let (section, entities, _) =
        load_blocking(&opened.save, &opened.saved, pos).expect("loads after pickup");
    assert!(entities.is_empty(), "the stale drop must not resurrect");
    // The section's own edits survive the rewrite (only the drop was cleared).
    assert_eq!(section.block_raw(1, 0, 1), Block::Stone.id());

    opened.save.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

/// The same stale-record guard, for mobs: a section record written with a mob, then
/// re-saved mob-free (the mob died, wandered off, or distance-despawned), must not
/// bring the mob back on reload — and `record_holds_entities` must track it. The
/// guard is one mechanism shared with dropped items, so this pins it for the mob
/// path too.
#[test]
fn re_saving_a_mob_free_section_clears_its_stale_record() {
    let dir = temp_world_dir("clear-stale-mobs");
    let pos = SectionPos::new(-7, 4, 3);

    let mut opened = open_at(dir.clone()).expect("open fresh");

    // Unload-with-mob: the record is written carrying one mob.
    let section = Section::new(pos.cx, pos.cy, pos.cz);
    let mut snap = SectionSnapshot::from_section(&section);
    snap.mobs.push(crate::mob::SavedMob {
        kind: crate::mob::Mob::Owl,
        pos: WorldPos::new(-100.5, 65.0, 56.5),
        yaw: 0.5,
        tags: Default::default(),
        container: Default::default(),
    });
    opened.save.save_sections(&mut opened.saved, vec![snap]);
    assert!(
        opened.save.record_holds_entities(pos),
        "record now carries a mob"
    );

    let (.., mobs) = load_blocking(&opened.save, &opened.saved, pos).expect("loads with mob");
    assert_eq!(mobs.len(), 1, "mob present before it leaves");

    // The mob is gone: the section is re-saved mob-free. The record must be rewritten
    // so the stale mob can't resurrect on the next load.
    let empty = SectionSnapshot::from_section(&section);
    opened.save.save_sections(&mut opened.saved, vec![empty]);
    assert!(
        !opened.save.record_holds_entities(pos),
        "rewrite cleared the flag"
    );

    let (.., mobs) = load_blocking(&opened.save, &opened.saved, pos).expect("loads after");
    assert!(mobs.is_empty(), "the stale mob must not resurrect");

    opened.save.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

/// A `level.dat` that exists but does not decode must refuse the open: a
/// world treated as new would get a fresh seed and world KV saved over the
/// real ones.
#[test]
fn an_unreadable_level_dat_refuses_the_open_and_is_left_alone() {
    let dir = temp_world_dir("bad-level");
    std::fs::create_dir_all(&dir).unwrap();
    let mut bad = level::FORMAT.current.to_le_bytes().to_vec();
    bad.push(1);
    std::fs::write(dir.join("level.dat"), &bad).unwrap();
    let err = open_at(dir.clone()).err().expect("refused");
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
    assert_eq!(std::fs::read(dir.join("level.dat")).unwrap(), bad);
    let _ = std::fs::remove_dir_all(&dir);
}

/// An unreadable player file is an error, never a fresh player, and its
/// bytes are kept before anything can overwrite them. One from a newer build
/// is also never overwritten this session.
#[test]
fn an_unreadable_player_file_is_quarantined_not_respawned_over() {
    const PAT: crate::net::identity::PlayerKey = crate::net::identity::PlayerKey([7; 32]);
    let dir = temp_world_dir("bad-player");
    let mut opened = open_at(dir.clone()).expect("open fresh");
    let path = super::worlds::player_path(&dir.join("players"), &PAT);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();

    // A current version header over a truncated body.
    let mut garbage = player::FORMAT.current.to_le_bytes().to_vec();
    garbage.extend([1, 2]);
    std::fs::write(&path, &garbage).unwrap();
    assert!(matches!(
        opened.save.load_player(&PAT),
        Err(RecordError::Corrupt { .. })
    ));
    let kept = dir
        .join("quarantine")
        .join("players")
        .join(path.file_name().unwrap());
    assert_eq!(std::fs::read(&kept).unwrap(), garbage);

    let newer = (player::FORMAT.current + 1).to_le_bytes();
    std::fs::write(&path, newer).unwrap();
    assert!(matches!(
        opened.save.load_player(&PAT),
        Err(RecordError::Newer { .. })
    ));
    opened
        .save
        .save_player(&PAT, &Player::new(WorldPos::new(0.0, 70.0, 0.0)));
    opened.save.shutdown();
    assert_eq!(
        std::fs::read(&path).unwrap(),
        newer,
        "a newer build's player file is never saved over"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// A legacy (name-keyed) player file that does not decode goes through the
/// same typed-error path: it is quarantined and NEVER moved to the identity's
/// path, and when it cannot be kept aside the identity's saves are dropped so
/// a fresh player never shadows it.
#[test]
fn an_unreadable_legacy_player_file_is_not_migrated() {
    const ANN: crate::net::identity::PlayerKey = crate::net::identity::PlayerKey([9; 32]);
    let dir = temp_world_dir("bad-legacy");
    let mut opened = open_at(dir.clone()).expect("open fresh");
    std::fs::create_dir_all(dir.join("players")).expect("players dir");
    let legacy = dir.join("players/Ann.dat");
    let owned = super::worlds::player_path(&dir.join("players"), &ANN);

    let mut garbage = player::FORMAT.current.to_le_bytes().to_vec();
    garbage.extend([1, 2]);
    std::fs::write(&legacy, &garbage).unwrap();
    assert!(matches!(
        opened.save.adopt_legacy_player("Ann", &ANN),
        Err(RecordError::Corrupt { .. })
    ));
    assert_eq!(std::fs::read(&legacy).unwrap(), garbage, "never moved");
    assert!(!owned.exists());
    assert_eq!(
        std::fs::read(dir.join("quarantine/players/Ann.dat")).unwrap(),
        garbage
    );

    let newer = (player::FORMAT.current + 1).to_le_bytes();
    std::fs::write(&legacy, newer).unwrap();
    assert!(matches!(
        opened.save.adopt_legacy_player("Ann", &ANN),
        Err(RecordError::Newer { .. })
    ));
    opened
        .save
        .save_player(&ANN, &Player::new(WorldPos::new(0.0, 70.0, 0.0)));
    opened.save.shutdown();
    assert_eq!(std::fs::read(&legacy).unwrap(), newer);
    assert!(
        !owned.exists(),
        "a fresh player never shadows a legacy file from a newer build"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// A section whose record must not be overwritten drops out of the index
/// (generation stands in for it) and its saves are skipped, so the record on
/// disk survives the session.
#[test]
fn a_write_protected_section_is_never_saved_over() {
    let dir = temp_world_dir("protected");
    let pos = SectionPos::new(2, 4, -2);
    let mut original = Section::new(pos.cx, pos.cy, pos.cz);
    original.set_block(1, 1, 1, Block::Stone);
    {
        let mut opened = open_at(dir.clone()).expect("open fresh");
        opened.save.save_sections(
            &mut opened.saved,
            vec![SectionSnapshot::from_section(&original)],
        );
        opened.save.shutdown();
    }
    {
        let mut opened = open_at(dir.clone()).expect("reopen");
        assert!(opened.saved.authoritative_contains(pos));
        opened.save.note_section_unreadable(
            &mut opened.saved,
            pos,
            SectionStore::Authoritative,
            &Unreadable {
                error: RecordError::Newer {
                    format: codec::SECTION.name,
                    found: codec::SECTION.current + 1,
                    newest: codec::SECTION.current,
                },
                quarantined: None,
            },
        );
        assert!(!opened.saved.authoritative_contains(pos));
        let generated = Section::new(pos.cx, pos.cy, pos.cz);
        opened.save.save_sections(
            &mut opened.saved,
            vec![SectionSnapshot::from_section(&generated)],
        );
        assert!(
            !opened.saved.authoritative_contains(pos),
            "the protected save was dropped"
        );
        opened.save.shutdown();
    }
    let opened = open_at(dir.clone()).expect("reopen again");
    let (section, ..) =
        load_blocking(&opened.save, &opened.saved, pos).expect("original still on disk");
    assert_eq!(section.block_raw(1, 1, 1), Block::Stone.id());
    let _ = std::fs::remove_dir_all(&dir);
}

/// Two worlds open in one process at once, with palettes that disagree about
/// every block's disk id: each world's records map through its OWN palette,
/// so both read back the blocks they were saved with. (A process-wide
/// palette made the second open remap the first world's records.)
#[test]
fn two_open_worlds_each_map_through_their_own_palette() {
    let (dir_a, dir_b) = (temp_world_dir("two-a"), temp_world_dir("two-b"));
    // World A: its palette.json lists the blocks rotated by one past air, so
    // every disk id differs from world B's fresh (registry-order) palette.
    std::fs::create_dir_all(&dir_a).unwrap();
    let name = |v: serde_json::Value| v.as_str().expect("serde name").to_owned();
    let mut blocks: Vec<String> = Block::all()
        .iter()
        .map(|&b| name(serde_json::to_value(b).unwrap()))
        .collect();
    blocks[1..].rotate_left(1);
    let items: Vec<String> = ItemType::all()
        .iter()
        .map(|&i| name(serde_json::to_value(i).unwrap()))
        .collect();
    std::fs::write(
        dir_a.join("palette.json"),
        serde_json::json!({ "blocks": blocks, "items": items }).to_string(),
    )
    .unwrap();

    let pos = SectionPos::new(1, 4, 1);
    let mut section = Section::new(pos.cx, pos.cy, pos.cz);
    section.set_block(2, 2, 2, Block::Stone);
    section.set_block(3, 3, 3, Block::OakLog);
    {
        let mut a = open_at(dir_a.clone()).expect("open a");
        let mut b = open_at(dir_b.clone()).expect("open b");
        assert_ne!(
            a.save.palette().block_to_disk(Block::Stone.id()),
            b.save.palette().block_to_disk(Block::Stone.id()),
            "the two worlds really disagree about disk ids"
        );
        for opened in [&mut a, &mut b] {
            opened.save.save_sections(
                &mut opened.saved,
                vec![SectionSnapshot::from_section(&section)],
            );
        }
        a.save.shutdown();
        b.save.shutdown();
    }
    let a = open_at(dir_a.clone()).expect("reopen a");
    let b = open_at(dir_b.clone()).expect("reopen b");
    for opened in [&a, &b] {
        let (back, ..) = load_blocking(&opened.save, &opened.saved, pos).expect("loads");
        assert_eq!(back.block_raw(2, 2, 2), Block::Stone.id());
        assert_eq!(back.block_raw(3, 3, 3), Block::OakLog.id());
    }
    drop((a, b));
    let _ = std::fs::remove_dir_all(&dir_a);
    let _ = std::fs::remove_dir_all(&dir_b);
}

/// A mob whose mod is gone never reaches the world, yet survives its section
/// being loaded and saved again: the save holds it from the load and writes
/// it back with the section.
#[test]
fn a_mob_whose_mod_is_gone_survives_its_section_being_resaved() {
    let dir = temp_world_dir("kept-mobs");
    std::fs::create_dir_all(&dir).unwrap();
    // The world's first mob species belongs to a mod this build lacks.
    std::fs::write(
        dir.join("palette.json"),
        r#"{ "blocks": ["petramond:air"], "items": ["petramond:air"], "mobs": ["gonemod:phantom"] }"#,
    )
    .unwrap();
    let pos = SectionPos::new(1, 4, 1);
    let phantom = mobs::DiskMob {
        species: 0,
        pos: WorldPos::new(20.0, 70.0, 20.0),
        yaw: 0.5,
        tags: Default::default(),
        slots: Vec::new(),
        unknown: Default::default(),
    };
    {
        let mut opened = open_at(dir.clone()).expect("open");
        let mut snap = SectionSnapshot::from_section(&Section::new(pos.cx, pos.cy, pos.cz));
        snap.kept.mobs.push(phantom.clone());
        opened.save.save_sections(&mut opened.saved, vec![snap]);
        opened.save.shutdown();
    }
    {
        // Loaded: nothing to spawn. Saved again the way the world saves it,
        // from the section alone.
        let mut opened = open_at(dir.clone()).expect("reopen");
        let (section, _, mobs) =
            load_blocking(&opened.save, &opened.saved, pos).expect("section loads");
        assert!(mobs.is_empty(), "the phantom is not spawned");
        opened.save.save_sections(
            &mut opened.saved,
            vec![SectionSnapshot::from_section(&section)],
        );
        opened.save.shutdown();
    }
    let opened = open_at(dir.clone()).expect("reopen again");
    let bytes = region::RegionReader::open(&region::region_path(&dir.join("region"), 0, 0))
        .expect("region opens")
        .read_record(region::local_index(pos))
        .expect("record reads")
        .expect("record present");
    let decoded = codec::decode_section(pos, &bytes, opened.save.palette()).expect("decodes");
    assert_eq!(decoded.kept.mobs, [phantom], "still on disk, unchanged");
    drop(opened);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Opening a world whose last save had a mod that is missing now reports
/// it and backs up the small files before anything rewrites them.
#[test]
fn opening_with_a_mod_missing_backs_the_world_up_first() {
    let dir = temp_world_dir("missing-mod");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("mods.json"),
        r#"{ "mods": [{ "id": "gonemod", "version": "1.0" }] }"#,
    )
    .unwrap();
    std::fs::write(
        dir.join("level.dat"),
        level::encode(5, 10, &Default::default(), &Default::default()),
    )
    .unwrap();
    let opened = open_at(dir.clone()).expect("opens");
    assert_eq!(opened.missing_mods, ["gonemod"]);
    let backup = dir.join("backup").join("mods-missing-gonemod");
    assert!(backup.join("level.dat").exists());
    assert!(backup.join("palette.json").exists());
    assert!(backup.join("mods.json").exists());
    drop(opened);
    let _ = std::fs::remove_dir_all(&dir);
}
