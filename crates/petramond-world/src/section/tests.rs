use super::*;

#[test]
fn biome_tint_hint_tracks_incremental_and_bulk_blocks() {
    let mut section = Section::new(0, 0, 0);
    assert!(!section.has_biome_tint_blocks());

    section.set_block(1, 1, 1, Block::Stone);
    assert!(!section.has_biome_tint_blocks());

    section.set_block(1, 1, 1, Block::Grass);
    assert!(section.has_biome_tint_blocks());

    section.set_block(1, 1, 1, Block::Dirt);
    assert!(!section.has_biome_tint_blocks());

    section.set_fluid(2, 2, 2, Block::Water, 0);
    assert!(section.has_biome_tint_blocks());

    section.set_block(2, 2, 2, Block::Air);
    assert!(!section.has_biome_tint_blocks());

    section.blocks_mut().set(0, Block::OakLeaves.id());
    section.recompute_opaque_count();
    assert!(section.has_biome_tint_blocks());
}

#[test]
fn particle_emitter_hint_tracks_incremental_and_bulk_blocks() {
    fn check(section: &Section) {
        let scanned: Vec<u16> = section
            .blocks_iter()
            .collect::<Vec<_>>()
            .iter()
            .enumerate()
            .filter(|(_, &id)| Block::from_id(id).particle_emitter().is_some())
            .map(|(i, _)| i as u16)
            .collect();
        assert_eq!(section.particle_emitter_cells(), scanned.as_slice());
        assert_eq!(section.has_particle_emitters(), !scanned.is_empty());
        assert_eq!(
            section.stream_metrics().particle_emitter_count as usize,
            scanned.len()
        );
    }

    let mut section = Section::new(0, 0, 0);
    check(&section);

    section.set_block(1, 1, 1, Block::Stone);
    check(&section);

    section.set_block(1, 1, 1, Block::Torch);
    assert!(section.has_particle_emitters());
    check(&section);

    section.set_block(0, 0, 3, Block::Torch);
    check(&section);

    section.set_block(1, 1, 1, Block::Air);
    check(&section);

    section.set_block(0, 0, 3, Block::Air);
    assert!(!section.has_particle_emitters());
    check(&section);

    section.blocks_mut().set(0, Block::Torch.id());
    section.recompute_opaque_count();
    assert!(section.has_particle_emitters());
    check(&section);

    section.blocks_mut().set(0, Block::Stone.id());
    section.recompute_opaque_count();
    check(&section);
}

#[test]
fn sparse_maps_walk_in_cell_order_whatever_the_insertion_order() {
    const CELLS: usize = 24;
    fn build(order: impl Iterator<Item = usize>) -> Section {
        let mut section = Section::new(0, 0, 0);
        for i in order {
            let (x, y, z) = (i % 16, i / 16, (i * 5) % 16);
            section.set_block(x, y, z, Block::FurnaceLit);
            section.insert_furnace(x, y, z, Furnace::default());
            section.insert_container(x, y, z, Container::with_len(3));
        }
        section
    }
    let mut forward = build(0..CELLS);
    let mut reverse = build((0..CELLS).rev());
    assert!(forward.furnaces().keys().is_sorted());
    assert!(reverse.containers().keys().is_sorted());
    let reskin = forward.tick_furnaces(|_| None);
    assert_eq!(reskin.len(), CELLS);
    assert!(reskin
        .iter()
        .map(|&(x, y, z, _)| section_idx(x, y, z))
        .is_sorted());
    assert_eq!(reskin, reverse.tick_furnaces(|_| None));
}
