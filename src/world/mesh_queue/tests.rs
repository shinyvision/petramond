use std::sync::Arc;

use crate::block::Block;
use crate::chunk::{SectionPos, SECTION_VOLUME};
use crate::section::Section;
use crate::world::store::{LoadTarget, World, WorldRole};

use super::DirtyMeshQueue;

fn solid_section(pos: SectionPos) -> Section {
    let mut section = Section::new(pos.cx, pos.cy, pos.cz);
    section.blocks_slice_mut().fill(Block::Stone.id());
    section.recompute_opaque_count();
    section
}

fn insert_solid_section(world: &mut World, pos: SectionPos) {
    world.ensure_column(pos.chunk_pos());
    world.sections.insert(pos, Arc::new(solid_section(pos)));
}

fn insert_sealed_cavity(world: &mut World, center: SectionPos) {
    let mut cavity = solid_section(center);
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
        insert_solid_section(
            world,
            SectionPos::new(center.cx + dx, center.cy + dy, center.cz + dz),
        );
    }
}

#[test]
fn mesh_job_uses_column_generated_biome_tint_halo() {
    let mut world = World::new(0, 0);
    let pos = SectionPos::new(0, 0, 0);
    insert_solid_section(&mut world, pos);
    let gen = crate::worldgen::driver::ChunkGenerator::new(0).generate_column_gen(pos.cx, pos.cz);
    world.column_gen.insert(pos.chunk_pos(), Arc::new(gen));

    let job = world
        .build_mesh_job(pos)
        .expect("the center column carries its complete tint halo");
    assert!(
        job.biome.iter().all(|&id| id != 0),
        "mesh jobs must not bake chunk-edge tint from missing-biome id 0"
    );
}

