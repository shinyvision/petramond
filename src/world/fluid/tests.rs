use super::*;
use crate::world::testutil::flat_server_world;
use crate::world::{ReplicaWorld, ServerWorld};
use petramond_math::world_pos::WorldPos;

fn water_flow_delay() -> u64 {
    Block::Water.fluid_def().unwrap().delay
}
fn lava_flow_delay() -> u64 {
    Block::Lava.fluid_def().unwrap().delay
}

mod budget;
mod quenching;
mod source_renewal;

fn run_ticks(w: &mut ServerWorld, n: u32) {
    let recipes = petramond_world::crafting::Recipes::default();
    for _ in 0..n {
        w.game_tick(&recipes);
    }
}

fn block(w: &ServerWorld, x: i32, y: i32, z: i32) -> Block {
    Block::from_id(w.data.chunk_block(x, y, z))
}

fn carve(w: &mut ServerWorld, x: i32, y: i32, z: i32) {
    w.set_block_world(x, y, z, Block::Air);
}

fn ring() -> u32 {
    water_flow_delay() as u32 + 2
}

#[test]
fn bucket_source_check_accepts_only_still_sources() {
    let mut w = flat_server_world();
    assert!(w.set_fluid_world(IVec3::new(2, 65, 2), Block::Water, 0));
    assert!(w.set_fluid_world(IVec3::new(3, 65, 2), Block::Water, flowing(3)));
    assert!(w.set_fluid_world(IVec3::new(4, 65, 2), Block::Water, FALLING));

    assert!(w.is_water_source_world(IVec3::new(2, 65, 2)));
    assert!(!w.is_water_source_world(IVec3::new(3, 65, 2)), "flowing");
    assert!(!w.is_water_source_world(IVec3::new(4, 65, 2)), "falling");
    assert!(!w.is_water_source_world(IVec3::new(5, 65, 2)), "air");
    assert!(!w.is_water_source_world(IVec3::new(2, 64, 2)), "stone");
}

#[test]
fn water_flow_dir_matches_surface_gradient_used_by_texture() {
    let mut w = flat_server_world();
    for x in 0..=5 {
        w.set_block_world(x, 65, 7, Block::Stone);
        w.set_block_world(x, 65, 9, Block::Stone);
    }
    assert!(w.set_fluid_world(IVec3::new(2, 65, 8), Block::Water, 0));
    assert!(w.set_fluid_world(IVec3::new(3, 65, 8), Block::Water, flowing(4)));

    let dir = w.data.fluid_flow_dir_at(3, 65, 8, Block::Water);
    assert!(dir.x > 0.99, "expected eastward flow, got {dir:?}");
    assert!(dir.z.abs() < 1e-5, "expected no sideways flow, got {dir:?}");
}

#[test]
fn flow_at_a_point_stops_above_the_fluid_surface() {
    let mut w = flat_server_world();
    for x in 0..=5 {
        w.set_block_world(x, 65, 7, Block::Stone);
        w.set_block_world(x, 65, 9, Block::Stone);
    }
    assert!(w.set_fluid_world(IVec3::new(2, 65, 8), Block::Water, 0));
    assert!(w.set_fluid_world(IVec3::new(3, 65, 8), Block::Water, flowing(4)));

    let submerged = w.data.fluid_current_at(WorldPos::new(3.5, 65.2, 8.5));
    assert!(
        submerged.velocity.x > 0.0,
        "a submerged probe drifts: {submerged:?}"
    );
    let skimming = w.data.fluid_current_at(WorldPos::new(3.5, 65.9375, 8.5));
    assert_eq!(
        skimming,
        petramond_world::fluid::FluidCurrent::NONE,
        "above the surface there is no water"
    );
    let source_top = w.data.fluid_current_at(WorldPos::new(2.5, 65.9375, 8.5));
    assert_eq!(
        source_top,
        petramond_world::fluid::FluidCurrent::NONE,
        "a source tops out at 8/9 too"
    );
    assert!(w.set_fluid_world(IVec3::new(3, 66, 8), Block::Water, flowing(1)));
    let capped = w.data.fluid_current_at(WorldPos::new(3.5, 65.9375, 8.5));
    assert!(
        capped.velocity.length_squared() > 0.0,
        "water above caps the cell full: {capped:?}"
    );
}

