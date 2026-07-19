use super::*;

use std::sync::Arc;

use crate::block::Block;
use crate::chunk::{Chunk, ChunkPos, SectionPos, SEA_LEVEL, SECTION_MAX_CY, SECTION_SIZE};
use crate::mathh::IVec3;
use crate::section::Section;

use crate::world::store::{LoadAnchor, LoadTarget, World};

/// A block entity arriving through the saved-section overlay path (not a live
/// placement) must land in the block-entity index, or it renders/ticks as if
/// it didn't exist after a reload.
#[test]
fn overlaid_saved_section_keeps_its_block_entities_live() {
    let mut world = World::new(0, 4);
    let sp = SectionPos::new(0, 4, 0);
    world.ensure_column(sp.chunk_pos());
    // The generated base the overlay replaces.
    world.sections.insert(sp, Arc::new(Section::new(0, 4, 0)));
    // A saved section carrying a chest lands from disk.
    let mut saved = Section::new(0, 4, 0);
    saved.set_block(0, 0, 0, crate::block::Block::Chest);
    saved.insert_container(
        0,
        0,
        0,
        crate::container::Container::with_len(crate::world::chest::CHEST_SLOTS),
    );
    saved.insert_entity_facing(0, 0, 0, crate::facing::Facing::default());
    world
        .pending_overlays
        .insert(sp, (saved, Vec::new(), Vec::new()));
    world.apply_pending_overlays();

    let mut out = Vec::new();
    world.collect_chests(&mut out);
    assert_eq!(out.len(), 1, "the overlaid chest must be collected");
}

/// The spawn census waits only for the nearby streamable neighborhood: a saved
/// mob record there must block caps, while unrelated far streaming must not.
#[test]
fn mob_census_waits_for_nearby_columns_and_overlays_only() {
    let mut world = World::new(0, 2);
    let center = ChunkPos::new(0, 0);
    let census_radius = 9;

    for dz in -2..=2 {
        for dx in -2..=2 {
            if dx * dx + dz * dz <= 4 {
                world.insert_empty_column_for_test(ChunkPos::new(dx, dz));
            }
        }
    }
    assert!(
        world.mob_census_loaded_around(center, census_radius),
        "every streamable nearby column is loaded"
    );

    let near = SectionPos::new(1, 4, 0);
    world.awaited_overlays.insert(near);
    assert!(!world.mob_census_loaded_around(center, census_radius));
    world.awaited_overlays.clear();

    let far = SectionPos::new(20, 4, 0);
    world.awaited_overlays.insert(far);
    assert!(
        world.mob_census_loaded_around(center, census_radius),
        "far streaming does not block the local census"
    );

    world.remove_column(ChunkPos::new(0, 1));
    assert!(
        !world.mob_census_loaded_around(center, census_radius),
        "a missing nearby column closes the gate"
    );
}

#[test]
fn split_keeps_surface_blocks_and_adds_stone_below() {
    let mut chunk = Chunk::new(0, 0);
    chunk.set_block(1, 64, 2, Block::Stone);
    chunk.set_block(3, 70, 4, Block::Grass);
    let (_column, sections) = split_generated_column(&chunk);

    // Surface block lands in section cy 4 (y 64) at local y 0.
    let s4 = sections.iter().find(|(cy, _)| *cy == 4).expect("cy 4");
    assert_eq!(s4.1.block_raw(1, 0, 2), Block::Stone.id());
    // Below-zero range is solid stone (room for caves).
    let below = sections.iter().find(|(cy, _)| *cy == -1).expect("cy -1");
    assert_eq!(below.1.block_raw(0, 0, 0), Block::Stone.id());
    assert_eq!(below.1.block_raw(8, 8, 8), Block::Stone.id());
}

#[test]
fn generated_water_metadata_survives_the_split() {
    let mut chunk = Chunk::new(0, 0);
    chunk.set_block(5, 64, 5, Block::Stone);
    chunk.set_water(5, 65, 5, Block::Water, 0x07);
    let (_column, sections) = split_generated_column(&chunk);
    let s4 = sections.iter().find(|(cy, _)| *cy == 4).expect("cy 4");
    assert_eq!(s4.1.block_raw(5, 1, 5), Block::Water.id());
    assert_eq!(s4.1.water_meta(5, 1, 5), 0x07, "falloff metadata carried");
}