#[test]
fn stale_rejected_light_bake_requests_a_rebake() {
    // A revision bump while a bake is in flight (an edit, a neighbour
    // landing) makes the result stale. Its rejection is the only moment
    // the pending slot is clear again — every request during the flight
    // was dedup-dropped — so without an immediate re-request the section
    // wedges light-dirty and every mesh sampling it parks forever.
    let mut world = World::new(0, 1);
    let pos = SectionPos::new(0, 0, 0);
    let mut section = solid_section(pos);
    section.set_block(8, 8, 8, Block::Air);
    world.insert_section_for_test(pos, section);
    assert!(world.sections[&pos].light_dirty, "fixture: bake wanted");

    world
        .light_bakes
        .request(0, pos, &world.sections, &world.columns);
    // Invalidate the in-flight bake exactly as an edit / landing does. The
    // result is drained on this thread, so the bump always beats it.
    world.mark_light_dirty_pos(pos);

    for _ in 0..2500 {
        world.pump_light_bakes();
        if !world.sections[&pos].light_dirty {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    panic!("stale-rejected bake was never re-requested: section wedged light-dirty");
}

#[test]
fn light_blocked_mesh_leaves_hot_dirty_queue() {
    let mut world = World::new(0, 0);
    let pos = SectionPos::new(0, 0, 0);
    let mut section = Section::new(pos.cx, pos.cy, pos.cz);
    section.set_block(0, 0, 0, Block::Dirt);
    world.insert_section_for_test(pos, section);

    world.tick_mesh_budget(1);

    assert!(
        world.dirty_meshes.is_empty(),
        "light-blocked meshes should not churn in the hot dirty queue"
    );
    assert!(
        world.light_blocked_meshes.contains(&pos),
        "the mesh should be parked until its light dependency finishes"
    );
}

#[test]
fn dirty_mesh_priority_is_near_first() {
    let target = LoadTarget::new(0, 0, 0, 16);
    let near = SectionPos::new(0, 0, 2);
    let far = SectionPos::new(16, 0, 0);

    let mut queue = DirtyMeshQueue::default();
    queue.push(far);
    queue.push(near);
    assert_eq!(
        queue.pop_nearest_batch(1, Some(target)),
        vec![near],
        "near dirty meshes must beat far dirty meshes"
    );
}

#[test]
fn all_air_transition_removes_stale_ghost_mesh() {
    let mut world = World::new(0, 0);
    let center = SectionPos::new(0, 0, 0);
    insert_solid_section(&mut world, center);
    world.queue_dirty_mesh(center);

    world.mesh_section_blocking_for_test(center);
    assert!(
        world.meshes.get(&center).is_some_and(|m| !m.is_empty()),
        "a solid section with missing neighbours meshes its exposed border"
    );

    // Mine the section out entirely: all-air emits nothing.
    let before_revision = {
        let s = world.section_mut(center).unwrap();
        s.blocks_slice_mut().fill(Block::Air.id());
        s.recompute_opaque_count();
        s.mesh_revision
    };
    assert!(
        world.clear_mesh_if_section_produces_no_mesh(center),
        "the all-air section should settle to no render output"
    );
    assert!(
        world
            .sections
            .get(&center)
            .is_some_and(|s| s.mesh_revision > before_revision),
        "settling to no-mesh must invalidate in-flight jobs built from the old blocks"
    );
    assert!(
        !world.meshes.contains_key(&center),
        "stale ghost mesh must be removed"
    );
    assert!(
        world
            .mesh_upload_dirty_columns
            .contains(&center.chunk_pos()),
        "the render column must be marked for GPU repack"
    );
}

#[test]
fn loaded_opaque_neighbour_planes_seal_future_mesh_work() {
    // Only exact loaded planes may seal; generated summaries can disagree
    // with saved/player-carved terrain.
    let mut world = World::new(0, 0);
    let center = SectionPos::new(0, 0, 0);
    insert_solid_section(&mut world, center);
    for (dx, dy, dz) in [
        (1, 0, 0),
        (-1, 0, 0),
        (0, 1, 0),
        (0, -1, 0),
        (0, 0, 1),
        (0, 0, -1),
    ] {
        insert_solid_section(
            &mut world,
            SectionPos::new(center.cx + dx, center.cy + dy, center.cz + dz),
        );
    }
    assert!(
        world.section_sealed_by_loaded_neighbors(center),
        "six exact opaque neighbour planes make future mesh work invisible"
    );
}

#[test]
fn sealed_section_around_player_still_meshes_and_remeshes() {
    let mut world = World::new(0, 4);
    let center = SectionPos::new(0, 0, 0);
    insert_sealed_cavity(&mut world, center);
    world.last_load_target = Some(LoadTarget::new(0, 0, 0, 4));

    assert!(
        !world.section_sealed_by_loaded_neighbors(center),
        "a player can already be inside an otherwise sealed underground section"
    );
    world.mesh_section_blocking_for_test(center);
    assert!(
        world
            .meshes
            .get(&center)
            .is_some_and(|mesh| !mesh.is_empty()),
        "the internal cavity walls must mesh around the player"
    );

    let before = world.mesh_upload_revisions[&center.chunk_pos()];
    world
        .section_mut(center)
        .unwrap()
        .set_block(9, 8, 8, Block::Air);
    world.queue_dirty_mesh(center);
    world.mesh_section_blocking_for_test(center);
    assert!(
        world.mesh_upload_revisions[&center.chunk_pos()] > before,
        "an edit inside the sealed section must install a fresh mesh"
    );
}

#[test]
fn far_sealed_section_requeues_when_player_approaches() {
    let mut world = World::new(0, 16);
    let center = SectionPos::new(0, 0, 0);
    insert_sealed_cavity(&mut world, center);
    world
        .section_mut(center)
        .unwrap()
        .set_skylight(Arc::from(vec![0u8; SECTION_VOLUME].into_boxed_slice()));
    world.last_load_target = Some(LoadTarget::new(8, 0, 0, 16));

    world.tick_mesh_budget(1);
    assert!(world.sealed_parked.contains(&center));
    assert!(!world.meshes.contains_key(&center));

    world.last_load_target = Some(LoadTarget::new(0, 0, 0, 16));
    world.vis_dirty = true;
    world.refresh_deep_visibility();
    assert!(!world.sealed_parked.contains(&center));
    assert!(world.dirty_meshes.contains(center));
    world.mesh_section_blocking_for_test(center);
    assert!(world.meshes.contains_key(&center));
}

#[test]
fn predicted_mine_relights_and_remeshes_the_opened_shaft_synchronously() {
    let mut world = World::new_with_role(0, 4, WorldRole::ClientReplica);
    let ground = SectionPos::new(0, 0, 0);
    let shaft = SectionPos::new(0, 1, 0);
    let roof = SectionPos::new(0, 2, 0);

    let mut ground_section = solid_section(ground);
    ground_section.set_skylight(vec![0u8; SECTION_VOLUME].into());

    let mut shaft_section = solid_section(shaft);
    for y in 0..crate::chunk::SECTION_SIZE {
        shaft_section.set_block(8, y, 8, Block::Air);
    }
    shaft_section.set_skylight(vec![0u8; SECTION_VOLUME].into());

    let mut roof_section = Section::new(roof.cx, roof.cy, roof.cz);
    for z in 0..crate::chunk::SECTION_SIZE {
        for x in 0..crate::chunk::SECTION_SIZE {
            roof_section.set_block(x, 0, z, Block::Dirt);
        }
    }
    roof_section.set_skylight(vec![0u8; SECTION_VOLUME].into());

    world.ensure_column(ground.chunk_pos());
    world.sections.insert(ground, Arc::new(ground_section));
    world.sections.insert(shaft, Arc::new(shaft_section));
    world.sections.insert(roof, Arc::new(roof_section));
    let column = world.columns.get_mut(&ground.chunk_pos()).unwrap();
    for z in 0..crate::chunk::SECTION_SIZE {
        for x in 0..crate::chunk::SECTION_SIZE {
            column.set_surface_y(x, z, 32);
            column.set_sky_cover_y(x, z, 32);
        }
    }
    world.last_load_target = Some(LoadTarget::new(0, 2, 0, 4));
    world.light_deferred.insert(shaft);

    let cell = crate::mathh::IVec3::new(8, 32, 8);
    assert!(world.set_block_world(cell.x, cell.y, cell.z, Block::Air));
    world.present_predicted_edit(&[(cell, Block::Dirt.id())]);

    assert!(!world.sections[&shaft].light_dirty);
    assert_eq!(
        world.sections[&shaft].skylight_at(8, 15, 8),
        crate::chunk::SKY_FULL
    );
    assert!(world.meshes.contains_key(&shaft));
    assert!(!world.light_deferred.contains(&shaft));
    assert!(!world.prediction_terrain.has_pending());
}

#[test]
fn reconciliation_is_async_and_never_overrides_authoritative_light() {
    use crate::net::protocol::{LightPayload, SectionBytes};

    // Reconciliation keeps the non-blocking path: retain the installed
    // prediction mesh until the corrective bundle has exact light.
    let mut world = World::new_with_role(0, 4, WorldRole::ClientReplica);
    let pos = SectionPos::new(0, 0, 0);
    insert_solid_section(&mut world, pos);
    world.last_load_target = Some(LoadTarget::new(0, 0, 0, 4));
    world.queue_dirty_mesh(pos);
    world.mesh_section_blocking_for_test(pos);
    let before = world.mesh_upload_revisions[&pos.chunk_pos()];

    let cell = crate::mathh::IVec3::new(8, 8, 8);
    assert!(world.set_block_world(8, 8, 8, Block::Air));
    world.reconcile_predicted_edit(&[(cell, Block::Stone.id())]);

    assert_eq!(world.mesh_upload_revisions[&pos.chunk_pos()], before);
    assert!(
        world.sections[&pos].light_dirty,
        "the light-changing mesh must not publish before its bake"
    );
    assert!(world.prediction_terrain.has_pending());

    let mut landed = false;
    for _ in 0..2500 {
        world.tick_mesh_budget(1);
        if world.mesh_upload_revisions[&pos.chunk_pos()] > before {
            assert!(
                !world.sections[&pos].light_dirty,
                "the published prediction mesh must already carry final local light"
            );
            assert!(!world.prediction_terrain.has_pending());
            landed = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    assert!(landed, "reconciliation terrain bundle did not land");

    // If authoritative light lands while another correction is in flight,
    // the bundle's mesh-revision fence must reject the stale local result.
    assert!(world.set_block_world(cell.x, cell.y, cell.z, Block::Stone));
    world.reconcile_predicted_edit(&[(cell, Block::Air.id())]);
    assert!(world.prediction_terrain.has_pending());
    world.install_remote_light(LightPayload {
        pos,
        skylight: SectionBytes(Arc::from(vec![7u8; SECTION_VOLUME].into_boxed_slice())),
        blocklight: None,
    });
    for _ in 0..2500 {
        world.drain_prediction_terrain();
        if !world.prediction_terrain.has_pending() {
            assert_eq!(world.sections[&pos].skylight_at(8, 8, 8), 7);
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    panic!("stale reconciliation bundle did not retire");
}

#[test]
fn forced_repack_remesh_bypasses_sealed_parking() {
    let mut world = World::new(0, 16);
    let center = SectionPos::new(0, 0, 0);
    insert_sealed_cavity(&mut world, center);
    world
        .section_mut(center)
        .unwrap()
        .set_skylight(Arc::from(vec![0u8; SECTION_VOLUME].into_boxed_slice()));
    world.last_load_target = Some(LoadTarget::new(8, 0, 0, 16));
    world.repack_forced.insert(center);
    world.queue_dirty_mesh(center);

    world.mesh_section_blocking_for_test(center);
    assert!(world.meshes.contains_key(&center));
    assert!(!world.repack_forced.contains(&center));
    assert!(!world.sealed_parked.contains(&center));
}