#[test]
fn game_tick_advances_and_block_update_schedules_a_water_check() {
    let mut w = flat_server_world();
    assert_eq!(w.current_tick(), 0);
    w.set_block_world(8, 65, 8, Block::Water);
    w.game_tick(&petramond_world::crafting::Recipes::default());
    assert_eq!(w.current_tick(), 1);
    assert_eq!(block(&w, 9, 65, 8), Block::Air);
    run_ticks(&mut w, water_flow_delay() as u32 + 1);
    assert_eq!(block(&w, 9, 65, 8), Block::Water);
}

#[test]
fn source_spreads_one_ring_per_delay_on_a_flat_floor() {
    let mut w = flat_server_world();
    w.set_block_world(8, 65, 8, Block::Water);
    run_ticks(&mut w, ring());
    for (dx, dz) in [(0, -1), (1, 0), (0, 1), (-1, 0)] {
        let (x, z) = (8 + dx, 8 + dz);
        assert_eq!(block(&w, x, 65, z), Block::Water, "cardinal {dx},{dz}");
        assert_eq!(level(w.data.fluid_meta_world(x, 65, z)), 1);
        assert!(!is_source(w.data.fluid_meta_world(x, 65, z)));
    }
    assert_eq!(block(&w, 9, 65, 9), Block::Air);
    assert!(is_source(w.data.fluid_meta_world(8, 65, 8)));
}

#[test]
fn flowing_water_dies_out_after_seven_blocks() {
    let mut w = flat_server_world();
    w.set_block_world(8, 65, 8, Block::Water);
    run_ticks(&mut w, 200);
    assert_eq!(level(w.data.fluid_meta_world(12, 65, 8)), 4);
    assert_eq!(level(w.data.fluid_meta_world(15, 65, 8)), 7);
    assert_eq!(block(&w, 16, 65, 8), Block::Air);
}

#[test]
fn source_prefers_flowing_toward_a_downhill_drop() {
    let mut w = flat_server_world();
    carve(&mut w, 10, 64, 8);
    w.set_block_world(8, 65, 8, Block::Water);
    run_ticks(&mut w, 12);
    assert_eq!(block(&w, 9, 65, 8), Block::Water);
    assert_eq!(block(&w, 7, 65, 8), Block::Air);
    assert_eq!(block(&w, 8, 65, 7), Block::Air);
    assert_eq!(block(&w, 8, 65, 9), Block::Air);
}

#[test]
fn slope_search_sees_a_drop_five_cells_out_but_not_six() {
    let mut w = flat_server_world();
    carve(&mut w, 13, 64, 8);
    w.set_block_world(8, 65, 8, Block::Water);
    run_ticks(&mut w, ring());
    assert_eq!(block(&w, 9, 65, 8), Block::Water, "toward the drop");
    assert_eq!(block(&w, 7, 65, 8), Block::Air, "away from the drop");
    assert_eq!(block(&w, 8, 65, 7), Block::Air);

    let mut w = flat_server_world();
    carve(&mut w, 14, 64, 8);
    w.set_block_world(8, 65, 8, Block::Water);
    run_ticks(&mut w, ring());
    for (dx, dz) in [(0, -1), (1, 0), (0, 1), (-1, 0)] {
        assert_eq!(
            block(&w, 8 + dx, 65, 8 + dz),
            Block::Water,
            "unseen drop: spread all ways ({dx},{dz})"
        );
    }
}

#[test]
fn water_pours_one_block_per_tick_not_the_whole_column_at_once() {
    let mut w = flat_server_world();
    w.set_block_world(8, 70, 8, Block::Water);

    run_ticks(&mut w, ring());
    assert!(
        is_falling(w.data.fluid_meta_world(8, 69, 8)),
        "the block just below the source should be falling"
    );
    assert_eq!(
        block(&w, 8, 65, 8),
        Block::Air,
        "water must not have fallen all the way down in one tick"
    );

    run_ticks(&mut w, 6 * water_flow_delay() as u32);
    for y in 65..=69 {
        assert_eq!(block(&w, 8, y, 8), Block::Water, "column y={y}");
        assert!(
            is_falling(w.data.fluid_meta_world(8, y, 8)),
            "falling y={y}"
        );
    }
    assert_eq!(block(&w, 8, 64, 8), Block::Stone);
}

