use std::sync::Arc;

use crate::block::Block;
use crate::chunk::{ChunkPos, SectionPos, SECTION_MIN_CY, SECTION_SIZE, SECTION_VOLUME};
use crate::mathh::IVec3;
use crate::mesh::ChunkMesh;
use crate::section::Section;
use crate::worldgen::driver::ChunkGenerator;

use super::World;

fn install_column_summary(world: &mut World, generator: &ChunkGenerator, pos: ChunkPos) {
    world.ensure_column(pos);
    world
        .column_gen
        .insert(pos, Arc::new(generator.generate_column_gen(pos.cx, pos.cz)));
}

#[test]
fn same_height_surface_swap_bumps_the_column_revision() {
    // Replacing the visible surface block in place (tilling, grass
    // spread, …) keeps the heightmap, but revision-gated surface
    // sampling must still see the column move or the swapped color is
    // never resampled.
    let mut world = World::new(0, 0);
    let sp = SectionPos::new(0, 4, 0);
    let mut s = Section::new(0, 4, 0);
    s.set_block(8, 0, 8, Block::Stone);
    world.insert_section_for_test(sp, s);
    let column = world.ensure_column(sp.chunk_pos());
    column.set_surface_y(8, 8, 64);
    column.set_sky_cover_y(8, 8, 64);

    let before = world.column_payload_revision(sp.chunk_pos());
    world.set_block_world(8, 64, 8, Block::Dirt);
    assert_eq!(
        world.columns[&sp.chunk_pos()].surface_y(8, 8),
        64,
        "fixture: the swap must not move the heightmap"
    );
    assert_ne!(
        before,
        world.column_payload_revision(sp.chunk_pos()),
        "a same-height surface swap must move the column revision"
    );
}

#[test]
fn edits_in_total_darkness_skip_light_invalidation_entirely() {
    // The adaptive relight radius: light values bound how far a plain
    // solid⇄air edit can matter, so mining inside unlit solid rock (the
    // hot gameplay path) must trigger NO light invalidation or rebake.
    let mut world = World::new(0, 4);
    let pos = SectionPos::new(0, 0, 0);
    let mut section = Section::new(0, 0, 0);
    section.blocks_slice_mut().fill(Block::Stone.id());
    section.recompute_opaque_count();
    world.insert_section_for_test(pos, section);
    {
        let s = world.section_mut(pos).unwrap();
        s.set_skylight(vec![0u8; SECTION_VOLUME].into());
        s.set_blocklight(vec![0u8; SECTION_VOLUME].into());
    }
    // The fixture insert demands a bake; only the edits below are under test.
    world.relight_demand.clear();
    assert!(!world.sections[&pos].light_dirty, "fixture: settled dark");

    assert!(world.set_block_world(8, 8, 8, Block::Air));
    assert!(
        !world.sections[&pos].light_dirty,
        "no light can reach the opened cell, so nothing may invalidate"
    );
    assert!(world.relight_demand.is_empty());

    // Control: the same break beside cached light must invalidate.
    world
        .section_mut(pos)
        .unwrap()
        .set_skylight(vec![crate::chunk::SKY_FULL; SECTION_VOLUME].into());
    assert!(world.set_block_world(8, 4, 8, Block::Air));
    assert!(
        world.sections[&pos].light_dirty,
        "a break beside lit cells must invalidate light"
    );
    assert!(world.relight_demand.contains(&pos));
}

#[test]
fn glass_raises_the_visible_surface_without_raising_sky_cover() {
    let mut world = World::new(0, 0);
    let cp = ChunkPos::new(0, 0);

    assert!(world.set_block_world(8, 0, 8, Block::Stone));
    assert!(world.set_block_world(8, 64, 8, Block::Glass));

    let column = &world.columns[&cp];
    assert_eq!(column.surface_y(8, 8), 64);
    assert_eq!(
        column.sky_cover_y(8, 8),
        0,
        "clear glass must not hide the open shaft from the skylight planner"
    );
}

