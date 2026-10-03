use super::*;
use crate::world::ServerWorld;

use std::sync::Arc;

use petramond_math::math::IVec3;
use petramond_world::block::Block;
use petramond_world::chunk::{
    Chunk, ChunkPos, SectionPos, SEA_LEVEL, SECTION_MAX_CY, SECTION_SIZE,
};
use petramond_world::section::Section;

use crate::world::store::{LoadAnchor, LoadTarget};

mod priorities;

#[test]
fn overlaid_saved_section_keeps_its_block_entities_live() {
    let mut world = ServerWorld::new(0, 4);
    let sp = SectionPos::new(0, 4, 0);
    world.data.ensure_column(sp.chunk_pos());
    world
        .data
        .sections
        .insert(sp, Arc::new(Section::new(0, 4, 0)));
    world.note_section_loaded(sp);
    let mut saved = Section::new(0, 4, 0);
    saved.set_block(0, 0, 0, petramond_world::block::Block::Chest);
    saved.insert_container(
        0,
        0,
        0,
        petramond_world::container::Container::with_len(crate::world::chest::CHEST_SLOTS),
    );
    saved.insert_entity_facing(0, 0, 0, petramond_math::facing::Facing::default());
    world
        .side
        .gen
        .pending_overlays
        .insert(sp, (saved, Vec::new(), Vec::new()));
    world.apply_pending_overlays();

    let mut out = Vec::new();
    world.collect_animated_blocks(&mut out);
    assert_eq!(out.len(), 1, "the overlaid chest must be collected");
}

#[test]
fn mob_census_waits_for_nearby_columns_and_overlays_only() {
    let mut world = ServerWorld::new(0, 2);
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
    world.side.gen.awaited_overlays.insert(near);
    world.note_stream_nonfinal(near);
    assert!(!world.mob_census_loaded_around(center, census_radius));
    world.side.gen.awaited_overlays.clear();

    let far = SectionPos::new(20, 4, 0);
    world.side.gen.awaited_overlays.insert(far);
    world.note_stream_nonfinal(far);
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

    let s4 = sections.iter().find(|(cy, _)| *cy == 4).expect("cy 4");
    assert_eq!(s4.1.block_raw(1, 0, 2), Block::Stone.id());
    let below = sections.iter().find(|(cy, _)| *cy == -1).expect("cy -1");
    assert_eq!(below.1.block_raw(0, 0, 0), Block::Stone.id());
    assert_eq!(below.1.block_raw(8, 8, 8), Block::Stone.id());
}

#[test]
fn generated_water_metadata_survives_the_split() {
    let mut chunk = Chunk::new(0, 0);
    chunk.set_block(5, 64, 5, Block::Stone);
    chunk.set_fluid(5, 65, 5, Block::Water, 0x07);
    let (_column, sections) = split_generated_column(&chunk);
    let s4 = sections.iter().find(|(cy, _)| *cy == 4).expect("cy 4");
    assert_eq!(s4.1.block_raw(5, 1, 5), Block::Water.id());
    assert_eq!(s4.1.fluid_meta(5, 1, 5), 0x07, "falloff metadata carried");
}

#[test]
fn water_kick_queues_source_water_over_a_drop() {
    let mut world = ServerWorld::new(0, 0);
    let mut section = Section::new(0, 4, 0);
    for z in 0..SECTION_SIZE {
        for x in 0..SECTION_SIZE {
            section.set_block(x, 0, z, Block::Stone);
        }
    }
    section.set_block(4, 0, 4, Block::Air);
    section.set_fluid(4, 1, 4, Block::Water, 0);
    world.insert_section_for_test(SectionPos::new(0, 4, 0), section);

    world.queue_loaded_section_fluid_updates(&[SectionPos::new(0, 4, 0)]);
    assert!(
        !world.queue_block_update(IVec3::new(4, 65, 4)),
        "water over a loaded air drop is kicked into flowing"
    );
    assert!(world.queue_block_update(IVec3::new(0, 65, 0)));
}