#[test]
fn a_source_over_air_pours_straight_down_and_never_fans_out() {
    let mut w = flat_server_world();
    w.set_block_world(8, 70, 8, Block::Water);

    run_ticks(&mut w, ring());
    assert!(
        is_falling(w.data.fluid_meta_world(8, 69, 8)),
        "poured below"
    );
    assert_eq!(block(&w, 9, 70, 8), Block::Air, "no sideways spread");

    run_ticks(&mut w, ring() * 8);
    for (x, z) in [(9, 8), (7, 8), (8, 9), (8, 7)] {
        assert_eq!(block(&w, x, 70, z), Block::Air, "no fan-out from the head");
    }
    for y in 66..=69 {
        assert!(
            is_falling(w.data.fluid_meta_world(8, y, 8)),
            "one column at y={y}"
        );
        assert_eq!(block(&w, 9, y, 8), Block::Air, "one column wide at y={y}");
    }
    assert_eq!(
        block(&w, 9, 65, 8),
        Block::Water,
        "the landing ring spreads"
    );
}

#[test]
fn a_source_on_a_pool_surface_spreads_across_it_but_its_flow_does_not_creep() {
    let mut w = flat_server_world();
    w.set_block_world(8, 65, 8, Block::Water);
    w.set_block_world(8, 66, 8, Block::Water);
    run_ticks(&mut w, 3 * ring());

    assert_eq!(block(&w, 9, 66, 8), Block::Water, "sheets over the pool");
    assert_eq!(
        block(&w, 10, 66, 8),
        Block::Air,
        "the flowing sheet, over water itself, must not creep onward"
    );
}

#[test]
fn flowing_water_over_a_drop_only_goes_down_not_sideways() {
    let mut w = flat_server_world();
    for x in 0..14 {
        for y in 65..=66 {
            w.set_block_world(x, y, 7, Block::Stone);
            w.set_block_world(x, y, 9, Block::Stone);
        }
    }
    carve(&mut w, 10, 64, 8);
    w.set_block_world(10, 63, 8, Block::Stone);
    w.set_block_world(7, 65, 8, Block::Water);
    run_ticks(&mut w, 300);

    assert_eq!(
        block(&w, 10, 65, 8),
        Block::Water,
        "stream reached the drop"
    );
    assert!(
        is_falling(w.data.fluid_meta_world(10, 64, 8)),
        "the cell over the hole should pour straight down"
    );
    assert_eq!(
        block(&w, 11, 65, 8),
        Block::Air,
        "must not creep past the drop"
    );
    assert_eq!(
        block(&w, 12, 65, 8),
        Block::Air,
        "must not creep past the drop"
    );
}

#[test]
fn flowing_water_does_not_flow_on_top_of_flowing_water() {
    let mut w = flat_server_world();
    for x in 5..14 {
        carve(&mut w, x, 64, 8);
        w.set_block_world(x, 62, 8, Block::Stone);
        for y in 63..=66 {
            w.set_block_world(x, y, 7, Block::Stone);
            w.set_block_world(x, y, 9, Block::Stone);
        }
    }
    w.set_block_world(6, 63, 8, Block::Water);
    w.set_block_world(6, 64, 8, Block::Water);
    run_ticks(&mut w, 400);

    assert_eq!(
        block(&w, 11, 63, 8),
        Block::Water,
        "lower sheet should spread"
    );
    assert_eq!(
        block(&w, 10, 64, 8),
        Block::Air,
        "flowing water must not flow on water"
    );
    assert_eq!(
        block(&w, 12, 64, 8),
        Block::Air,
        "flowing water must not climb the channel"
    );
}

