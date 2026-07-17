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

#[test]
fn particle_emitter_hint_tracks_incremental_and_bulk_blocks() {
    let mut section = Section::new(0, 0, 0);
    assert!(!section.has_particle_emitters());

    section.set_block(1, 1, 1, Block::Stone);
    assert!(!section.has_particle_emitters());

    section.set_block(1, 1, 1, Block::Torch);
    assert!(section.has_particle_emitters());

    section.set_block(1, 1, 1, Block::Air);
    assert!(!section.has_particle_emitters());

    section.blocks_slice_mut()[0] = Block::Torch.id();
    section.recompute_opaque_count();
    assert!(section.has_particle_emitters());
}
