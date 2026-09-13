use super::*;
use crate::world::engine_behavior::EngineBlockBehavior;
use petramond_world::chunk::{SectionPos, SECTION_SIZE};
use petramond_world::section::Section;

#[test]
fn water_from_above_or_any_side_makes_stone() {
    for direction in [UP].into_iter().chain(CARDINALS) {
        for meta in [0, flowing(2), FALLING] {
            for update_water in [false, true] {
                let mut w = flat_world();
                let lava = IVec3::new(8, 65, 8);
                let water = lava + direction;
                assert!(w.set_fluid_world(lava, Block::Lava, meta));
                assert!(w.set_fluid_world(water, Block::Water, 0));
                assert!(w.set_fluid_world(lava + DOWN, Block::Water, 0));

                FLUID.neighbor_update(&mut w, if update_water { water } else { lava });

                assert_eq!(
                    block(&w, lava.x, lava.y, lava.z),
                    Block::Stone,
                    "water at {direction:?} cools lava with meta {meta} to stone"
                );
                assert_eq!(block(&w, water.x, water.y, water.z), Block::Water);
                assert_eq!(block(&w, lava.x, lava.y - 1, lava.z), Block::Water);
                assert_eq!(w.fluid_meta_world(lava.x, lava.y, lava.z), 0);
            }
        }
    }
}

#[test]
fn falling_fluid_makes_stone_where_it_enters_the_other_fluid() {
    for (incoming, receiving) in [(Block::Lava, Block::Water), (Block::Water, Block::Lava)] {
        let mut w = flat_world();
        // A shaft keeps the contact vertical and the deeper fluid enclosed.
        for y in 63..=69 {
            for z in 7..=9 {
                for x in 7..=9 {
                    if y == 63 || x != 8 || z != 8 {
                        assert!(w.set_block_world(x, y, z, Block::Stone));
                    }
                }
            }
        }
        for y in [64, 65] {
            assert!(w.set_fluid_world(IVec3::new(8, y, 8), receiving, 0));
        }
        let source = IVec3::new(8, 69, 8);
        assert!(w.set_fluid_world(source, incoming, 0));

        for _ in 0..lava_ring() * 4 {
            run_ticks(&mut w, 1);
            if block(&w, 8, 65, 8) == Block::Stone {
                break;
            }
        }

        assert!(w.is_fluid_source_world(source, incoming));
        for y in 66..=68 {
            assert_eq!(block(&w, 8, y, 8), incoming, "{incoming:?} at y={y}");
            assert!(is_falling(w.fluid_meta_world(8, y, 8)));
        }
        assert_eq!(block(&w, 8, 65, 8), Block::Stone, "{incoming:?} landing");
        assert_eq!(w.fluid_meta_world(8, 65, 8), 0);
        assert_eq!(
            block(&w, 8, 64, 8),
            receiving,
            "the stone seals off the deeper {receiving:?}"
        );
    }
}

#[test]
fn lava_above_water_reacts_only_when_it_flows_into_the_lower_cell() {
    for lava_meta in [0, flowing(2), FALLING] {
        for water_meta in [0, flowing(4), FALLING] {
            let mut w = flat_world();
            let lava = IVec3::new(8, 66, 8);
            let water = lava + DOWN;
            assert!(w.set_fluid_world(lava + UP, Block::Lava, 0));
            assert!(w.set_fluid_world(lava, Block::Lava, lava_meta));
            assert!(w.set_fluid_world(water, Block::Water, water_meta));

            for updated in [water, lava, water, lava] {
                FLUID.neighbor_update(&mut w, updated);
                assert_eq!(block(&w, 8, 66, 8), Block::Lava);
                assert_eq!(block(&w, 8, 65, 8), Block::Water);
                assert_eq!(w.fluid_meta_world(8, 66, 8), lava_meta);
                assert_eq!(w.fluid_meta_world(8, 65, 8), water_meta);
            }

            FLUID.scheduled_tick(&mut w, lava);

            assert_eq!(block(&w, 8, 65, 8), Block::Stone, "water meta {water_meta}");
            assert_eq!(w.fluid_meta_world(8, 65, 8), 0);
            assert_eq!(block(&w, 8, 66, 8), Block::Lava);
            for d in CARDINALS {
                let side = lava + d;
                assert_eq!(block(&w, side.x, side.y, side.z), Block::Air);
            }

            run_ticks(&mut w, lava_ring());
            assert_eq!(
                block(&w, 7, 66, 8),
                Block::Lava,
                "the delayed flow check spreads across the new stone floor"
            );
        }
    }
}