#[test]
fn cut_off_waterfall_and_pool_fully_drain() {
    let mut w = flat_server_world();
    w.set_block_world(8, 65, 8, Block::Stone);
    w.set_block_world(8, 66, 8, Block::Stone);
    w.set_block_world(8, 67, 8, Block::Water);
    run_ticks(&mut w, 250);

    let any_falling = (60..68).any(|y| {
        [(7, 8), (9, 8), (8, 7), (8, 9)]
            .iter()
            .any(|&(x, z)| block(&w, x, y, z) == Block::Water)
    });
    let any_pool = block(&w, 6, 65, 8) == Block::Water;
    assert!(
        any_falling && any_pool,
        "setup should produce a waterfall + pool"
    );

    w.set_block_world(8, 67, 8, Block::Air);
    run_ticks(&mut w, 600);

    for y in 65..=68 {
        for z in 0..16 {
            for x in 0..16 {
                assert_ne!(
                    block(&w, x, y, z),
                    Block::Water,
                    "water left at ({x},{y},{z}) — flowing water sustained itself"
                );
            }
        }
    }
}

#[test]
fn flowing_water_recedes_when_its_source_is_removed() {
    let mut w = flat_server_world();
    w.set_block_world(8, 65, 8, Block::Water);
    run_ticks(&mut w, 40);
    assert_eq!(block(&w, 10, 65, 8), Block::Water);

    w.set_block_world(8, 65, 8, Block::Air);
    run_ticks(&mut w, 200);
    for r in 1..=4 {
        assert_eq!(
            block(&w, 8 + r, 65, 8),
            Block::Air,
            "ring {r} should be dry"
        );
    }
    assert_eq!(block(&w, 8, 65, 8), Block::Air);
}

#[test]
fn a_cut_off_sheet_steps_down_through_levels_rather_than_vanishing() {
    let mut w = flat_server_world();
    w.set_block_world(8, 65, 8, Block::Water);
    run_ticks(&mut w, 60);
    assert_eq!(level(w.data.fluid_meta_world(9, 65, 8)), 1);

    w.set_block_world(8, 65, 8, Block::Air);
    run_ticks(&mut w, ring());
    assert_eq!(block(&w, 9, 65, 8), Block::Water);
    assert_eq!(level(w.data.fluid_meta_world(9, 65, 8)), 3);
}

#[test]
fn flowing_water_washes_away_a_fragile_plant() {
    let mut w = flat_server_world();
    let flower = IVec3::new(10, 65, 8);
    w.set_block_world(flower.x, flower.y, flower.z, Block::Poppy);
    w.set_block_world(8, 65, 8, Block::Water);
    run_ticks(&mut w, 80);

    assert_eq!(
        block(&w, flower.x, flower.y, flower.z),
        Block::Water,
        "water should flood the flower's cell, washing it away"
    );
    let breaks = w.take_natural_breaks();
    assert!(
        breaks
            .iter()
            .any(|&(p, b)| p == flower && b == Block::Poppy),
        "the washed-away flower was recorded for its drop + burst"
    );
}

#[test]
fn a_water_write_reschedules_the_light() {
    use petramond_world::chunk::SECTION_VOLUME;
    let mut w = flat_server_world();
    let cell = IVec3::new(10, 65, 8);
    w.set_block_world(cell.x, cell.y, cell.z, Block::Torch);

    w.section_at_world_mut_for_test(cell.x, cell.y, cell.z)
        .unwrap()
        .set_skylight(vec![0u8; SECTION_VOLUME].into());
    assert!(
        !w.section_at_world_for_test(cell.x, cell.y, cell.z)
            .unwrap()
            .light_dirty,
        "baseline: the section's light is settled"
    );

    assert!(w.set_fluid_world(cell, Block::Water, FALLING));
    assert_eq!(block(&w, cell.x, cell.y, cell.z), Block::Water);
    assert!(
        w.section_at_world_for_test(cell.x, cell.y, cell.z)
            .unwrap()
            .light_dirty,
        "a water write must reschedule the relight"
    );
}

