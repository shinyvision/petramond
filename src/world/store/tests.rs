use crate::world::ServerWorld;
use std::sync::Arc;

use petramond_math::math::IVec3;
use petramond_mesh::ChunkMesh;
use petramond_world::block::Block;
use petramond_world::chunk::{
    ChunkPos, SectionPos, SECTION_MAX_CY, SECTION_MIN_CY, SECTION_SIZE, SECTION_VOLUME,
};
use petramond_world::section::Section;
use petramond_worldgen::ChunkGenerator;

fn install_column_summary(world: &mut ServerWorld, generator: &ChunkGenerator, pos: ChunkPos) {
    world.data.ensure_column(pos);
    world.set_column_gen(pos, Arc::new(generator.generate_column_gen(pos.cx, pos.cz)));
}

#[test]
fn failed_section_mut_lookup_does_not_pollute_the_random_tick_index() {
    let mut world = ServerWorld::new(0, 0);
    let absent = SectionPos::new(0, SECTION_MAX_CY + 1, 0);

    assert!(world.data.section_mut(absent).is_none());
    assert!(
        !world.data.random_tick_dirty.contains(&absent),
        "a failed boundary probe must not become derived-index work"
    );
}

#[test]
fn same_height_surface_swap_bumps_the_column_revision() {
    let mut world = ServerWorld::new(0, 0);
    let sp = SectionPos::new(0, 4, 0);
    let mut s = Section::new(0, 4, 0);
    s.set_block(8, 0, 8, Block::Stone);
    world.insert_section_for_test(sp, s);
    let column = world.data.ensure_column(sp.chunk_pos());
    column.set_surface_y(8, 8, 64);
    column.set_sky_cover_y(8, 8, 64);

    let before = world.data.column_payload_revision(sp.chunk_pos());
    world.set_block_world(8, 64, 8, Block::Dirt);
    assert_eq!(
        world.data.columns[&sp.chunk_pos()].surface_y(8, 8),
        64,
        "fixture: the swap must not move the heightmap"
    );
    assert_ne!(
        before,
        world.data.column_payload_revision(sp.chunk_pos()),
        "a same-height surface swap must move the column revision"
    );
}

#[test]
fn edits_in_total_darkness_skip_light_invalidation_entirely() {
    let mut world = ServerWorld::new(0, 4);
    let pos = SectionPos::new(0, 0, 0);
    let mut section = Section::new(0, 0, 0);
    section.blocks_mut().fill(Block::Stone.id());
    section.recompute_opaque_count();
    world.insert_section_for_test(pos, section);
    {
        let s = world.data.section_mut(pos).unwrap();
        s.set_skylight(vec![0u8; SECTION_VOLUME].into());
        s.set_blocklight(vec![petramond_world::light::LightRgb::ZERO; SECTION_VOLUME].into());
    }
    world.data.relight_demand.clear();
    assert!(
        !world.data.sections[&pos].light_dirty,
        "fixture: settled dark"
    );

    assert!(world.set_block_world(8, 8, 8, Block::Air));
    assert!(
        !world.data.sections[&pos].light_dirty,
        "no light can reach the opened cell, so nothing may invalidate"
    );
    assert!(world.data.relight_demand.is_empty());

    world
        .data
        .section_mut(pos)
        .unwrap()
        .set_skylight(vec![petramond_world::chunk::SKY_FULL; SECTION_VOLUME].into());
    assert!(world.set_block_world(8, 4, 8, Block::Air));
    assert!(
        world.data.sections[&pos].light_dirty,
        "a break beside lit cells must invalidate light"
    );
    assert!(world.data.relight_demand.contains(&pos));
}

#[test]
fn glass_raises_the_visible_surface_without_raising_sky_cover() {
    let mut world = ServerWorld::new(0, 0);
    let cp = ChunkPos::new(0, 0);

    assert!(world.set_block_world(8, 0, 8, Block::Stone));
    assert!(world.set_block_world(8, 64, 8, Block::Glass));

    let column = &world.data.columns[&cp];
    assert_eq!(column.surface_y(8, 8), 64);
    assert_eq!(
        column.sky_cover_y(8, 8),
        0,
        "clear glass must not hide the open shaft from the skylight planner"
    );
}