#[test]
fn eviction_racing_an_edit_relight_rewrites_the_record_lightless() {
    // Two adjacent sections persist with clean baked light. An edit in A
    // then dirties B's light (content change → B's on-disk cubes are
    // pre-edit stale). If eviction/quit wins the race against B's rebake,
    // the persist gate must rewrite B's record WITHOUT light so reload
    // rebakes — the pre-fix gate skipped unmodified light-dirty sections
    // entirely, stranding the stale cubes as a permanent dark seam.
    let dir = std::env::temp_dir().join(format!("petramond-stale-light-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let opened = crate::save::open_at(dir.clone()).expect("open save");
    let mut world = World::new(0, 0);
    world.attach_save(opened.save);

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
        s.set_blocklight(vec![0u8; SECTION_VOLUME].into());
        s.mark_light_clean();
        world.insert_section_for_test(sp, s);
        world.section_mut(sp).expect("loaded").modified = true;
    }
    world.flush_modified_chunks();
    assert!(
        world.save().expect("save").manifest_contains(b),
        "fixture: B's record is on disk"
    );
    assert!(
        !world.sections[&b].light_dirty,
        "fixture: B persisted with clean light"
    );

    // The edit in A, one cell from the seam: B's cached AND persisted
    // light are now stale.
    world.set_block_world(15, 65, 8, Block::Stone);
    assert!(world.sections[&b].light_dirty);

    let snap = world
        .snapshot_section_for_save(b, Vec::new(), Vec::new(), false)
        .expect("an unmodified on-disk section with edit-dirtied light must rewrite");
    assert!(
        snap.skylight.is_none() && snap.blocklight.is_none(),
        "the rewrite must omit the stale cubes so reload rebakes"
    );

    drop(world);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn mesh_column_index_tracks_multiple_vertical_meshes() {
    let mut world = World::new(0, 0);
    let lower = SectionPos::new(4, 0, -2);
    let upper = SectionPos::new(4, 1, -2);
    let column = lower.chunk_pos();

    assert!(!world.column_has_mesh(column));
    world.install_mesh(lower, ChunkMesh::empty());
    world.install_mesh(upper, ChunkMesh::empty());
    assert!(world.column_has_mesh(column));
    let bits = world.mesh_column_cys[&column];
    assert_eq!(bits.count_ones(), 2);

    assert!(world.remove_mesh(lower));
    assert!(world.column_has_mesh(column));
    assert_eq!(world.mesh_column_cys[&column].count_ones(), 1);

    assert!(world.remove_mesh(upper));
    assert!(!world.column_has_mesh(column));
    assert!(!world.mesh_column_cys.contains_key(&column));
}

#[test]
fn virtual_full_opaque_summary_blocks_collision_without_raw_voxels() {
    let seed = 0x51EED;
    let generator = ChunkGenerator::new(seed);
    let mut world = World::new(seed, 0);
    install_column_summary(&mut world, &generator, ChunkPos::new(0, 0));

    let y = SECTION_MIN_CY * SECTION_SIZE as i32;
    assert_eq!(
        Block::from_id(world.chunk_block(0, y, 0)),
        Block::Air,
        "raw reads stay exact: absent voxel buffers still read as air"
    );
    assert_eq!(
        world.physics_block(0, y, 0),
        Block::Stone,
        "physics reads may use the generated full-opaque summary"
    );
    assert!(
        !world.collision_boxes_at(0, y, 0).is_empty(),
        "virtual full-opaque summary should collide as a full block"
    );
    assert!(
        !world.placement_cell_open(IVec3::new(0, y, 0)),
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

    let mut world = World::new(seed, 0);
    world.ensure_column(cp);
    world.column_gen.insert(cp, Arc::clone(&col));

    let cy = cave_top.div_euclid(SECTION_SIZE as i32);
    let sp = SectionPos::new(cp.cx, cy, cp.cz);
    let section = generator.generate_section(sp, &col);
    world.sections.insert(sp, Arc::new(section));
    world.note_section_loaded(sp);

    world.recompute_column_heightmaps(cp);

    assert_eq!(
        world.columns.get(&cp).unwrap().surface_y(x, z),
        cave_top,
        "heightmap refresh must not restore original pre-cave surface {original}"
    );
}

#[test]
fn heightmap_recompute_keeps_glass_out_of_direct_sky_cover() {
    let mut world = World::new(0, 0);
    let cp = ChunkPos::new(0, 0);
    let ground = SectionPos::new(0, 0, 0);
    let roof = SectionPos::new(0, 4, 0);

    let mut ground_section = Section::new(0, 0, 0);
    ground_section.set_block(8, 0, 8, Block::Stone);
    world.sections.insert(ground, Arc::new(ground_section));
    world.note_section_loaded(ground);
    let mut roof_section = Section::new(0, 4, 0);
    roof_section.set_block(8, 0, 8, Block::Glass);
    world.sections.insert(roof, Arc::new(roof_section));
    world.note_section_loaded(roof);

    let column = world.ensure_column(cp);
    column.set_surface_y(8, 8, 64);
    column.set_sky_cover_y(8, 8, 64);

    assert!(world.recompute_column_heightmaps(cp).is_some());
    let column = &world.columns[&cp];
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

    let mut world = World::new(seed, 0);
    let column = world.ensure_column(cp);
    column.set_surface_y(x, z, ground);
    column.set_sky_cover_y(x, z, ground);
    world.column_gen.insert(cp, col);

    let ground_sp = SectionPos::from_world(
        cp.cx * SECTION_SIZE as i32 + x as i32,
        ground,
        cp.cz * SECTION_SIZE as i32 + z as i32,
    )
    .unwrap();
    world.sections.insert(
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
    world.sections.insert(lower_sp, Arc::new(lower_section));
    world.note_section_loaded(lower_sp);

    world.recompute_column_heightmaps(cp);

    assert_eq!(
        world.columns.get(&cp).unwrap().surface_y(x, z),
        lower,
        "a loaded dug shaft must not be covered again by the generated fallback"
    );
}

#[test]
fn removing_surface_cover_relights_loaded_sections_below_the_changed_section() {
    let dir = std::env::temp_dir().join(format!(
        "petramond-sky-cover-relight-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    let opened = crate::save::open_at(dir.clone()).expect("open save");
    let mut world = World::new(0, 0);
    world.attach_save(opened.save);
    let cp = ChunkPos::new(0, 0);
    let shaft_x = 8;
    let shaft_z = 8;
    let cover_y = 64;
    let top = SectionPos::new(0, 4, 0);
    let lower = SectionPos::new(0, 2, 0);

    let column = world.ensure_column(cp);
    column.set_surface_y(shaft_x, shaft_z, cover_y);
    column.set_sky_cover_y(shaft_x, shaft_z, cover_y);

    let mut top_section = Section::new(top.cx, top.cy, top.cz);
    top_section.set_block(shaft_x, 0, shaft_z, Block::Dirt);
    top_section.set_skylight(vec![0u8; SECTION_VOLUME].into());
    top_section.set_blocklight(vec![0u8; SECTION_VOLUME].into());
    top_section.dirty = false;

    let mut lower_section = Section::new(lower.cx, lower.cy, lower.cz);
    lower_section.set_skylight(vec![0u8; SECTION_VOLUME].into());
    lower_section.set_blocklight(vec![0u8; SECTION_VOLUME].into());
    lower_section.dirty = false;

    world.sections.insert(top, Arc::new(top_section));
    world.note_section_loaded(top);
    world.sections.insert(lower, Arc::new(lower_section));
    world.note_section_loaded(lower);

    assert!(
        !world.sections.get(&lower).unwrap().light_dirty,
        "fixture lower section starts with settled dark skylight"
    );
    assert!(
        !world.sections.get(&lower).unwrap().dirty,
        "fixture lower section starts with no pending mesh work"
    );

    assert!(world.set_block_world(shaft_x as i32, cover_y, shaft_z as i32, Block::Air));

    assert!(
        world.sections.get(&lower).unwrap().light_dirty,
        "removing sky cover must invalidate skylight below the edited section"
    );
    assert!(
        world.light_edited_since_persist.contains(&lower),
        "distant light invalidation must be tracked in case eviction beats the rebake"
    );

    // The mark itself demands the rebake (`relight_demand`) — no mesh is
    // pre-queued for the distant section — and the landed bake's changed
    // cubes requeue its mesh.
    let mut landed = false;
    for _ in 0..2500 {
        world.pump_light_bakes();
        if !world.sections.get(&lower).unwrap().light_dirty {
            landed = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    assert!(landed, "the marked distant section must rebake unprompted");
    let lower_section = world.sections.get(&lower).unwrap();
    assert_eq!(
        lower_section.skylight_at(shaft_x, 8, shaft_z),
        crate::chunk::SKY_FULL,
        "the opened shaft must reach full skylight below"
    );
    assert!(
        lower_section.dirty,
        "changed cubes must requeue the section's mesh"
    );

    drop(world);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn air_edit_into_absent_full_opaque_section_materializes_generated_base() {
    let seed = 0x51EED;
    let generator = ChunkGenerator::new(seed);
    let mut world = World::new(seed, 0);
    install_column_summary(&mut world, &generator, ChunkPos::new(0, 0));

    let y = SECTION_MIN_CY * SECTION_SIZE as i32;
    let sp = SectionPos::from_world(0, y, 0).unwrap();
    assert!(
        !world.sections.contains_key(&sp),
        "the deep generated-solid section starts summary-only"
    );

    assert!(world.set_block_world(0, y, 0, Block::Air));
    assert!(
        world.sections.contains_key(&sp),
        "editing virtual solid materializes the generated section"
    );
    assert_eq!(Block::from_id(world.chunk_block(0, y, 0)), Block::Air);
    assert_ne!(
        Block::from_id(world.chunk_block(1, y, 0)),
        Block::Air,
        "materialization preserves the generated solid neighbours instead of creating an empty section"
    );
}
