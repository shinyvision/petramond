use super::*;

#[test]
fn only_water_renews_sources_between_neighboring_sources() {
    for fluid in [Block::Water, Block::Lava] {
        for source_below in [false, true] {
            for initial_meta in [None, Some(flowing(3)), Some(FALLING)] {
                let mut w = flat_world();
                let y = if source_below { 66 } else { 65 };
                let gap = IVec3::new(8, y, 8);
                if source_below {
                    assert!(w.set_fluid_world(gap + DOWN, fluid, 0));
                    for x in [7, 9] {
                        assert!(w.set_block_world(x, y - 1, 8, Block::Stone));
                    }
                }
                assert!(w.set_fluid_world(gap - IVec3::X, fluid, 0));
                assert!(w.set_fluid_world(gap + IVec3::X, fluid, 0));
                if let Some(meta) = initial_meta {
                    assert!(w.set_fluid_world(gap, fluid, meta));
                }
                run_ticks(&mut w, (fluid_of(fluid).unwrap().delay as u32 + 2) * 2);

                assert_eq!(block(&w, gap.x, gap.y, gap.z), fluid);
                assert_eq!(
                    w.is_fluid_source_world(gap, fluid),
                    fluid == Block::Water,
                    "{fluid:?}, source below: {source_below}, initial meta: {initial_meta:?}"
                );
            }
        }
    }
}

#[test]
fn lava_fed_from_above_stays_falling_between_sources() {
    let mut w = flat_world();
    let gap = IVec3::new(8, 65, 8);
    for offset in [-IVec3::X, IVec3::X, UP] {
        assert!(w.set_fluid_world(gap + offset, Block::Lava, 0));
    }
    assert!(w.set_fluid_world(gap, Block::Lava, FALLING));
    run_ticks(&mut w, lava_ring() * 2);

    assert_eq!(block(&w, gap.x, gap.y, gap.z), Block::Lava);
    assert!(is_falling(w.fluid_meta_world(gap.x, gap.y, gap.z)));
}

#[test]
fn lava_between_removed_sources_fully_drains() {
    let mut w = flat_world();
    for x in [7, 9] {
        assert!(w.set_fluid_world(IVec3::new(x, 65, 8), Block::Lava, 0));
    }
    run_ticks(&mut w, lava_ring() * 4);
    assert_eq!(block(&w, 8, 65, 8), Block::Lava);

    for x in [7, 9] {
        carve(&mut w, x, 65, 8);
    }
    run_ticks(&mut w, lava_ring() * 8);

    for x in 3..=13 {
        for z in 4..=12 {
            assert_eq!(block(&w, x, 65, z), Block::Air, "lava left at ({x},65,{z})");
        }
    }
}