#[test]
fn one_deep_flow_between_two_sources_becomes_a_source() {
    let mut w = flat_server_world();
    w.set_block_world(7, 65, 8, Block::Water);
    w.set_block_world(9, 65, 8, Block::Water);
    run_ticks(&mut w, 60);

    assert_eq!(
        block(&w, 8, 65, 8),
        Block::Water,
        "the gap filled with water"
    );
    assert!(
        is_source(w.data.fluid_meta_world(8, 65, 8)),
        "a flow on solid ground flanked by two sources must become a source"
    );
    assert!(is_source(w.data.fluid_meta_world(7, 65, 8)));
    assert!(is_source(w.data.fluid_meta_world(9, 65, 8)));
    assert!(
        !is_source(w.data.fluid_meta_world(8, 65, 7)),
        "the surrounding ring (one source neighbour) stays flowing"
    );
}

#[test]
fn a_one_deep_flow_resting_on_a_source_becomes_a_source() {
    let mut w = flat_server_world();
    assert!(w.set_fluid_world(IVec3::new(8, 65, 8), Block::Water, 0));
    assert!(w.set_fluid_world(IVec3::new(7, 66, 8), Block::Water, 0));
    assert!(w.set_fluid_world(IVec3::new(9, 66, 8), Block::Water, 0));
    assert!(w.set_fluid_world(IVec3::new(8, 66, 8), Block::Water, flowing(1)));
    run_ticks(&mut w, 30);

    assert!(
        is_source(w.data.fluid_meta_world(8, 66, 8)),
        "a flow resting on a source, flanked by two sources, converts"
    );
}

#[test]
fn a_falling_cell_between_two_sources_converts_to_a_source() {
    let mut w = flat_server_world();
    w.set_block_world(7, 65, 8, Block::Water);
    w.set_block_world(9, 65, 8, Block::Water);
    assert!(w.set_fluid_world(IVec3::new(8, 65, 8), Block::Water, FALLING));
    run_ticks(&mut w, 30);

    assert!(
        is_source(w.data.fluid_meta_world(8, 65, 8)),
        "a falling cell flanked by two sources over solid ground converts"
    );
}

#[test]
fn a_flow_over_a_drop_never_converts_even_between_two_sources() {
    let mut w = flat_server_world();
    carve(&mut w, 8, 64, 8);
    w.set_block_world(7, 65, 8, Block::Water);
    w.set_block_world(9, 65, 8, Block::Water);
    run_ticks(&mut w, 80);

    assert_eq!(
        block(&w, 8, 65, 8),
        Block::Water,
        "the gap still carries flowing water poured from the sources"
    );
    assert!(
        !is_source(w.data.fluid_meta_world(8, 65, 8)),
        "a flow resting on air (a waterfall lip) must never become a source"
    );
}

fn cut_cascade(w: &mut ServerWorld) -> (Vec<IVec3>, (IVec3, IVec3)) {
    for y in 52..64 {
        for z in -10..=10 {
            for x in -10..=10 {
                w.set_block_world(x, y, z, Block::Stone);
            }
        }
    }
    let mut cells: Vec<IVec3> = Vec::new();
    let mut fill = |w: &mut ServerWorld, xs: std::ops::RangeInclusive<i32>, top: i32| {
        for x in xs {
            for z in -2..=2i32 {
                for y in (top - 1)..=top {
                    let p = IVec3::new(x, y, z);
                    assert!(w.set_fluid_world(p, Block::Water, 0));
                    cells.push(p);
                }
                for y in (top + 1)..=64 {
                    w.set_block_world(x, y, z, Block::Air);
                }
            }
        }
    };
    fill(w, -4..=-1, 64);
    fill(w, 1..=4, 60);
    for z in -2..=2 {
        w.set_block_world(0, 64, z, Block::Air);
    }
    cells.sort_by_key(|p| (p.y, p.z, p.x));
    (cells, (IVec3::new(-4, 59, -2), IVec3::new(4, 64, 2)))
}

