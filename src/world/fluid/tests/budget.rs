use super::*;

/// Air cells in the loaded fixture, well inside its streamed area: a flow
/// check there finds no fluid and returns, so only the scheduler is measured.
fn air_cells() -> Vec<IVec3> {
    (66..=85)
        .flat_map(|y| (0..=10).flat_map(move |z| (0..=10).map(move |x| IVec3::new(x, y, z))))
        .collect()
}

#[test]
fn fluid_checks_past_the_budget_carry_over_to_later_ticks() {
    let mut w = flat_server_world();
    let cells = air_cells();
    assert!(cells.len() > FLUID_CHECKS_PER_TICK);
    for &p in &cells {
        w.schedule_fluid_tick(p, 1);
    }
    run_ticks(&mut w, 1);
    assert_eq!(w.pending_fluid_checks(), cells.len() - FLUID_CHECKS_PER_TICK);
    run_ticks(&mut w, 1);
    assert_eq!(w.pending_fluid_checks(), 0, "the carry-over drains next tick");
}

/// A check the budget pushed out still runs — first thing next tick.
#[test]
fn a_carried_over_flow_check_runs_on_the_next_tick() {
    let mut w = flat_server_world();
    let source = IVec3::new(8, 65, 8);
    assert!(w.set_fluid_world(source, Block::Water, 0));
    for &p in air_cells().iter().take(FLUID_CHECKS_PER_TICK) {
        w.schedule_fluid_tick(p, 1);
    }
    w.schedule_fluid_tick(source, 1);
    run_ticks(&mut w, 1);
    assert_eq!(block(&w, 9, 65, 8), Block::Air, "over budget: not yet run");
    run_ticks(&mut w, 1);
    assert_eq!(block(&w, 9, 65, 8), Block::Water, "carried over and run");
}

#[test]
fn a_fluid_batch_announces_each_touched_cell_once() {
    let mut w = flat_server_world();
    let a = IVec3::new(15, 65, 8); // section (0,4,0)
    let b = IVec3::new(16, 65, 8); // section (1,4,0)
    let seq = w.changes_end();
    let mut announce = FluidAnnounce::default();
    assert!(w.write_fluid_cell(a, Block::Water, 0, &mut announce));
    assert!(w.write_fluid_cell(a, Block::Water, flowing(3), &mut announce));
    assert!(w.write_fluid_cell(b, Block::Water, flowing(2), &mut announce));
    assert!(w.write_fluid_cell(a, Block::Air, 0, &mut announce));
    assert_eq!(announce.cells(), &[a, b]);
    assert_eq!(announce.section_count(), 2);
    // Block updates queue at each cell's first write; a and b neighbour
    // each other, so the two 7-cell sets share two cells.
    assert_eq!(w.data.sim.update_queue.len(), 12);
    assert!(w.changes_since(seq).1.is_empty(), "announced only on flush");
    announce.flush(&mut w);
    let (_, changed, lost) = w.changes_since(seq);
    assert!(!lost);
    assert_eq!(changed, vec![a, b], "one change-log entry per touched cell");
    assert_eq!(w.data.sim.update_queue.len(), 12);
    assert_eq!(block(&w, a.x, a.y, a.z), Block::Air);
}

#[test]
fn cursor_reads_match_the_world_lookups() {
    let mut w = flat_server_world();
    assert!(w.set_fluid_world(IVec3::new(3, 65, 3), Block::Water, 0));
    assert!(w.set_fluid_world(IVec3::new(4, 65, 3), Block::Water, flowing(4)));
    let reads = FluidReads::new(&w);
    // Spans loaded columns and the unloaded ring past the 3x3 fixture.
    for x in -20..=36 {
        for y in 62..=67 {
            for z in -2..=5 {
                let p = IVec3::new(x, y, z);
                assert_eq!(reads.block(p), w.data.physics_block(x, y, z), "{p:?}");
                assert_eq!(reads.meta(p), w.data.fluid_meta_world(x, y, z), "{p:?}");
            }
        }
    }
}
