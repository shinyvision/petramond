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

    section.set_water(2, 2, 2, Block::Water, 0);
    assert!(section.has_biome_tint_blocks());

    section.set_block(2, 2, 2, Block::Air);
    assert!(!section.has_biome_tint_blocks());

    section.blocks_slice_mut()[0] = Block::OakLeaves.id();
    section.recompute_opaque_count();
    assert!(section.has_biome_tint_blocks());
}

/// The sparse emitter-cell index is what presentation walks each frame, and it
/// is maintained on two independent paths (the per-cell setters and the bulk
/// metrics install). A path that updates one and not the other renders ghost or
/// missing flames, so every step is checked against a from-scratch scan.
#[test]
fn particle_emitter_hint_tracks_incremental_and_bulk_blocks() {
    fn check(section: &Section) {
        let scanned: Vec<u16> = section
            .blocks_slice()
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

    // A second, LOWER-indexed emitter: the index must stay ascending, since the
    // gather's output order (and with it the render's depth-sort tie-break)
    // follows it.
    section.set_block(0, 0, 3, Block::Torch);
    check(&section);

    section.set_block(1, 1, 1, Block::Air);
    check(&section);

    section.set_block(0, 0, 3, Block::Air);
    assert!(!section.has_particle_emitters());
    check(&section);

    section.blocks_slice_mut()[0] = Block::Torch.id();
    section.recompute_opaque_count();
    assert!(section.has_particle_emitters());
    check(&section);

    // Bulk install over a section that already carried emitters must not leave
    // the old cells behind.
    section.blocks_slice_mut()[0] = Block::Stone.id();
    section.recompute_opaque_count();
    check(&section);
}