fn water_cells(w: &ServerWorld) -> Vec<IVec3> {
    let mut out = Vec::new();
    for y in 50..70 {
        for z in -10..=10 {
            for x in -10..=10 {
                if block(w, x, y, z) == Block::Water {
                    out.push(IVec3::new(x, y, z));
                }
            }
        }
    }
    out
}

#[test]
fn a_cut_cascade_flows_inside_its_gorge_and_a_breached_one_does_not() {
    let mut w = flat_server_world();
    let (generated, (lo, hi)) = cut_cascade(&mut w);
    assert!(
        generated
            .iter()
            .all(|&p| is_source(w.data().fluid_meta_world(p.x, p.y, p.z))),
        "worldgen water is meta 0 — a still source"
    );
    let inside = |p: &IVec3| {
        (p.x >= lo.x && p.x <= hi.x) && (p.y >= lo.y && p.y <= hi.y) && (p.z >= lo.z && p.z <= hi.z)
    };

    w.set_block_world(-2, 65, 0, Block::Stone);
    w.set_block_world(-2, 65, 0, Block::Air);
    for &p in &generated {
        w.schedule_fluid_tick(p, water_flow_delay());
    }
    run_ticks(&mut w, ring() * 12);

    let wet = water_cells(&w);
    assert!(
        wet.len() > generated.len(),
        "the cascade never ran: {} wet cells, same as generated — a chain that \
         does not pour is the failure mode, not the success one",
        wet.len()
    );
    assert!(
        wet.iter().any(|p| p.x > 0 && p.y > 61),
        "no water in the gorge over the LOWER pool: the fall never formed"
    );
    let out: Vec<&IVec3> = wet.iter().filter(|p| !inside(p)).collect();
    assert!(
        out.is_empty(),
        "water escaped the gorge at {:?}",
        &out[..out.len().min(8)]
    );

    for y in 60..=64 {
        w.set_block_world(5, y, 0, Block::Air);
    }
    for x in 6..=9 {
        w.set_block_world(x, 60, 0, Block::Air);
    }
    w.set_block_world(4, 60, 0, Block::Air);
    w.set_block_world(4, 60, 0, Block::Water);
    run_ticks(&mut w, ring() * 12);
    assert!(
        water_cells(&w).iter().any(|p| p.x > hi.x),
        "a breached gorge must leak, or the contained half of this test proves \
         nothing"
    );
}

fn lava_ring() -> u32 {
    lava_flow_delay() as u32 + 2
}

#[test]
fn lava_spreads_slower_and_shorter_than_water() {
    let mut w = flat_server_world();
    assert!(w.set_fluid_world(IVec3::new(2, 65, 2), Block::Lava, 0));
    run_ticks(&mut w, lava_ring());
    assert_eq!(
        block(&w, 3, 65, 2),
        Block::Lava,
        "one ring reaches one cell"
    );
    assert_eq!(w.data.fluid_meta_world(3, 65, 2), flowing(8 - 6));
    run_ticks(&mut w, lava_ring() * 2);
    assert_eq!(block(&w, 5, 65, 2), Block::Lava);
    assert_eq!(block(&w, 6, 65, 2), Block::Air, "lava reaches three cells");
}

#[test]
fn lava_is_insulated_from_water_flow_probes() {
    let mut w = flat_server_world();
    assert!(w.set_fluid_world(IVec3::new(2, 65, 2), Block::Lava, 0));
    assert_eq!(
        w.data.fluid_current_at(WorldPos::new(2.5, 65.2, 2.5)),
        petramond_world::fluid::FluidCurrent::NONE,
        "lava is a hazard, not a conveyor"
    );
    assert!(w
        .data
        .is_fluid_source_world(IVec3::new(2, 65, 2), Block::Lava));
    assert!(!w.is_water_source_world(IVec3::new(2, 65, 2)));
}

#[test]
fn lava_pours_down_as_falling_cells_and_lands_in_a_ring() {
    let mut w = flat_server_world();
    carve(&mut w, 2, 64, 2);
    assert!(w.set_fluid_world(IVec3::new(2, 65, 2), Block::Lava, 0));
    run_ticks(&mut w, lava_ring());
    assert_eq!(block(&w, 2, 64, 2), Block::Lava);
    assert_eq!(w.data.fluid_meta_world(2, 64, 2), FALLING);
    assert_eq!(
        block(&w, 3, 65, 2),
        Block::Air,
        "no sideways spread over a drop"
    );
}