#[test]
fn eviction_racing_an_edit_relight_rewrites_the_record_lightless() {
    let dir = petramond_util::test_dirs::TestScratchDir::new("stale-light");
    let opened = crate::save::open_at(dir.to_path_buf()).expect("open save");
    let mut world = ServerWorld::new(0, 0);
    world.attach_save(opened.save, opened.saved);

    let a = SectionPos::new(0, 4, 0);
    let b = SectionPos::new(1, 4, 0);
    for &sp in &[a, b] {
        let mut s = Section::new(sp.cx, sp.cy, sp.cz);
        for z in 0..SECTION_SIZE {
            for x in 0..SECTION_SIZE {
                s.set_block(x, 0, z, Block::Stone);
            }
        }
        s.set_skylight(vec![0u8; SECTION_VOLUME].into());
        s.set_blocklight(vec![petramond_world::light::LightRgb::ZERO; SECTION_VOLUME].into());
        s.mark_light_clean();
        world.insert_section_for_test(sp, s);
        world.data.section_mut(sp).expect("loaded").modified = true;
    }
    world.flush_modified_chunks();
    assert!(
        world.data.saved_index().contains(b),
        "fixture: B's record is on disk"
    );
    assert!(
        !world.data.sections[&b].light_dirty,
        "fixture: B persisted with clean light"
    );

    world.set_block_world(15, 65, 8, Block::Stone);
    assert!(world.data.sections[&b].light_dirty);

    let snap = world
        .snapshot_section_for_save(b, Vec::new(), Vec::new(), false)
        .expect("an unmodified on-disk section with edit-dirtied light must rewrite");
    assert!(
        snap.skylight.is_none() && snap.blocklight.is_none(),
        "the rewrite must omit the stale cubes so reload rebakes"
    );

    drop(world);
}

#[test]
fn mesh_column_index_tracks_multiple_vertical_meshes() {
    let mut world = crate::world::ReplicaWorld::new(0, 0);
    let terrain = &mut world.side.terrain;
    let lower = SectionPos::new(4, 0, -2);
    let upper = SectionPos::new(4, 1, -2);
    let column = lower.chunk_pos();

    assert!(!terrain.column_has_mesh(column));
    terrain.install_mesh(lower, ChunkMesh::empty());
    terrain.install_mesh(upper, ChunkMesh::empty());
    assert!(terrain.column_has_mesh(column));
    let bits = terrain.mesh_column_cys[&column];
    assert_eq!(bits.count_ones(), 2);

    assert!(terrain.remove_mesh(lower));
    assert!(terrain.column_has_mesh(column));
    assert_eq!(terrain.mesh_column_cys[&column].count_ones(), 1);

    assert!(terrain.remove_mesh(upper));
    assert!(!terrain.column_has_mesh(column));
    assert!(!terrain.mesh_column_cys.contains_key(&column));
}

#[test]
fn virtual_full_opaque_summary_blocks_collision_without_raw_voxels() {
    use petramond_world::{section::SectionSummary, world::WorldData};
    let mut world = ServerWorld::new(0, 0);
    let pos = ChunkPos::new(0, 0);
    world.data.ensure_column(pos);
    let mut summaries = vec![SectionSummary::Unknown; WorldData::column_section_range().count()];
    summaries[0] = SectionSummary::FullOpaque;
    world
        .data
        .column_summaries
        .insert(pos, summaries.into_boxed_slice());

    let y = SECTION_MIN_CY * SECTION_SIZE as i32;
    assert_eq!(
        Block::from_id(world.data.chunk_block(0, y, 0)),
        Block::Air,
        "raw reads stay exact: absent voxel buffers still read as air"
    );
    assert_eq!(
        world.data.physics_block(0, y, 0),
        Block::Stone,
        "physics reads may use the generated full-opaque summary"
    );
    assert!(
        !world.data.collision_boxes_at(0, y, 0).is_empty(),
        "virtual full-opaque summary should collide as a full block"
    );
    assert!(
        !world.data.placement_cell_open(IVec3::new(0, y, 0)),
        "placement must not treat absent known-solid terrain as open air"
    );
}