#[test]
fn water_kick_queues_source_water_over_a_drop() {
    // A source-water cell with air directly below (and that section loaded) must be
    // kicked into flowing on load. Build the section directly (no set_block_world, so
    // nothing else queues an update) — local y 1 water over local y 0 air.
    let mut world = World::new(0, 0);
    let mut section = Section::new(0, 4, 0);
    for z in 0..SECTION_SIZE {
        for x in 0..SECTION_SIZE {
            section.set_block(x, 0, z, Block::Stone); // world y 64 floor
        }
    }
    section.set_block(4, 0, 4, Block::Air); // carve a hole at world (4,64,4)
    section.set_water(4, 1, 4, Block::Water, 0); // source water at world (4,65,4)
    world.insert_section_for_test(SectionPos::new(0, 4, 0), section);

    world.queue_loaded_section_water_updates(&[SectionPos::new(0, 4, 0)]);
    // The water over the carved hole has a loaded air neighbour below, so the kick
    // queued it: re-queuing the same cell now returns false (already pending).
    assert!(
        !world.queue_block_update(IVec3::new(4, 65, 4)),
        "water over a loaded air drop is kicked into flowing"
    );
    // A different, un-queued cell still returns true — the kick wasn't indiscriminate.
    assert!(world.queue_block_update(IVec3::new(0, 65, 0)));
}

#[test]
fn high_flight_still_wants_the_surface_band() {
    let generator = crate::worldgen::driver::ChunkGenerator::new(0x51EED);
    let col = generator.generate_column_gen(0, 0);
    let cys = World::wanted_section_cys(&col, SECTION_MAX_CY + 100, 0);
    let surface_cy = col
        .surf_range()
        .0
        .max(SEA_LEVEL)
        .div_euclid(SECTION_SIZE as i32);

    assert!(
        cys.contains(&SECTION_MAX_CY),
        "high flight still wants the clamped player/top window"
    );
    assert!(
        cys.contains(&surface_cy),
        "high flight must retain/generate the visible surface band"
    );
}

/// Two distant anchors: `update_load_multi` must request BOTH
/// neighbourhoods (nothing outside the union) and keep loaded content near
/// each anchor while evicting what no anchor wants.
#[test]
fn multi_anchor_requests_and_keeps_both_neighbourhoods() {
    let mut world = World::new(0, 4);
    let a = LoadAnchor {
        cx: 0,
        cy: 4,
        cz: 0,
        radius: 64,
    };
    let b = LoadAnchor {
        cx: 40,
        cy: 4,
        cz: 0,
        radius: 64,
    };
    world.insert_empty_column_for_test(ChunkPos::new(0, 0));
    world.insert_empty_column_for_test(ChunkPos::new(40, 0));
    world.insert_empty_column_for_test(ChunkPos::new(20, 0)); // far from both

    world.update_load_multi(&[a, b]);

    let near = |p: &ChunkPos, cx: i32| (p.cx - cx).abs() <= 4 && p.cz.abs() <= 4;
    assert!(
        world.pending.keys().any(|p| near(p, 0)),
        "anchor A's columns are requested"
    );
    assert!(
        world.pending.keys().any(|p| near(p, 40)),
        "anchor B's columns are requested"
    );
    assert!(
        world.pending.keys().all(|p| near(p, 0) || near(p, 40)),
        "nothing outside the anchors' union is requested"
    );

    assert!(world.chunk_loaded(0, 0), "anchor A's column is kept");
    assert!(world.chunk_loaded(40, 0), "anchor B's column is kept");
    assert!(
        !world.chunk_loaded(20, 0),
        "a column no anchor keeps is evicted"
    );
}