#[test]
fn horizontal_water_flow_cools_lava_without_displacing_it() {
    let mut w = flat_server_world();
    assert!(w.set_fluid_world(IVec3::new(2, 65, 2), Block::Lava, 0));
    assert!(w.set_fluid_world(IVec3::new(4, 65, 2), Block::Water, 0));
    run_ticks(&mut w, lava_ring() * 6);
    assert_eq!(block(&w, 2, 65, 2), Block::Stone, "the lava quenched");
    for x in 3..=4 {
        assert_ne!(block(&w, x, 65, 2), Block::Lava, "lava never entered x={x}");
    }
}

/// A generated lava fall (a still source in the ceiling, its falling column below) has to survive
/// the streamed-in kick as one fall. The falling cells re-arm and recompute to themselves, the base
/// spreads its landing pool, and the source doesn't creep along the ceiling into a curtain of
/// falls. The source's own check is forced too, so this pins the spread rule, not just what the
/// kick arms.
#[test]
fn a_generated_lava_fall_rearms_and_builds_its_landing_pool() {
    use petramond_world::chunk::{Chunk, ChunkPos, SectionPos, CHUNK_SX, CHUNK_SZ};
    let mut w = ServerWorld::new(0, 1);
    let mut c = Chunk::new(0, 0);
    for z in 0..CHUNK_SZ {
        for x in 0..CHUNK_SX {
            c.set_block(x, 64, z, Block::Stone);
        }
    }
    for z in 2..=8 {
        for x in 2..=8 {
            for y in 65..=68 {
                c.set_block(x, y, z, Block::Air);
            }
            for y in 69..=70 {
                c.set_block(x, y, z, Block::Stone);
            }
        }
    }
    c.set_fluid(5, 69, 5, Block::Lava, 0);
    for y in 65..=68 {
        c.set_fluid(5, y, 5, Block::Lava, FALLING);
    }
    w.insert_chunk_for_test(ChunkPos::new(0, 0), c);

    w.queue_loaded_section_fluid_updates(&[SectionPos::new(0, 4, 0)]);
    w.schedule_fluid_tick(IVec3::new(5, 69, 5), lava_flow_delay());
    run_ticks(&mut w, lava_ring() * 8);

    let lava_cells_at = |w: &ServerWorld, y: i32| -> Vec<(i32, i32)> {
        let mut out = Vec::new();
        for z in 0..CHUNK_SZ as i32 {
            for x in 0..CHUNK_SX as i32 {
                if block(w, x, y, z) == Block::Lava {
                    out.push((x, z));
                }
            }
        }
        out
    };
    assert_eq!(
        lava_cells_at(&w, 69),
        vec![(5, 5)],
        "the ceiling row holds exactly the stamped source"
    );
    assert!(
        is_source(w.data.fluid_meta_world(5, 69, 5)),
        "the pour stays a source"
    );
    for y in 66..=68 {
        assert_eq!(
            lava_cells_at(&w, y),
            vec![(5, 5)],
            "the fall is one column wide at y={y}"
        );
        assert!(
            is_falling(w.data.fluid_meta_world(5, y, 5)),
            "the stamped column at y={y} re-arms to itself"
        );
    }
    assert_eq!(
        block(&w, 4, 65, 5),
        Block::Lava,
        "the fall builds its landing pool"
    );
    assert!(lava_cells_at(&w, 65).len() > 1);
}

