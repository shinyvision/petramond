use super::*;
use crate::world::mesh_pool::{build_inline, nbhd_idx27};
use petramond_world::{
    chunk::{SECTION_MAX_CY, SECTION_MIN_CY, SECTION_SIZE},
    section::SectionSummary,
};

/// Some pair the shipped catalog declares — the plumbing under test is the
/// snapshot path, not which blocks are tuned to transition.
fn shipped_pair() -> (Block, Block) {
    let rules = petramond_world::texture_transition::rules();
    rules
        .sets
        .iter()
        .find_map(|set| {
            let m = &set.materials;
            (0..m.len())
                .flat_map(|a| (0..m.len()).map(move |b| (a, b)))
                .find_map(|(a, b)| {
                    set.allows(a as u8 + 1, b as u8 + 1)
                        .then_some((m[a].block, m[b].block))
                })
        })
        .expect("the shipped catalog declares at least one pair")
}

#[test]
fn transition_donors_use_snapshot_tints_across_sections() {
    let mut world = World::new(0, 0);
    let center = SectionPos::new(0, 0, 0);
    let east = SectionPos::new(1, 0, 0);
    let (source, donor) = shipped_pair();
    for (pos, x, b) in [(center, 15, source), (east, 0, donor)] {
        let mut s = Section::new(pos.cx, pos.cy, pos.cz);
        s.set_block(x, 8, 8, b);
        if pos == east {
            s.set_block(x, 9, 8, Block::ShortGrass);
        }
        world.insert_section_for_test(pos, s);
    }
    let has_transition = |world: &World| {
        let mesh = build_inline(world.build_mesh_job(center).unwrap()).unwrap();
        mesh.opaque
            .iter()
            .any(petramond_mesh::vertex::transition::Transition::carried_by)
    };
    assert!(has_transition(&world));
    world.section_mut(east).unwrap().cell_kv_set(
        0,
        8,
        8,
        petramond_world::block::TINT_KV_KEY.into(),
        vec![64, 128, 192],
    );
    assert!(!has_transition(&world));
    world
        .section_mut(east)
        .unwrap()
        .cell_kv_remove(0, 8, 8, petramond_world::block::TINT_KV_KEY);
    assert!(has_transition(&world));
    world
        .section_mut(east)
        .unwrap()
        .set_block(1, 9, 8, Block::SnowLayer);
    assert!(
        !has_transition(&world),
        "actual snow bedding excludes the donor"
    );
    world
        .section_mut(east)
        .unwrap()
        .set_block(1, 9, 8, Block::Air);
    assert!(
        has_transition(&world),
        "removing snow restores the decorated donor"
    );
}

#[test]
fn empty_summary_is_known_but_pending_stream_data_is_not() {
    let mut world = World::new(0, 0);
    let center = SectionPos::new(0, 0, 0);
    insert_solid_section(&mut world, center);
    let above = SectionPos::new(0, 1, 0);
    let idx = nbhd_idx27(0, 1, 0);
    assert!(world.build_mesh_job(center).unwrap().nbhd[idx].is_none());
    world.column_summaries.insert(
        center.chunk_pos(),
        vec![SectionSummary::Empty; (SECTION_MAX_CY - SECTION_MIN_CY + 1) as usize]
            .into_boxed_slice(),
    );
    assert!(world.build_mesh_job(center).unwrap().nbhd[idx].is_some());
    world.stream_nonfinal.insert(above);
    assert!(world.build_mesh_job(center).unwrap().nbhd[idx].is_none());
}

#[test]
fn edits_inside_the_sampling_halo_of_a_neighbour_remesh_it() {
    let halo = &petramond_mesh::SAMPLING_HALO;
    let mut world = World::new(0, 0);
    let center = SectionPos::new(0, 0, 0);
    insert_solid_section(&mut world, center);
    let n = SECTION_SIZE as i32;
    let (h, up, down) = (halo.horizontal as i32, halo.up as i32, halo.down as i32);
    // The farthest cell each neighbour's halo reaches, then one past it.
    for (cell, reaches) in [
        ((8, n + up - 1, 8), true),
        ((8, n + up, 8), false),
        ((8, -down, 8), true),
        ((8, -down - 1, 8), false),
        ((n + h - 1, 9, 8), true),
        ((n + h, 9, 8), false),
        ((-h, 9, 8), true),
        ((-h - 1, 9, 8), false),
        ((8, 9, n + h - 1), true),
        ((8, 9, -h), true),
    ] {
        let before = world.sections[&center].mesh_revision;
        world.queue_dirty_meshes_sampling_cell(cell.0, cell.1, cell.2);
        assert_eq!(
            world.sections[&center].mesh_revision > before,
            reaches,
            "edit at {cell:?}"
        );
    }
}