#[test]
fn descending_lava_blocks_a_waterfall_only_when_it_reaches_the_water() {
    let mut w = flat_world();
    // A water channel ends underneath the lava, then spills down a shaft.
    for y in 65..=70 {
        for x in 5..=9 {
            for z in [7, 9] {
                assert!(w.set_block_world(x, y, z, Block::Stone));
            }
        }
        for x in [5, 9] {
            assert!(w.set_block_world(x, y, 8, Block::Stone));
        }
        if y <= 67 || y >= 69 {
            for x in [6, 7] {
                assert!(w.set_block_world(x, y, 8, Block::Stone));
            }
        }
    }
    let lava = IVec3::new(8, 69, 8);
    assert!(w.set_fluid_world(lava + UP, Block::Lava, 0));
    assert!(w.set_fluid_world(lava, Block::Lava, FALLING));
    assert!(w.set_fluid_world(IVec3::new(6, 68, 8), Block::Water, 0));

    for _ in 0..lava_flow_delay() {
        run_ticks(&mut w, 1);
        assert_eq!(block(&w, 8, 69, 8), Block::Lava);
        assert_ne!(block(&w, 8, 68, 8), Block::Stone);
    }
    for y in 65..=68 {
        assert_eq!(block(&w, 8, y, 8), Block::Water, "waterfall at y={y}");
    }

    run_ticks(&mut w, 1);

    assert_eq!(block(&w, 8, 68, 8), Block::Stone);
    assert_eq!(block(&w, 8, 69, 8), Block::Lava);
    assert!(is_falling(w.fluid_meta_world(8, 69, 8)));
    assert!(w.is_fluid_source_world(lava + UP, Block::Lava));

    run_ticks(&mut w, ring() * 10);

    assert_eq!(block(&w, 7, 68, 8), Block::Water);
    assert_eq!(block(&w, 8, 68, 8), Block::Stone);
    assert_eq!(block(&w, 8, 69, 8), Block::Lava);
    for y in 65..=67 {
        assert_eq!(block(&w, 8, y, 8), Block::Air, "cut-off waterfall at y={y}");
    }
}

#[test]
fn water_cools_its_other_contacts_while_the_falling_lava_waits_for_its_update() {
    let mut w = flat_world();
    let water = IVec3::new(8, 67, 8);
    assert!(w.set_fluid_world(water, Block::Water, 0));
    assert!(w.set_fluid_world(water + UP, Block::Lava, FALLING));
    assert!(w.set_fluid_world(water + UP + UP, Block::Lava, 0));
    for direction in [DOWN, IVec3::X] {
        assert!(w.set_fluid_world(water + direction, Block::Lava, 0));
    }

    FLUID.neighbor_update(&mut w, water);

    assert_eq!(block(&w, 8, 67, 8), Block::Water);
    assert_eq!(block(&w, 8, 68, 8), Block::Lava);
    assert_eq!(block(&w, 8, 66, 8), Block::Stone);
    assert_eq!(block(&w, 9, 67, 8), Block::Stone);

    FLUID.neighbor_update(&mut w, water + UP);
    assert_eq!(block(&w, 8, 68, 8), Block::Lava);
    assert_eq!(block(&w, 8, 67, 8), Block::Water);

    FLUID.scheduled_tick(&mut w, water + UP);
    assert_eq!(block(&w, 8, 68, 8), Block::Lava);
    assert_eq!(block(&w, 8, 67, 8), Block::Stone);
}