/// The settled short-circuit skips the per-pump missing-column rescan but
/// must never hide a column that became missing WITHOUT an anchor change
/// (eviction, failed gen job) — a stale flag here means terrain that never
/// loads again while the player stands still.
#[test]
fn settled_missing_scan_resumes_after_eviction() {
    let mut world = World::new(0, 4);
    // Repeated same-target updates submit the whole wanted disc (64 per
    // call) and then settle.
    for _ in 0..100 {
        world.update_load(0, 4, 0);
        if world.missing_columns_settled {
            break;
        }
    }
    assert!(
        world.missing_columns_settled,
        "a fully requested disc settles the scan"
    );
    let victim = ChunkPos::new(0, 0);
    assert!(
        world.pending.contains_key(&victim) || world.column_gen.contains_key(&victim),
        "the player's own column is requested or loaded"
    );

    // A column dropped without any anchor change must be re-found by the
    // next same-target scan.
    world.remove_column(victim);
    assert!(
        !world.missing_columns_settled,
        "eviction un-settles the scan"
    );
    world.update_load(0, 4, 0);
    assert!(
        world.pending.contains_key(&victim),
        "the evicted column is re-requested by the next scan"
    );
}

/// One anchor through `update_load_multi` IS `update_load`: same
/// target, same requested set, no multi-anchor residue.
#[test]
fn single_anchor_multi_load_matches_update_load() {
    let mut single = World::new(0x51EED, 3);
    let mut multi = World::new(0x51EED, 3);
    single.update_load(2, 5, -1);
    multi.update_load_multi(&[LoadAnchor {
        cx: 2,
        cy: 5,
        cz: -1,
        radius: 64,
    }]);

    assert_eq!(single.last_load_target, multi.last_load_target);
    assert!(multi.extra_load_targets.is_empty());
    let sorted = |w: &World| {
        let mut p: Vec<ChunkPos> = w.pending.keys().copied().collect();
        p.sort_by_key(|c| (c.cx, c.cz));
        p
    };
    assert_eq!(
        sorted(&single),
        sorted(&multi),
        "one anchor must request exactly the update_load set"
    );
}

#[test]
fn streaming_wants_a_full_horizontal_disc() {
    let target = LoadTarget::new(0, 5, 0, 16);

    assert!(
        World::column_wanted(target, ChunkPos::new(10, 0)),
        "positive X is wanted"
    );
    assert!(
        World::column_wanted(target, ChunkPos::new(-10, 0)),
        "equal-distance negative X is wanted"
    );
    assert!(
        World::column_wanted(target, ChunkPos::new(0, 16)),
        "the circular boundary is included"
    );
    assert!(
        !World::column_wanted(target, ChunkPos::new(12, 12)),
        "the square corner outside the disc is excluded"
    );
    assert!(
        !World::column_kept(target, ChunkPos::new(-20, 0)),
        "columns beyond circular unload hysteresis are evicted"
    );
}

#[test]
fn streaming_priority_is_distance_only() {
    let target = LoadTarget::new(0, 5, 0, 16);

    assert!(
        target.column_priority_key(ChunkPos::new(0, 2))
            < target.column_priority_key(ChunkPos::new(16, 0)),
        "near terrain must beat the far edge"
    );
    assert_eq!(
        target.column_priority_key(ChunkPos::new(6, 0)),
        target.column_priority_key(ChunkPos::new(-6, 0)),
        "opposite directions at the same distance have equal priority"
    );
    assert_eq!(
        target.column_priority_key(ChunkPos::new(6, 0)),
        target.column_priority_key(ChunkPos::new(0, 6)),
        "axes at the same distance have equal priority"
    );
}

