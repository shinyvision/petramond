use super::*;

#[test]
fn visible_ground_keeps_its_transition_under_partial_shapes() {
    for cover in [
        Block::Air,
        Block::ShortGrass,
        Block::PebblesSmall,
        Block::PebblesLarge,
        Block::OakFence,
        Block::Glass,
    ] {
        let mut section = Section::new(0, 0, 0);
        section.set_block(8, 8, 8, Block::Sand);
        section.set_block(9, 8, 8, Block::Grass);
        section.set_block(9, 9, 8, cover);
        let m = mesh(&section);
        for x in [8.0, 9.0] {
            let top = m
                .opaque
                .chunks_exact(4)
                .find(|q| {
                    q.iter().all(|v| {
                        v.pos[1] == 9.0
                            && v.pos[0] >= x
                            && v.pos[0] <= x + 1.0
                            && v.pos[2] >= 8.0
                            && v.pos[2] <= 9.0
                    }) && shade_idx(&q[0]) == 0
                })
                .unwrap_or_else(|| panic!("missing ground face below {cover:?}"));
            assert!(
                top.iter()
                    .all(crate::vertex::transition::Transition::carried_by),
                "{cover:?} interrupted the transition at {x}"
            );
        }
    }
}