#[test]
fn unfed_lava_drains_before_it_can_flow_down_into_water() {
    for meta in [flowing(2), flowing(6), FALLING] {
        let mut w = flat_world();
        let lava = IVec3::new(8, 66, 8);
        assert!(w.set_fluid_world(lava, Block::Lava, meta));
        assert!(w.set_fluid_world(lava + DOWN, Block::Water, 0));

        FLUID.neighbor_update(&mut w, lava);

        assert_eq!(block(&w, 8, 65, 8), Block::Water);
        assert_eq!(block(&w, 8, 66, 8), Block::Lava);
        assert_eq!(w.fluid_meta_world(8, 66, 8), meta);

        run_ticks(&mut w, lava_ring());

        assert_eq!(block(&w, 8, 66, 8), Block::Air);
        assert_eq!(block(&w, 8, 65, 8), Block::Water);
    }
}

#[test]
fn water_cools_lava_before_it_can_pour_into_water_below() {
    for direction in [UP].into_iter().chain(CARDINALS) {
        for update_offset in [IVec3::ZERO, direction] {
            let mut w = flat_world();
            let lava = IVec3::new(8, 66, 8);
            assert!(w.set_fluid_world(lava, Block::Lava, 0));
            assert!(w.set_fluid_world(lava + DOWN, Block::Water, 0));
            assert!(w.set_fluid_world(lava + direction, Block::Water, 0));

            FLUID.neighbor_update(&mut w, lava + update_offset);

            assert_eq!(block(&w, 8, 66, 8), Block::Stone);
            assert_eq!(block(&w, 8, 65, 8), Block::Water);
        }
    }
}

#[test]
fn downward_flow_resolves_the_waters_other_contacts_before_consuming_it() {
    for update_water_first in [false, true] {
        let mut w = flat_world();
        let water = IVec3::new(8, 67, 8);
        assert!(w.set_fluid_world(water, Block::Water, 0));
        for direction in [UP, DOWN, IVec3::X] {
            assert!(w.set_fluid_world(water + direction, Block::Lava, 0));
        }

        if update_water_first {
            FLUID.neighbor_update(&mut w, water);
        }
        FLUID.scheduled_tick(&mut w, water + UP);

        assert_eq!(block(&w, 8, 67, 8), Block::Stone);
        assert_eq!(block(&w, 8, 68, 8), Block::Lava);
        assert_eq!(block(&w, 8, 66, 8), Block::Stone);
        assert_eq!(block(&w, 9, 67, 8), Block::Stone);
    }
}

#[test]
fn water_arriving_beside_lava_during_the_same_tick_cools_it_before_it_pours() {
    let mut w = flat_world();
    let water = IVec3::new(6, 66, 8);
    let lava = IVec3::new(8, 66, 8);
    for (x, y, z) in [(6, 65, 8), (7, 65, 8), (5, 66, 8), (6, 66, 7), (6, 66, 9)] {
        assert!(w.set_block_world(x, y, z, Block::Stone));
    }
    assert!(w.set_fluid_world(water, Block::Water, 0));
    assert!(w.set_fluid_world(lava, Block::Lava, FALLING));
    assert!(w.set_fluid_world(lava + UP, Block::Lava, 0));
    assert!(w.set_fluid_world(lava + DOWN, Block::Water, 0));
    w.schedule_block_tick(water, 1);
    w.schedule_block_tick(lava, 1);

    run_ticks(&mut w, 1);

    assert_eq!(block(&w, 7, 66, 8), Block::Water);
    assert_eq!(block(&w, 8, 66, 8), Block::Stone);
    assert_eq!(block(&w, 8, 65, 8), Block::Water);
    assert!(w.is_fluid_source_world(lava + UP, Block::Lava));
}

#[test]
fn contact_waits_for_stream_final_neighbors_then_reacts_on_the_update() {
    let mut w = flat_world();
    assert!(w.set_fluid_world(IVec3::new(15, 65, 8), Block::Lava, 0));
    assert!(w.set_fluid_world(IVec3::new(16, 65, 8), Block::Water, 0));
    let pending = SectionPos::new(1, 4, 0);
    w.gen.awaited_overlays.insert(pending);
    w.note_stream_nonfinal(pending);

    run_ticks(&mut w, 1);
    assert_eq!(block(&w, 15, 65, 8), Block::Lava);
    assert_eq!(block(&w, 16, 65, 8), Block::Water);

    w.gen.awaited_overlays.remove(&pending);
    w.settle_stream_nonfinal(pending);
    run_ticks(&mut w, 1);

    assert_eq!(block(&w, 15, 65, 8), Block::Stone);
    assert_eq!(block(&w, 16, 65, 8), Block::Water);
}

