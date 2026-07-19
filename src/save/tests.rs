use super::worlds::delete_world_at;
use super::*;
use crate::block::Block;
use crate::item::{ItemStack, ItemType};
use crate::mathh::Vec3;
use crate::player::Player;

fn temp_world_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("petramond-savetest-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn load_blocking(save: &WorldSave, pos: SectionPos) -> Option<LoadedSection> {
    save.request_load(pos, true);
    for _ in 0..500 {
        if let Some(l) = save.poll_loaded() {
            return Some(l);
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
            opened.save.load_player("Rachel S!").is_none(),
            "fresh world has no player files"
        );
        assert!(!opened.save.manifest_contains(pos));

        let mut section = Section::new(pos.cx, pos.cy, pos.cz);
        section.set_block(3, 0, 7, Block::Stone);
        section.set_water(3, 1, 7, Block::Water, 0x12);
        let mut snap = SectionSnapshot::from_section(&section);
        let mut drop = DroppedItem::new(
            Vec3::new(80.5, 70.0, -39.5),
            ItemStack::new(ItemType::Dirt, 9),
            1,
        );
        drop.ticks_lived = 2500;
        snap.entities.push(drop);
        opened.save.save_sections(vec![snap]);

        opened.save.save_level(level::encode(
            0xABCD,
            4242,
            &Default::default(),
            &Default::default(),
        ));

        // The player rides its own file, keyed by SANITIZED name — the
        // display name may contain anything.
        let mut plr = Player::new(Vec3::new(80.0, 70.0, -40.0));
        plr.inventory.set_active(4);
        opened.save.save_player("Rachel S!", player::encode(&plr));

        opened.save.shutdown(); // flush queued writes + join the I/O thread
    }

    {
        let opened = open_at(dir.clone()).expect("reopen");

        let level = opened.level.expect("level.dat restored");
        assert_eq!(level.seed, 0xABCD);
        assert_eq!(level.tick, 4242, "the world tick persists across sessions");

        let restored = opened
            .save
            .load_player("Rachel S!")
            .and_then(|b| player::decode(&b))
            .expect("player file restored under the same (sanitized) name");
        assert_eq!(restored.pos, Vec3::new(80.0, 70.0, -40.0));
        assert_eq!(restored.inventory.active_slot(), 4);

        assert!(
            opened.save.manifest_contains(pos),
            "manifest sees saved section"
        );

        let loaded = load_blocking(&opened.save, pos).expect("section loads from disk");
        let section = loaded.section.expect("section record decodes");
        assert_eq!(section.block_raw(3, 0, 7), Block::Stone.id());
        assert_eq!(section.block_raw(3, 1, 7), Block::Water.id());
        assert_eq!(section.water_meta(3, 1, 7), 0x12);

        // The item entity comes back with its section, lifetime intact.
        assert_eq!(loaded.entities.len(), 1);
        assert_eq!(loaded.entities[0].stack, ItemStack::new(ItemType::Dirt, 9));
        assert_eq!(
            loaded.entities[0].ticks_lived, 2500,
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
        opened
            .save
            .save_sections(vec![cached_snap, SectionSnapshot::from_section(&edited)]);

        assert!(opened.save.explored_manifest_contains(cached_pos));
        assert!(!opened.save.authoritative_manifest_contains(cached_pos));
        assert_eq!(
            opened
                .save
                .manifest_sections_in_column(cached_pos.chunk_pos())
                .collect::<Vec<_>>(),
            vec![edited_pos],
            "disposable cache sections must not widen the wanted vertical range"
        );
        opened.save.shutdown();
    }

    {
        let opened = open_at(dir.clone()).expect("reopen");
        assert!(opened.save.explored_manifest_contains(cached_pos));
        assert!(!opened.save.authoritative_manifest_contains(cached_pos));
        assert!(opened.save.authoritative_manifest_contains(edited_pos));
        let loaded = load_blocking(&opened.save, cached_pos).expect("cache section loads");
        assert_eq!(
            loaded
                .section
                .expect("cache record decodes")
                .block_raw(2, 3, 4),
            Block::Stone.id()
        );
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
        Vec3::new(33.0, 65.0, -63.0),
        ItemStack::new(ItemType::Dirt, 3),
        1,
    ));
    opened.save.save_sections(vec![snap]);
    assert!(
        opened.save.record_holds_entities(pos),
        "record now carries a drop"
    );

    let with_item = load_blocking(&opened.save, pos).expect("loads with item");
    assert_eq!(with_item.entities.len(), 1, "drop is present before pickup");

    // Pickup-then-unload: the section is re-saved with no drops. The channel is
    // ordered, so this write lands before the load below reads it back.
    let empty = SectionSnapshot::from_section(&section); // entities default to empty
    opened.save.save_sections(vec![empty]);
    assert!(
        !opened.save.record_holds_entities(pos),
        "rewrite cleared the flag"
    );

    let after = load_blocking(&opened.save, pos).expect("loads after pickup");
    assert!(
        after.entities.is_empty(),
        "the stale drop must not resurrect"
    );
    // The section's own edits survive the rewrite (only the drop was cleared).
    assert_eq!(
        after.section.expect("section decodes").block_raw(1, 0, 1),
        Block::Stone.id()
    );

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
        pos: Vec3::new(-100.5, 65.0, 56.5),
        yaw: 0.5,
        tags: Default::default(),
    });
    opened.save.save_sections(vec![snap]);
    assert!(
        opened.save.record_holds_entities(pos),
        "record now carries a mob"
    );

    let with_mob = load_blocking(&opened.save, pos).expect("loads with mob");
    assert_eq!(with_mob.mobs.len(), 1, "mob present before it leaves");

    // The mob is gone: the section is re-saved mob-free. The record must be rewritten
    // so the stale mob can't resurrect on the next load.
    let empty = SectionSnapshot::from_section(&section);
    opened.save.save_sections(vec![empty]);
    assert!(
        !opened.save.record_holds_entities(pos),
        "rewrite cleared the flag"
    );

    let after = load_blocking(&opened.save, pos).expect("loads after");
    assert!(after.mobs.is_empty(), "the stale mob must not resurrect");

    opened.save.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}
