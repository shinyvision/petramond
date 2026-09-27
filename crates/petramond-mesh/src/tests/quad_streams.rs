use super::*;

#[test]
fn every_terrain_stream_is_a_whole_number_of_quads() {
    let mut section = floor_section(Block::Stone);
    section.set_block(1, 1, 1, Block::OakLeaves);
    section.set_block(3, 1, 3, Block::ShortGrass);
    section.set_block(5, 1, 5, Block::Torch);
    section.set_block(7, 1, 7, Block::Cactus);
    section.set_block(9, 1, 9, Block::OakStairs);
    section.set_block(11, 1, 11, Block::Water);
    section.set_block(12, 1, 11, Block::Water);
    let m = mesh(&section);

    for (name, len) in [
        ("opaque", m.opaque.len()),
        ("far_opaque", m.far_opaque_len as usize),
        ("transparent", m.transparent.len()),
        ("transparent_two_sided", m.transparent_two_sided.len()),
        ("translucent", m.translucent.len()),
    ] {
        assert_eq!(len % 4, 0, "{name} stream is not a whole number of quads");
    }
    assert!(!m.opaque.is_empty());
    assert!(!m.transparent_two_sided.is_empty(), "water top expected");
}