#[test]
fn a_refused_downward_quench_neither_spreads_sideways_nor_is_lost() {
    let mut w = flat_world();
    let lava = IVec3::new(8, SECTION_SIZE as i32 * 4, 8);
    for d in CARDINALS {
        carve(&mut w, lava.x + d.x, lava.y, lava.z + d.z);
    }
    assert!(w.set_fluid_world(lava, Block::Lava, 0));
    assert!(w.set_fluid_world(lava + DOWN, Block::Water, 0));
    let receiving = SectionPos::new(0, 3, 0);
    w.insert_pending_section(receiving);

    FLUID.scheduled_tick(&mut w, lava);

    assert_eq!(block(&w, lava.x, lava.y - 1, lava.z), Block::Water);
    for d in CARDINALS {
        let side = lava + d;
        assert_eq!(
            block(&w, side.x, side.y, side.z),
            Block::Air,
            "the unwritable quencher below is not a floor"
        );
    }

    w.remove_pending_section(receiving);
    run_ticks(&mut w, crate::world::sim_guard::SIM_RETRY_DELAY as u32 + 1);
    assert_eq!(
        block(&w, lava.x, lava.y - 1, lava.z),
        Block::Stone,
        "the pour retries before its ordinary flow delay"
    );
    assert!(lava_flow_delay() > crate::world::sim_guard::SIM_RETRY_DELAY + 1);
}

#[test]
fn lava_routes_downhill_into_water() {
    let mut w = flat_world();
    assert!(w.set_block_world(10, 63, 8, Block::Stone));
    assert!(w.set_fluid_world(IVec3::new(10, 64, 8), Block::Water, 0));
    assert!(w.set_fluid_world(IVec3::new(8, 65, 8), Block::Lava, 0));

    run_ticks(&mut w, lava_ring());

    assert_eq!(block(&w, 9, 65, 8), Block::Lava);
    for (x, z) in [(7, 8), (8, 7), (8, 9)] {
        assert_eq!(
            block(&w, x, 65, z),
            Block::Air,
            "flow heads toward the water"
        );
    }

    run_ticks(&mut w, lava_ring() * 2);

    assert_eq!(block(&w, 10, 64, 8), Block::Stone);
    assert_eq!(block(&w, 10, 65, 8), Block::Lava);
}

#[test]
fn vertical_contacts_rearm_across_sections_in_either_load_order() {
    let lower = SectionPos::new(0, 4, 0);
    let upper = SectionPos::new(0, 5, 0);
    for (incoming, receiving, meta) in [
        (Block::Lava, Block::Water, 0),
        (Block::Lava, Block::Water, FALLING),
        (Block::Water, Block::Lava, 0),
    ] {
        for last in [lower, upper] {
            let mut w = World::new(0, 1);
            for pos in [lower, upper] {
                let mut section = Section::new(pos.cx, pos.cy, pos.cz);
                for y in 0..SECTION_SIZE {
                    for z in 0..SECTION_SIZE {
                        for x in 0..SECTION_SIZE {
                            section.set_block(x, y, z, Block::Stone);
                        }
                    }
                }
                if pos == lower {
                    section.set_fluid(8, SECTION_SIZE - 1, 8, receiving, 0);
                } else {
                    section.set_fluid(8, 0, 8, incoming, meta);
                    if is_falling(meta) {
                        section.set_fluid(8, 1, 8, incoming, 0);
                    }
                }
                w.insert_section_for_test(pos, section);
            }

            w.queue_loaded_section_fluid_updates(&[last]);
            run_ticks(&mut w, 1);

            if incoming == Block::Lava {
                assert_eq!(block(&w, 8, 79, 8), receiving);
                assert_eq!(block(&w, 8, 80, 8), incoming);
                run_ticks(&mut w, lava_flow_delay() as u32);
            }
            assert_eq!(
                block(&w, 8, 79, 8),
                Block::Stone,
                "{incoming:?} above {receiving:?}, {last:?} loaded last"
            );
            assert_eq!(block(&w, 8, 80, 8), incoming);
        }
    }
}