#[test]
fn heightmap_recompute_preserves_generated_cave_mouth_surface() {
    let seed = 0x1234_5678;
    let generator = ChunkGenerator::new(seed);
    let mut found = None;

    'search: for cz in -8..=8 {
        for cx in -8..=8 {
            let col = Arc::new(generator.generate_column_gen(cx, cz));
            for z in 0..SECTION_SIZE {
                for x in 0..SECTION_SIZE {
                    let original = col.surface_y(x, z);
                    let cave_top = col.heightmap_surface_y(x, z);
                    if cave_top < original {
                        found = Some((ChunkPos::new(cx, cz), col, x, z, original, cave_top));
                        break 'search;
                    }
                }
            }
        }
    }

    let Some((cp, col, x, z, original, cave_top)) = found else {
        panic!("test seed/search window must contain at least one cave-mouth column");
    };

    let mut world = ServerWorld::new(seed, 0);
    world.data.ensure_column(cp);
    world.set_column_gen(cp, Arc::clone(&col));

    let cy = cave_top.div_euclid(SECTION_SIZE as i32);
    let sp = SectionPos::new(cp.cx, cy, cp.cz);
    let section = generator.generate_section(sp, &col);
    world.data.sections.insert(sp, Arc::new(section));
    world.note_section_loaded(sp);

    world.recompute_column_heightmaps(cp);

    assert_eq!(
        world.data.columns.get(&cp).unwrap().surface_y(x, z),
        cave_top,
        "heightmap refresh must not restore original pre-cave surface {original}"
    );
}

#[test]
fn heightmap_recompute_keeps_glass_out_of_direct_sky_cover() {
    let mut world = ServerWorld::new(0, 0);
    let cp = ChunkPos::new(0, 0);
    let ground = SectionPos::new(0, 0, 0);
    let roof = SectionPos::new(0, 4, 0);

    let mut ground_section = Section::new(0, 0, 0);
    ground_section.set_block(8, 0, 8, Block::Stone);
    world.data.sections.insert(ground, Arc::new(ground_section));
    world.note_section_loaded(ground);
    let mut roof_section = Section::new(0, 4, 0);
    roof_section.set_block(8, 0, 8, Block::Glass);
    world.data.sections.insert(roof, Arc::new(roof_section));
    world.note_section_loaded(roof);

    let column = world.data.ensure_column(cp);
    column.set_surface_y(8, 8, 64);
    column.set_sky_cover_y(8, 8, 64);

    assert!(world.recompute_column_heightmaps(cp).is_some());
    let column = &world.data.columns[&cp];
    assert_eq!(column.surface_y(8, 8), 64);
    assert_eq!(
        column.sky_cover_y(8, 8),
        0,
        "saved/streamed glass must remain clear when column maps are rebuilt"
    );
}