#[test]
fn surface_bias_orders_the_surface_shell_before_below_band_sections() {
    let target = LoadTarget::new(0, 4, 0, 32);
    let band_lo = 3;
    let deep_near = SectionPos::new(0, 1, 0); // adjacent, below the band
    let deep_far = SectionPos::new(6, 1, 0);
    let surface_far = SectionPos::new(12, 4, 0); // half a render distance out

    assert!(
        target.surface_biased_section_key(surface_far, band_lo, false)
            < target.surface_biased_section_key(deep_near, band_lo, false),
        "an above-ground anchor streams the visible surface shell before \
         even an adjacent below-band section"
    );
    assert!(
        target.surface_biased_section_key(deep_near, band_lo, false)
            < target.surface_biased_section_key(deep_far, band_lo, false),
        "below-band sections keep their own nearest-first order"
    );
    assert!(
        target.surface_biased_section_key(deep_near, band_lo, true)
            < target.surface_biased_section_key(surface_far, band_lo, true),
        "an underground (caving) anchor keeps pure 3D nearest-first"
    );
    assert_eq!(
        target.surface_biased_section_key(surface_far, band_lo, false),
        target.section_priority_key(surface_far),
        "in-band sections are never penalized"
    );
}

#[test]
fn first_bake_defers_until_generation_neighborhood_settles() {
    use std::sync::Arc;

    let mut world = World::new(0x51EED, 4);
    let target = LoadTarget::new(0, 4, 0, 4);
    world.last_load_target = Some(target);
    let generator = crate::worldgen::driver::ChunkGenerator::new(world.seed);
    for dz in -1..=1 {
        for dx in -1..=1 {
            let cp = ChunkPos::new(dx, dz);
            world
                .column_gen
                .insert(cp, Arc::new(generator.generate_column_gen(dx, dz)));
            world.ensure_column(cp);
        }
    }

    // A fresh, never-lit section whose neighbour above is still generating.
    let sp = SectionPos::new(0, 4, 0);
    let mut section = Section::new(0, 4, 0);
    section.set_block(0, 0, 0, Block::Stone);
    world.sections.insert(sp, Arc::new(section));
    let generating = SectionPos::new(0, 5, 0);
    world.pending_sections.insert(generating);
    world.light_deferred.insert(sp);

    world.flush_settled_deferred(target);
    assert!(
        world.light_deferred.contains(&sp),
        "a neighbour's gen is in flight: the first bake must wait"
    );
    assert!(
        !world.light_bakes.has_pending(),
        "no bake may be requested from a half-landed neighbourhood"
    );

    // The neighbour lands (or is discarded): the neighbourhood is now settled —
    // every other absent neighbour belongs to a landed column that skipped it.
    world.pending_sections.remove(&generating);
    world.flush_settled_deferred(target);
    assert!(
        !world.light_deferred.contains(&sp),
        "settled sections leave the deferred set"
    );
    assert!(
        world.light_bakes.has_pending(),
        "the single first bake fires on settle"
    );
    assert!(
        !world.dirty_meshes.is_empty(),
        "the first mesh queues alongside the first bake"
    );
}

#[test]
fn sealed_first_light_waits_for_player_proximity_then_bakes() {
    let mut world = World::new(0, 0);
    let center = SectionPos::new(0, 0, 0);
    let mut cavity = Section::new(0, 0, 0);
    cavity.blocks_slice_mut().fill(Block::Stone.id());
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
        section.blocks_slice_mut().fill(Block::Stone.id());
        section.recompute_opaque_count();
        world.insert_section_for_test(pos, section);
    }
    let generator = crate::worldgen::driver::ChunkGenerator::new(world.seed);
    world.column_gen.insert(
        center.chunk_pos(),
        Arc::new(generator.generate_column_gen(center.cx, center.cz)),
    );

    let far = LoadTarget::new(8, 0, 0, 0);
    world.last_load_target = Some(far);
    world.light_deferred.insert(center);
    world.flush_settled_deferred(far);
    assert!(
        world.light_deferred.contains(&center),
        "an unreachable sealed cavity can leave its first light deferred"
    );
    assert!(!world.light_bakes.has_pending());

    let near = LoadTarget::new(0, 0, 0, 0);
    world.last_load_target = Some(near);
    world.flush_settled_deferred(near);
    assert!(!world.light_deferred.contains(&center));
    assert!(
        world.light_bakes.has_pending(),
        "approaching the cavity must wake its first light bake"
    );
}