#[test]
fn the_kick_arms_a_generated_lava_water_contact_without_air() {
    use petramond_world::chunk::{SectionPos, SECTION_SIZE};
    use petramond_world::section::Section;
    let build = || {
        let mut w = ServerWorld::new(0, 1);
        let mut a = Section::new(0, 4, 0);
        let mut b = Section::new(1, 4, 0);
        for z in 0..SECTION_SIZE {
            for y in 0..SECTION_SIZE {
                for x in 0..SECTION_SIZE {
                    a.set_block(x, y, z, Block::Stone);
                    b.set_block(x, y, z, Block::Stone);
                }
            }
        }
        a.set_fluid(5, 1, 5, Block::Lava, 0);
        a.set_fluid(6, 1, 5, Block::Water, 0);
        a.set_fluid(15, 3, 8, Block::Lava, 0);
        b.set_fluid(0, 3, 8, Block::Water, 0);
        w.insert_section_for_test(SectionPos::new(0, 4, 0), a);
        w.insert_section_for_test(SectionPos::new(1, 4, 0), b);
        w
    };

    for last in [SectionPos::new(0, 4, 0), SectionPos::new(1, 4, 0)] {
        let mut w = build();
        w.queue_loaded_section_fluid_updates(&[last]);
        run_ticks(&mut w, 1);
        assert_eq!(
            block(&w, 15, 67, 8),
            Block::Stone,
            "the seam contact quenches when {last:?} lands last"
        );
        assert_eq!(
            block(&w, 16, 67, 8),
            Block::Water,
            "side contact keeps water"
        );
    }
    let mut w = build();
    w.queue_loaded_section_fluid_updates(&[SectionPos::new(0, 4, 0)]);
    run_ticks(&mut w, 1);
    assert_eq!(
        block(&w, 5, 65, 5),
        Block::Stone,
        "an enclosed interior contact quenches"
    );
}

#[test]
fn a_replica_renders_each_fluids_sheet_at_the_servers_heights() {
    use petramond_world::chunk::{Chunk, ChunkPos, CHUNK_SX, CHUNK_SZ};

    for fluid in [Block::Water, Block::Lava] {
        let desc = fluid_of(fluid).expect("a simulated fluid");
        let pool = std::sync::Arc::new(crate::worker::JobPool::new(1));
        let mut server = ServerWorld::with_pool(0, 1, pool.clone());
        for cz in -1..=1 {
            for cx in -1..=1 {
                let mut c = Chunk::new(cx, cz);
                for z in 0..CHUNK_SZ {
                    for x in 0..CHUNK_SX {
                        c.set_block(x, 64, z, Block::Stone);
                    }
                }
                server.insert_chunk_for_test(ChunkPos::new(cx, cz), c);
            }
        }
        let mut replica = ReplicaWorld::with_pool(0, 1, pool);
        for cp in server.data.columns.keys().copied().collect::<Vec<_>>() {
            replica.install_remote_column(server.column_payload(cp).unwrap());
        }
        for sp in server.data.sections.keys().copied().collect::<Vec<_>>() {
            replica.install_remote_section(server.section_payload(sp).unwrap());
        }

        server.set_replication_capture(true);
        assert!(server.set_fluid_world(IVec3::new(8, 65, 8), fluid, 0));
        let ring = desc.delay as u32 + 2;
        run_ticks(&mut server, ring * 8);
        for d in server.take_block_deltas() {
            replica.apply_remote_delta(d);
        }

        let drop_off = desc.drop_off as i32;
        let reach = (8 + drop_off - 1) / drop_off - 1;
        let height = |w: &ServerWorld, x: i32| {
            fluid_height(w.data.fluid_meta_world(x, 65, 8), block(w, x, 66, 8), fluid)
        };
        let mut prev = height(&server, 8);
        for x in 9..=8 + reach {
            assert_eq!(
                block(&server, x, 65, 8),
                fluid,
                "{fluid:?} reaches {reach} cells"
            );
            let h = height(&server, x);
            assert!(h < prev, "{fluid:?} steps down at x={x}: {h} vs {prev}");
            prev = h;
            assert_eq!(replica.data.chunk_block(x, 65, 8), fluid.id());
            assert_eq!(
                replica.data.fluid_meta_world(x, 65, 8),
                server.data.fluid_meta_world(x, 65, 8),
                "{fluid:?} at x={x}: the replica renders the server's meta"
            );
        }
        assert_eq!(
            block(&server, 9 + reach, 65, 8),
            Block::Air,
            "{fluid:?} ends at its reach"
        );
    }
}