#[test]
fn heightmap_recompute_preserves_loaded_dug_shaft_below_generated_surface() {
    let seed = 0x51EED;
    let generator = ChunkGenerator::new(seed);
    let mut found = None;

    'search: for cz in -8..=8 {
        for cx in -8..=8 {
            let col = Arc::new(generator.generate_column_gen(cx, cz));
            for z in 0..SECTION_SIZE {
                for x in 0..SECTION_SIZE {
                    let ground = col.heightmap_surface_y(x, z);
                    let lower = ground - SECTION_SIZE as i32 - 1;
                    let wx = cx * SECTION_SIZE as i32 + x as i32;
                    let wz = cz * SECTION_SIZE as i32 + z as i32;
                    if SectionPos::from_world(wx, ground, wz).is_some()
                        && SectionPos::from_world(wx, lower, wz).is_some()
                    {
                        found = Some((ChunkPos::new(cx, cz), col, x, z, ground, lower));
                        break 'search;
                    }
                }
            }
        }
    }

    let Some((cp, col, x, z, ground, lower)) = found else {
        panic!("test seed/search window must contain a diggable surface column");
    };

    let mut world = ServerWorld::new(seed, 0);
    let column = world.data.ensure_column(cp);
    column.set_surface_y(x, z, ground);
    column.set_sky_cover_y(x, z, ground);
    world.set_column_gen(cp, col);

    let ground_sp = SectionPos::from_world(
        cp.cx * SECTION_SIZE as i32 + x as i32,
        ground,
        cp.cz * SECTION_SIZE as i32 + z as i32,
    )
    .unwrap();
    world.data.sections.insert(
        ground_sp,
        Arc::new(Section::new(cp.cx, ground_sp.cy, cp.cz)),
    );
    world.note_section_loaded(ground_sp);

    let lower_sp = SectionPos::from_world(
        cp.cx * SECTION_SIZE as i32 + x as i32,
        lower,
        cp.cz * SECTION_SIZE as i32 + z as i32,
    )
    .unwrap();
    let mut lower_section = Section::new(cp.cx, lower_sp.cy, cp.cz);
    lower_section.set_block(
        x,
        lower.rem_euclid(SECTION_SIZE as i32) as usize,
        z,
        Block::Stone,
    );
    world
        .data
        .sections
        .insert(lower_sp, Arc::new(lower_section));
    world.note_section_loaded(lower_sp);

    world.recompute_column_heightmaps(cp);

    assert_eq!(
        world.data.columns.get(&cp).unwrap().surface_y(x, z),
        lower,
        "a loaded dug shaft must not be covered again by the generated fallback"
    );
}

#[test]
fn removing_surface_cover_relights_loaded_sections_below_the_changed_section() {
    let dir = petramond_util::test_dirs::TestScratchDir::new("sky-cover-relight");
    let opened = crate::save::open_at(dir.to_path_buf()).expect("open save");
    let mut world = ServerWorld::new(0, 0);
    world.attach_save(opened.save, opened.saved);
    let cp = ChunkPos::new(0, 0);
    let shaft_x = 8;
    let shaft_z = 8;
    let cover_y = 64;
    let top = SectionPos::new(0, 4, 0);
    let lower = SectionPos::new(0, 2, 0);

    let column = world.data.ensure_column(cp);
    column.set_surface_y(shaft_x, shaft_z, cover_y);
    column.set_sky_cover_y(shaft_x, shaft_z, cover_y);

    let mut top_section = Section::new(top.cx, top.cy, top.cz);
    top_section.set_block(shaft_x, 0, shaft_z, Block::Dirt);
    top_section.set_skylight(vec![0u8; SECTION_VOLUME].into());
    top_section.set_blocklight(vec![petramond_world::light::LightRgb::ZERO; SECTION_VOLUME].into());
    top_section.dirty = false;

    let mut lower_section = Section::new(lower.cx, lower.cy, lower.cz);
    lower_section.set_skylight(vec![0u8; SECTION_VOLUME].into());
    lower_section
        .set_blocklight(vec![petramond_world::light::LightRgb::ZERO; SECTION_VOLUME].into());
    lower_section.dirty = false;

    world.data.sections.insert(top, Arc::new(top_section));
    world.note_section_loaded(top);
    world.data.sections.insert(lower, Arc::new(lower_section));
    world.note_section_loaded(lower);

    assert!(
        !world.data.sections.get(&lower).unwrap().light_dirty,
        "fixture lower section starts with settled dark skylight"
    );
    assert!(
        !world.data.sections.get(&lower).unwrap().dirty,
        "fixture lower section starts with no pending mesh work"
    );

    assert!(world.set_block_world(shaft_x as i32, cover_y, shaft_z as i32, Block::Air));

    assert!(
        world.data.sections.get(&lower).unwrap().light_dirty,
        "removing sky cover must invalidate skylight below the edited section"
    );
    assert!(
        world.data.light_edited_since_persist.contains(&lower),
        "distant light invalidation must be tracked in case eviction beats the rebake"
    );

    let mut landed = false;
    for _ in 0..2500 {
        world.pump_light_bakes();
        if !world.data.sections.get(&lower).unwrap().light_dirty {
            landed = true;
            break;
        }
    }
    assert!(landed, "the marked distant section must rebake unprompted");
    let lower_section = world.data.sections.get(&lower).unwrap();
    assert_eq!(
        lower_section.skylight_at(shaft_x, 8, shaft_z),
        petramond_world::chunk::SKY_FULL,
        "the opened shaft must reach full skylight below"
    );
    assert!(
        lower_section.dirty,
        "changed cubes must requeue the section's mesh"
    );

    drop(world);
}