#[test]
fn stale_pending_columns_are_pruned_to_current_disc() {
    let mut world = World::new(0, 16);
    let outside = ChunkPos::new(17, 0);
    let inside = ChunkPos::new(-10, 0);
    world.pending.insert(outside, None);
    world.pending.insert(inside, None);

    let target = LoadTarget::new(0, 5, 0, 16);
    world.prune_stale_column_requests(target);

    assert!(
        !world.pending.contains_key(&outside),
        "queued work outside the disc should be dropped"
    );
    assert!(
        world.pending.contains_key(&inside),
        "queued work inside the disc stays queued"
    );
}

#[test]
fn horizontal_move_requests_sections_for_newly_wanted_loaded_columns() {
    use std::sync::Arc;

    let mut world = World::new(0x51EED, 8);
    let old = LoadTarget::new(0, 5, 0, 8);
    let newly_wanted = ChunkPos::new(9, 0);
    assert!(
        !World::column_wanted(old, newly_wanted),
        "test setup: column starts outside the old disc"
    );

    let generator = crate::worldgen::driver::ChunkGenerator::new(world.seed);
    let col = Arc::new(generator.generate_column_gen(newly_wanted.cx, newly_wanted.cz));
    world.column_gen.insert(newly_wanted, col);
    world.last_load_target = Some(old);

    world.update_load(1, 5, 0);

    assert!(
        world
            .pending_sections
            .iter()
            .any(|sp| sp.chunk_pos() == newly_wanted),
        "a generated column that enters the disc must request its sections"
    );
}