#[test]
fn high_flight_still_wants_the_surface_band() {
    let generator = petramond_worldgen::ChunkGenerator::new(0x51EED);
    let col = generator.generate_column_gen(0, 0);
    let cys = ServerWorld::wanted_section_cys(&col, SECTION_MAX_CY + 100, 0);
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

#[test]
fn multi_anchor_requests_and_keeps_both_neighbourhoods() {
    let mut world = ServerWorld::new(0, 4);
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
    world.insert_empty_column_for_test(ChunkPos::new(20, 0));

    world.update_load_multi(&[a, b]);

    let near = |p: &ChunkPos, cx: i32| (p.cx - cx).abs() <= 4 && p.cz.abs() <= 4;
    assert!(
        world.side.gen.pending.keys().any(|p| near(p, 0)),
        "anchor A's columns are requested"
    );
    assert!(
        world.side.gen.pending.keys().any(|p| near(p, 40)),
        "anchor B's columns are requested"
    );
    assert!(
        world
            .side
            .gen
            .pending
            .keys()
            .all(|p| near(p, 0) || near(p, 40)),
        "nothing outside the anchors' union is requested"
    );

    assert!(world.data.chunk_loaded(0, 0), "anchor A's column is kept");
    assert!(world.data.chunk_loaded(40, 0), "anchor B's column is kept");
    assert!(
        !world.data.chunk_loaded(20, 0),
        "a column no anchor keeps is evicted"
    );
}

#[test]
fn settled_missing_scan_resumes_after_eviction() {
    let mut world = ServerWorld::new(0, 4);
    for _ in 0..100 {
        world.update_load(0, 4, 0);
        if world.data.missing_columns_settled {
            break;
        }
    }
    assert!(
        world.data.missing_columns_settled,
        "a fully requested disc settles the scan"
    );
    let victim = ChunkPos::new(0, 0);
    assert!(
        world.side.gen.pending.contains_key(&victim)
            || world.side.gen.column_gen.contains_key(&victim),
        "the player's own column is requested or loaded"
    );

    world.remove_column(victim);
    assert!(
        !world.data.missing_columns_settled,
        "eviction un-settles the scan"
    );
    world.update_load(0, 4, 0);
    assert!(
        world.side.gen.pending.contains_key(&victim),
        "the evicted column is re-requested by the next scan"
    );
}

#[test]
fn single_anchor_multi_load_matches_update_load() {
    let mut single = ServerWorld::new(0x51EED, 3);
    let mut multi = ServerWorld::new(0x51EED, 3);
    single.update_load(2, 5, -1);
    multi.update_load_multi(&[LoadAnchor {
        cx: 2,
        cy: 5,
        cz: -1,
        radius: 64,
    }]);

    assert_eq!(single.data.last_load_target, multi.data.last_load_target);
    assert!(multi.data.extra_load_targets.is_empty());
    let sorted = |w: &ServerWorld| {
        let mut p: Vec<ChunkPos> = w.side.gen.pending.keys().copied().collect();
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
        ServerWorld::column_wanted(target, ChunkPos::new(10, 0)),
        "positive X is wanted"
    );
    assert!(
        ServerWorld::column_wanted(target, ChunkPos::new(-10, 0)),
        "equal-distance negative X is wanted"
    );
    assert!(
        ServerWorld::column_wanted(target, ChunkPos::new(0, 16)),
        "the circular boundary is included"
    );
    assert!(
        !ServerWorld::column_wanted(target, ChunkPos::new(12, 12)),
        "the square corner outside the disc is excluded"
    );
    assert!(
        !ServerWorld::column_kept(target, ChunkPos::new(-20, 0)),
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
    let deep_near = SectionPos::new(0, 1, 0);
    let deep_far = SectionPos::new(6, 1, 0);
    let surface_far = SectionPos::new(12, 4, 0);

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

    let mut world = ServerWorld::new(0x51EED, 4);
    let target = LoadTarget::new(0, 4, 0, 4);
    world.data.last_load_target = Some(target);
    let generator = petramond_worldgen::ChunkGenerator::new(world.data.seed);
    for dz in -1..=1 {
        for dx in -1..=1 {
            let cp = ChunkPos::new(dx, dz);
            world
                .side
                .gen
                .column_gen
                .insert(cp, Arc::new(generator.generate_column_gen(dx, dz)));
            world.data.ensure_column(cp);
        }
    }

    let sp = SectionPos::new(0, 4, 0);
    let mut section = Section::new(0, 4, 0);
    section.set_block(0, 0, 0, Block::Stone);
    world.data.sections.insert(sp, Arc::new(section));
    world.note_section_loaded(sp);
    let generating = SectionPos::new(0, 5, 0);
    world.insert_pending_section(generating);
    world.data.light_deferred.insert(sp);

    world.flush_settled_deferred(target);
    assert!(
        world.data.light_deferred.contains(&sp),
        "a neighbour's gen is in flight: the first bake must wait"
    );
    assert!(
        !world.light_bakes.has_pending(),
        "no bake may be requested from a half-landed neighbourhood"
    );

    world.remove_pending_section(generating);
    world.flush_settled_deferred(target);
    assert!(
        !world.data.light_deferred.contains(&sp),
        "settled sections leave the deferred set"
    );
    assert!(
        world.light_bakes.has_pending(),
        "the single first bake fires on settle"
    );
}

#[test]
fn sealed_first_light_waits_for_player_proximity_then_bakes() {
    let mut world = ServerWorld::new(0, 0);
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
    let generator = petramond_worldgen::ChunkGenerator::new(world.data.seed);
    world.side.gen.column_gen.insert(
        center.chunk_pos(),
        Arc::new(generator.generate_column_gen(center.cx, center.cz)),
    );

    let far = LoadTarget::new(8, 0, 0, 0);
    world.data.last_load_target = Some(far);
    world.data.light_deferred.insert(center);
    world.flush_settled_deferred(far);
    assert!(
        world.data.light_deferred.contains(&center),
        "an unreachable sealed cavity can leave its first light deferred"
    );
    assert!(!world.light_bakes.has_pending());

    let near = LoadTarget::new(0, 0, 0, 0);
    world.data.last_load_target = Some(near);
    world.flush_settled_deferred(near);
    assert!(!world.data.light_deferred.contains(&center));
    assert!(
        world.light_bakes.has_pending(),
        "approaching the cavity must wake its first light bake"
    );
}

#[test]
fn stale_pending_columns_are_pruned_to_current_disc() {
    let mut world = ServerWorld::new(0, 16);
    let outside = ChunkPos::new(17, 0);
    let inside = ChunkPos::new(-10, 0);
    world.side.gen.pending.insert(outside, None);
    world.side.gen.pending.insert(inside, None);

    let target = LoadTarget::new(0, 5, 0, 16);
    world.prune_stale_column_requests(target);

    assert!(
        !world.side.gen.pending.contains_key(&outside),
        "queued work outside the disc should be dropped"
    );
    assert!(
        world.side.gen.pending.contains_key(&inside),
        "queued work inside the disc stays queued"
    );
}

#[test]
fn horizontal_move_requests_sections_for_newly_wanted_loaded_columns() {
    use std::sync::Arc;

    let mut world = ServerWorld::new(0x51EED, 8);
    let old = LoadTarget::new(0, 5, 0, 8);
    let newly_wanted = ChunkPos::new(9, 0);
    assert!(
        !ServerWorld::column_wanted(old, newly_wanted),
        "test setup: column starts outside the old disc"
    );

    let generator = petramond_worldgen::ChunkGenerator::new(world.data.seed);
    let col = Arc::new(generator.generate_column_gen(newly_wanted.cx, newly_wanted.cz));
    world.set_column_gen(newly_wanted, col);
    world.data.last_load_target = Some(old);

    world.update_load(1, 5, 0);

    assert!(
        world
            .side
            .gen
            .pending_sections
            .iter()
            .any(|sp| sp.chunk_pos() == newly_wanted),
        "a generated column that enters the disc must request its sections"
    );
}

#[test]
fn cubic_world_generates_meshes_saves_and_reloads_an_edit() {
    use std::time::Instant;

    let dir = petramond_util::test_dirs::TestScratchDir::new("cubic-e2e");
    let opened = crate::save::open_at(dir.to_path_buf()).expect("open save");
    let pool = Arc::new(crate::worker::JobPool::new(
        crate::worker::JobPool::default_threads(),
    ));
    let mut world = ServerWorld::with_pool(0x51EED, 2, pool.clone());
    world.attach_save(opened.save, opened.saved);
    let mut replica = crate::world::ReplicaWorld::with_pool(0x51EED, 2, pool);
    let mut mirror = crate::world::ReplicaMirror::new(&mut world);
    let deadline = Instant::now() + petramond_util::test_time::TEST_HARD_DEADLINE;

    world.update_load(0, 8, 0);
    while !world.data.chunk_loaded(0, 0) {
        assert!(Instant::now() < deadline, "the origin column streamed in");
        world.poll();
    }

    while replica.iter_meshes().next().is_none() {
        assert!(Instant::now() < deadline, "at least one section meshed");
        world.poll();
        world.pump_light_bakes();
        mirror.sync(&mut world, &mut replica);
        replica.tick_mesh_budget(64);
    }

    let edit = IVec3::new(4, 250, 4);
    assert!(world.set_block_world(edit.x, edit.y, edit.z, Block::Stone));
    assert_eq!(
        world.data.chunk_block(edit.x, edit.y, edit.z),
        Block::Stone.id()
    );

    world.flush_modified_chunks();
    let sp = SectionPos::from_world(edit.x, edit.y, edit.z).unwrap();
    {
        assert!(
            world.data.saved_section_contains(sp),
            "edit's section is in the manifest"
        );
        let save = world.save().expect("save attached");
        save.request_load(world.data.saved_index(), sp, false);
        let mut got = None;
        while got.is_none() {
            assert!(Instant::now() < deadline, "section read back from disk");
            if let Some(l) = save.poll_loaded() {
                got = Some(l);
            }
        }
        let loaded = got.expect("section read back from disk");
        let crate::save::SectionRecord::Decoded { section, .. } = loaded.record else {
            panic!("section record decodes");
        };
        assert_eq!(
            section.block_raw(4, 250usize.rem_euclid(16), 4),
            Block::Stone.id(),
            "the edit persisted to disk"
        );
    }

    world.clear_world();
    world.data.last_load_target = None;
    world.update_load(0, 8, 0);
    while world.data.chunk_block(edit.x, edit.y, edit.z) != Block::Stone.id() {
        assert!(
            Instant::now() < deadline,
            "the saved edit overlaid back on after reload"
        );
        world.poll();
    }
    assert_eq!(
        world.data.chunk_block(edit.x, edit.y, edit.z),
        Block::Stone.id(),
        "the saved edit overlaid back on after reload"
    );
}

#[test]
fn explored_terrain_reloads_from_disk_without_generating() {
    let dir = petramond_util::test_dirs::TestScratchDir::new("explored-terrain");

    let stream_settled = |world: &mut ServerWorld| {
        use std::time::Instant;
        world.update_load(0, 8, 0);
        let deadline = Instant::now() + petramond_util::test_time::TEST_HARD_DEADLINE;
        loop {
            world.poll();
            if world.data.loaded_section_count() > 0
                && world.side.gen.pending.is_empty()
                && world.side.gen.pending_sections.is_empty()
                && world.side.gen.awaited_overlays.is_empty()
            {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "streaming settled (nothing pending)"
            );
        }
    };

    let light_settled = |world: &mut ServerWorld| {
        use std::time::Instant;
        let deadline = Instant::now() + petramond_util::test_time::TEST_HARD_DEADLINE;
        while Instant::now() < deadline {
            world.poll();
            world.pump_light_bakes();
            let done = world
                .data
                .sections
                .values()
                .all(|s| !s.light_dirty || s.all_opaque());
            if done {
                return true;
            }
        }
        false
    };

    let opened = crate::save::open_at(dir.to_path_buf()).expect("open save");
    let mut world = ServerWorld::new(0x51EED, 2);
    world.attach_save(opened.save, opened.saved);
    stream_settled(&mut world);
    assert!(light_settled(&mut world), "first-visit light bakes settle");
    let first_sections: Vec<SectionPos> = world.data.sections.keys().copied().collect();
    assert!(!first_sections.is_empty());
    let first_blocks: std::collections::HashMap<SectionPos, Vec<u16>> = first_sections
        .iter()
        .map(|sp| {
            (
                *sp,
                world.data.sections[sp].blocks_iter().collect::<Vec<_>>(),
            )
        })
        .collect();
    world.flush_modified_chunks();
    {
        for sp in &first_sections {
            assert!(
                world.data.saved_section_contains(*sp),
                "explored section {sp:?} must persist"
            );
        }
        let save = world.save().expect("save attached");
        assert!(
            save.colgen_manifest_contains(ChunkPos::new(0, 0)),
            "explored columns must enter the column-gen cache"
        );
    }
    drop(world);

    let opened = crate::save::open_at(dir.to_path_buf()).expect("reopen save");
    let mut world = ServerWorld::new(0x51EED, 2);
    world.attach_save(opened.save, opened.saved);
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
            .data
            .sections
            .get(sp)
            .unwrap_or_else(|| panic!("section {sp:?} reloaded"));
        assert_eq!(
            section.blocks_iter().collect::<Vec<_>>(),
            &blocks[..],
            "reloaded content diverged at {sp:?}"
        );
    }

    let relit = world
        .data
        .sections
        .values()
        .filter(|s| s.light_dirty && !s.all_opaque())
        .count();
    assert_eq!(
        relit, 0,
        "reloaded sections must keep their persisted light without re-baking"
    );
}

#[test]
fn vertical_window_generates_near_the_player_not_the_whole_column() {
    use std::time::Instant;

    let mut world = ServerWorld::new(0xC0FFEE, 1);
    let deep = (0, -60, 0);
    let surface = (0, 96, 0);

    world.update_load(0, 6, 0);
    let deadline = Instant::now() + petramond_util::test_time::TEST_HARD_DEADLINE;
    loop {
        world.poll();
        if world.data.loaded_section_count() > 0
            && world.side.gen.pending.is_empty()
            && world.side.gen.pending_sections.is_empty()
        {
            break;
        }
        assert!(Instant::now() < deadline, "the surface window streamed in");
    }
    assert!(
        world
            .data
            .section_loaded_at(surface.0, surface.1, surface.2),
        "a surface section streamed in around the player"
    );
    assert!(
        !world.data.section_loaded_at(deep.0, deep.1, deep.2),
        "the deep cave-space section is NOT generated while the player is at the surface"
    );

    world.update_load(0, -4, 0);
    let deadline = Instant::now() + petramond_util::test_time::TEST_HARD_DEADLINE;
    while !world.data.section_loaded_at(deep.0, deep.1, deep.2) && Instant::now() < deadline {
        world.poll();
    }
    assert!(
        world.data.section_loaded_at(deep.0, deep.1, deep.2),
        "the deep section streamed in once the player descended to it"
    );
}

mod sea_ice_streaming {
    use super::*;
    use petramond_world::block::Block;

    #[test]
    fn sea_ice_streams_into_the_live_world() {
        let mut world = ServerWorld::new(34, 2);
        world.update_load(6, 3, -1);
        let (wx, wy, wz) = (6 * 16 + 15, 63, -16 + 15);
        use std::time::Instant;
        let deadline = Instant::now() + petramond_util::test_time::TEST_HARD_DEADLINE;
        while !world.data.section_loaded_at(wx, wy, wz) {
            assert!(
                Instant::now() < deadline,
                "sea-ice section never streamed within the hard deadline"
            );
            world.poll();
        }
        let live = Block::from_id(world.data.chunk_block(wx, wy, wz));
        let oneshot = petramond_worldgen::generate_chunk(34, 6, -1);
        let expected = oneshot.block(15, 63, 15);
        assert_eq!(
            live, expected,
            "streamed world must match one-shot generation"
        );
    }
}

#[test]
fn sky_cavern_walks_under_an_overhang_and_stops_at_closed_walls() {
    let mut world = ServerWorld::new(0, 4);
    let generator = petramond_worldgen::ChunkGenerator::new(0);
    let shaft = ChunkPos::new(0, 0);
    let overhang = ChunkPos::new(1, 0);
    let wall = ChunkPos::new(0, 1);
    let behind = ChunkPos::new(0, 2);
    let mut shell_lo = i32::MAX;
    let mut cols = Vec::new();
    for cp in [shaft, overhang, wall, behind] {
        let col = Arc::new(generator.generate_column_gen(cp.cx, cp.cz));
        shell_lo = shell_lo.min(*ServerWorld::surface_window_for_column(&col, 0).start());
        world.set_column_gen(cp, Arc::clone(&col));
        cols.push((cp, col));
    }
    let cavern_cy = shell_lo - 1;
    let section = |cp: ChunkPos, cy: i32, fill: Block| {
        let mut s = Section::new(cp.cx, cy, cp.cz);
        s.blocks_mut().fill(fill.id());
        s.recompute_opaque_count();
        s
    };
    for (cp, col) in &cols {
        let shell = ServerWorld::surface_window_for_column(col, 0);
        let top = col.content_top().div_euclid(SECTION_SIZE as i32);
        for cy in cavern_cy..=top.min(*shell.end()) {
            let fill = if *cp == shaft {
                Block::Air
            } else {
                Block::Stone
            };
            world
                .insert_section_for_test(SectionPos::new(cp.cx, cy, cp.cz), section(*cp, cy, fill));
        }
    }
    for cp in [overhang, behind] {
        world.insert_section_for_test(
            SectionPos::new(cp.cx, cavern_cy, cp.cz),
            section(cp, cavern_cy, Block::Air),
        );
    }
    for (cp, _) in &cols {
        world.recompute_column_heightmaps(*cp);
    }

    let ingested: Vec<SectionPos> = world.data.sections.keys().copied().collect();
    let columns = cols.iter().map(|(cp, _)| *cp).collect();
    world.grow_sky_caverns(&ingested, &columns, LoadTarget::new(0, shell_lo + 6, 0, 4));

    let at = |cp: ChunkPos| SectionPos::new(cp.cx, cavern_cy, cp.cz);
    assert!(
        world.sky_cavern_contains(at(overhang)),
        "cavern floor under the neighbouring column's rock is seen through the opening"
    );
    assert!(
        world.sky_cavern_contains(at(wall)),
        "a closed wall section is seen: its faces are what the sightline hits"
    );
    assert!(
        !world.sky_cavern_contains(at(behind)),
        "nothing behind a closed wall is seen"
    );
    let below = SectionPos::new(shaft.cx, cavern_cy - 1, shaft.cz);
    assert!(
        world.side.gen.pending_sections.contains(&below),
        "the unloaded floor under the open cavern is requested"
    );
}