#[test]
fn air_edit_into_absent_full_opaque_section_materializes_generated_base() {
    let seed = 0x51EED;
    let generator = ChunkGenerator::new(seed);
    let mut world = ServerWorld::new(seed, 0);
    install_column_summary(&mut world, &generator, ChunkPos::new(0, 0));

    let y = SECTION_MIN_CY * SECTION_SIZE as i32;
    let sp = SectionPos::from_world(0, y, 0).unwrap();
    assert!(
        !world.data.sections.contains_key(&sp),
        "the deep generated-solid section starts summary-only"
    );

    assert!(world.set_block_world(0, y, 0, Block::Air));
    assert!(
        world.data.sections.contains_key(&sp),
        "editing virtual solid materializes the generated section"
    );
    assert_eq!(Block::from_id(world.data.chunk_block(0, y, 0)), Block::Air);
    assert_ne!(
        Block::from_id(world.data.chunk_block(1, y, 0)),
        Block::Air,
        "materialization preserves the generated solid neighbours instead of creating an empty section"
    );
}

fn persisted(world: &ServerWorld, sp: SectionPos) -> bool {
    let saved = world.data.saved_index();
    saved.authoritative_contains(sp) || saved.explored_contains(sp)
}

/// The save flush examines only sections that changed since the last one; it must still write
/// exactly what a scan of every loaded section would (a first persist, an edit, an edit made
/// after a flush, a section that only persists once its light settles).
#[test]
fn a_save_flush_writes_every_section_a_full_scan_would() {
    let dir = petramond_util::test_dirs::TestScratchDir::new("flush-candidates");
    let opened = crate::save::open_at(dir.to_path_buf()).expect("open save");
    let mut world = ServerWorld::new(0, 0);
    world.attach_save(opened.save, opened.saved);
    let lit = |sp: SectionPos, dirty_light: bool| {
        let mut s = Section::new(sp.cx, sp.cy, sp.cz);
        s.set_block(3, 0, 3, Block::Stone);
        s.set_skylight(vec![0u8; SECTION_VOLUME].into());
        s.set_blocklight(vec![petramond_world::light::LightRgb::ZERO; SECTION_VOLUME].into());
        s.mark_light_clean();
        s.light_dirty = dirty_light;
        s
    };
    let sections: Vec<SectionPos> = (0..6).map(|i| SectionPos::new(i, 4, 0)).collect();
    for (i, &sp) in sections.iter().enumerate() {
        world.insert_section_for_test(sp, lit(sp, i == 5));
    }
    let scan = |world: &ServerWorld| -> Vec<SectionPos> {
        let mut out: Vec<SectionPos> = world
            .data
            .sections
            .keys()
            .copied()
            .filter(|&sp| {
                world
                    .snapshot_section_for_save(sp, Vec::new(), Vec::new(), false)
                    .is_some()
            })
            .collect();
        out.sort_by_key(|p| (p.cx, p.cy, p.cz));
        out
    };

    let expected = scan(&world);
    assert_eq!(
        expected.len(),
        5,
        "fixture: five light-final first persists"
    );
    world.flush_modified_chunks();
    assert!(expected.iter().all(|&sp| persisted(&world, sp)));
    assert!(!persisted(&world, sections[5]), "unsettled light waits");
    assert!(scan(&world).is_empty(), "nothing left for a full scan");

    world.set_block_world(2 * 16 + 5, 65, 5, Block::Dirt);
    world
        .data
        .section_mut(sections[5])
        .expect("loaded")
        .mark_light_clean();
    let expected = scan(&world);
    assert!(expected.contains(&sections[2]) && expected.contains(&sections[5]));
    world.flush_modified_chunks();
    assert!(scan(&world).is_empty(), "the flush caught every change");
    assert!(persisted(&world, sections[5]));
    assert!(!world.data.sections[&sections[2]].modified);
    drop(world);
}