/// The whole cubic pipeline in one go (worldgen-tests only — it runs the real gen +
/// save threads): a column streams in and meshes, a block edited into the open air
/// above the surface materializes its section, and after a flush + evict + reload the
/// edit comes back via the disk overlay. Generate → mesh → edit → save → reload.
#[cfg(feature = "worldgen-tests")]
#[test]
fn cubic_world_generates_meshes_saves_and_reloads_an_edit() {
    use std::time::Duration;

    let dir = std::env::temp_dir().join(format!("petramond-cubic-e2e-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let opened = crate::save::open_at(dir.clone()).expect("open save");
    let mut world = World::new(0x51EED, 2);
    world.attach_save(opened.save);

    // Stream the origin column: generate (worker) + ingest. The later edit lands well
    // above the active vertical window; reload coverage comes from the save manifest.
    world.update_load(0, 8, 0);
    let mut spun = 0;
    while !world.chunk_loaded(0, 0) && spun < 3000 {
        world.poll();
        std::thread::sleep(Duration::from_millis(2));
        spun += 1;
    }
    assert!(world.chunk_loaded(0, 0), "the origin column streamed in");

    // Mesh the loaded sections. Poll + sleep between budgets so the async light bakes
    // the mesher waits on can finish, exactly as they do between real frames (a tight
    // no-delay loop never lets the light pool produce a result).
    for _ in 0..400 {
        world.poll();
        world.tick_mesh_budget(64);
        if world.iter_meshes().next().is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(
        world.iter_meshes().next().is_some(),
        "at least one section meshed"
    );

    // Edit a block into the open air well above any terrain (max surface ~171): this
    // materializes section (0,15,0) on write.
    let edit = IVec3::new(4, 250, 4);
    assert!(world.set_block_world(edit.x, edit.y, edit.z, Block::Stone));
    assert_eq!(world.chunk_block(edit.x, edit.y, edit.z), Block::Stone.id());

    // Flush to disk, then wait for the save thread to drain by reading the section back
    // through a blocking load (the channel is ordered, so this trails the write).
    world.flush_modified_chunks();
    let sp = SectionPos::from_world(edit.x, edit.y, edit.z).unwrap();
    {
        let save = world.save().expect("save attached");
        assert!(
            save.manifest_contains(sp),
            "edit's section is in the manifest"
        );
        save.request_load(sp, false);
        let mut got = None;
        for _ in 0..1500 {
            if let Some(l) = save.poll_loaded() {
                got = Some(l);
                break;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        let loaded = got.expect("section read back from disk");
        let section = loaded.section.expect("section record decodes");
        assert_eq!(
            section.block_raw(4, 250usize.rem_euclid(16), 4),
            Block::Stone.id(),
            "the edit persisted to disk"
        );
    }

    // Evict everything, then re-stream: gen rebuilds the column and the saved section
    // overlays the edit back on.
    world.clear_world();
    world.last_load_target = None;
    world.update_load(0, 8, 0);
    let mut spun = 0;
    while world.chunk_block(edit.x, edit.y, edit.z) != Block::Stone.id() && spun < 3000 {
        world.poll();
        std::thread::sleep(Duration::from_millis(2));
        spun += 1;
    }
    assert_eq!(
        world.chunk_block(edit.x, edit.y, edit.z),
        Block::Stone.id(),
        "the saved edit overlaid back on after reload"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// Explored-terrain persistence end to end: a first visit persists every
/// explored section AND the column-gen cache on flush; a reload of the same
/// area installs everything from disk — every stream event is `Loaded`,
/// none `Generated` — with content identical to the first visit.
#[cfg(feature = "worldgen-tests")]
#[test]
fn explored_terrain_reloads_from_disk_without_generating() {
    use std::time::Duration;

    let dir =
        std::env::temp_dir().join(format!("petramond-explored-terrain-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);

    let stream_settled = |world: &mut World| {
        world.update_load(0, 8, 0);
        let mut settled = 0;
        let mut last = 0usize;
        for _ in 0..5000 {
            world.poll();
            let now = world.loaded_section_count();
            if now == last && now > 0 {
                settled += 1;
                if settled >= 100 {
                    break;
                }
            } else {
                settled = 0;
                last = now;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
    };

    // Every section's light settled: baked-and-clean, or fully opaque
    // (never bakes). The first-persist gate waits for exactly this.
    let light_settled = |world: &mut World| {
        for _ in 0..5000 {
            world.poll();
            world.pump_light_bakes();
            let done = world
                .sections
                .values()
                .all(|s| !s.light_dirty || s.all_opaque());
            if done {
                return true;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        false
    };

    // First visit: generate, then flush (autosave path) — the flag persists
    // every explored section and the column-gen cache.
    let opened = crate::save::open_at(dir.clone()).expect("open save");
    let mut world = World::new(0x51EED, 2);
    world.attach_save(opened.save);
    stream_settled(&mut world);
    assert!(light_settled(&mut world), "first-visit light bakes settle");
    let first_sections: Vec<SectionPos> = world.sections.keys().copied().collect();
    assert!(!first_sections.is_empty());
    let first_blocks: std::collections::HashMap<SectionPos, Vec<u8>> = first_sections
        .iter()
        .map(|sp| (*sp, world.sections[sp].blocks_slice().to_vec()))
        .collect();
    world.flush_modified_chunks();
    {
        let save = world.save().expect("save attached");
        for sp in &first_sections {
            assert!(
                save.manifest_contains(*sp),
                "explored section {sp:?} must persist"
            );
        }
        assert!(
            save.colgen_manifest_contains(ChunkPos::new(0, 0)),
            "explored columns must enter the column-gen cache"
        );
    }
    drop(world); // joins the save thread: everything is on disk.

    // Reload: same area must come back entirely from disk.
    let opened = crate::save::open_at(dir.clone()).expect("reopen save");
    let mut world = World::new(0x51EED, 2);
    world.attach_save(opened.save);
    world.set_stream_event_capture(true);
    stream_settled(&mut world);

    let events = world.take_stream_events();
    let generated = events
        .iter()
        .filter(|e| matches!(e, StreamEvent::Generated(_)))
        .count();
    let loaded = events
        .iter()
        .filter(|e| matches!(e, StreamEvent::Loaded(_)))
        .count();
    assert_eq!(
        generated, 0,
        "explored terrain must not regenerate on reload ({loaded} loaded)"
    );
    assert!(loaded > 0, "sections came back from disk");
    for (sp, blocks) in &first_blocks {
        let section = world
            .sections
            .get(sp)
            .unwrap_or_else(|| panic!("section {sp:?} reloaded"));
        assert_eq!(
            section.blocks_slice(),
            &blocks[..],
            "reloaded content diverged at {sp:?}"
        );
    }

    // Light persistence: every reloaded section came back with its saved
    // cubes ALREADY CLEAN — nothing above ever drained a bake for the
    // reloaded world, so a single dirty section here would mean the load
    // path re-queued a bake (the exact work persistence exists to skip).
    let relit = world
        .sections
        .values()
        .filter(|s| s.light_dirty && !s.all_opaque())
        .count();
    assert_eq!(
        relit, 0,
        "reloaded sections must keep their persisted light without re-baking"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// The defining S3 behaviour: worldgen runs per section, CLOSEST TO THE PLAYER. A
/// player at the surface streams the surface band but NOT the deep sections below
/// y=0 (the cave space); descending streams those deep sections in. Proves the
/// vertical window genuinely bounds generation in 3D rather than batching whole
/// 256-tall columns.
#[cfg(feature = "worldgen-tests")]
#[test]
fn vertical_window_generates_near_the_player_not_the_whole_column() {
    use std::time::{Duration, Instant};

    let mut world = World::new(0xC0FFEE, 1);
    // y=-60 is deep section cy=-4 (the would-be cave space); y=96 is the surface band.
    let deep = (0, -60, 0);
    let surface = (0, 96, 0);

    // Player near the surface (section cy 6): stream until a surface section lands.
    world.update_load(0, 6, 0);
    let deadline = Instant::now() + Duration::from_secs(30);
    while !world.chunk_loaded(0, 0) && Instant::now() < deadline {
        world.poll();
        std::thread::sleep(Duration::from_millis(2));
    }
    // Drain a few more polls so the whole window has a chance to stream in.
    for _ in 0..32 {
        world.poll();
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(
        world.section_loaded_at(surface.0, surface.1, surface.2),
        "a surface section streamed in around the player"
    );
    assert!(
        !world.section_loaded_at(deep.0, deep.1, deep.2),
        "the deep cave-space section is NOT generated while the player is at the surface"
    );

    // Descend to that deep section (cy -4): now it must stream in.
    world.update_load(0, -4, 0);
    let deadline = Instant::now() + Duration::from_secs(30);
    while !world.section_loaded_at(deep.0, deep.1, deep.2) && Instant::now() < deadline {
        world.poll();
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(
        world.section_loaded_at(deep.0, deep.1, deep.2),
        "the deep section streamed in once the player descended to it"
    );
}

#[cfg(all(test, feature = "worldgen-tests"))]
mod sea_ice_streaming {
    use super::*;
    use crate::block::Block;
    use std::time::Duration;

    /// The frozen sea exists in the LIVE streamed world, not just the one-shot
    /// generator: a waterline ice cell must survive the per-section pipeline
    /// AND the analytic section summaries (a section wrongly classified
    /// `FullWater` would never materialize and serve virtual water instead of
    /// its ice — the failure mode this pins). Seed 34 chunk (6,-1) local
    /// (15,15) holds sea ice at y = SEA_LEVEL.
    #[test]
    fn sea_ice_streams_into_the_live_world() {
        let mut world = World::new(34, 2);
        world.update_load(6, 3, -1);
        let (wx, wy, wz) = (6 * 16 + 15, 63, -16 + 15);
        // Give-up bound only (generous per WIKI/testing-and-verification.md —
        // a starved shared JobPool can stretch quiet stretches far past any
        // tight window; passing runs never wait this long).
        let mut spun = 0;
        while !world.section_loaded_at(wx, wy, wz) && spun < 30_000 {
            world.poll();
            std::thread::sleep(Duration::from_millis(2));
            spun += 1;
        }
        let live = Block::from_id(world.chunk_block(wx, wy, wz));
        let oneshot = crate::worldgen::generate_chunk(34, 6, -1);
        let expected = oneshot.block(15, 63, 15);
        assert_eq!(expected, Block::Ice, "the pinned column still freezes");
        assert_eq!(
            live, expected,
            "streamed world must match one-shot generation"
        );
    }
}
