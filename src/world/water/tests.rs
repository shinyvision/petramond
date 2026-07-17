use super::*;
// Source/flow tests place water at y>=65, above flat_world's stone floor.
use crate::world::testutil::flat_world;

fn run_ticks(w: &mut World, n: u32) {
    // Water flow needs no recipes; an empty set keeps the furnace step a no-op.
    let recipes = crate::crafting::Recipes::default();
    for _ in 0..n {
        w.game_tick(&recipes);
    }
}

fn block(w: &World, x: i32, y: i32, z: i32) -> Block {
    Block::from_id(w.chunk_block(x, y, z))
}

fn carve(w: &mut World, x: i32, y: i32, z: i32) {
    w.set_block_world(x, y, z, Block::Air);
}

/// One full ring advance: the flow delay, plus slack for the update dispatch.
const RING: u32 = WATER_FLOW_DELAY as u32 + 2;

#[test]
fn bucket_source_check_accepts_only_still_sources() {
    let mut w = flat_world();
    assert!(w.set_water_world(IVec3::new(2, 65, 2), Block::Water, 0));
    assert!(w.set_water_world(IVec3::new(3, 65, 2), Block::Water, flowing(3)));
    assert!(w.set_water_world(IVec3::new(4, 65, 2), Block::Water, FALLING));

    assert!(w.is_water_source_world(IVec3::new(2, 65, 2)));
    assert!(!w.is_water_source_world(IVec3::new(3, 65, 2)), "flowing");
    assert!(!w.is_water_source_world(IVec3::new(4, 65, 2)), "falling");
    assert!(!w.is_water_source_world(IVec3::new(5, 65, 2)), "air");
    assert!(!w.is_water_source_world(IVec3::new(2, 64, 2)), "stone");
}

#[test]
fn water_flow_dir_matches_surface_gradient_used_by_texture() {
    let mut w = flat_world();
    // A one-wide channel: side walls remove sideways air, so the gradient at
    // the flowing cell points east, the same direction its top texture faces.
    for x in 0..=5 {
        w.set_block_world(x, 65, 7, Block::Stone);
        w.set_block_world(x, 65, 9, Block::Stone);
    }
    assert!(w.set_water_world(IVec3::new(2, 65, 8), Block::Water, 0));
    assert!(w.set_water_world(IVec3::new(3, 65, 8), Block::Water, flowing(4)));

    let dir = w.water_flow_dir_at(3, 65, 8);
    assert!(dir.x > 0.99, "expected eastward flow, got {dir:?}");
    assert!(dir.z.abs() < 1e-5, "expected no sideways flow, got {dir:?}");
}

/// The body-probe sampler only pushes below the fluid's real surface: a
/// probe in a flowing cell's top sliver (feet standing on a 15/16 block
/// beside an irrigation channel) catches no current, while a submerged
/// probe in the same cell does.
#[test]
fn flow_at_a_point_stops_above_the_fluid_surface() {
    let mut w = flat_world();
    for x in 0..=5 {
        w.set_block_world(x, 65, 7, Block::Stone);
        w.set_block_world(x, 65, 9, Block::Stone);
    }
    assert!(w.set_water_world(IVec3::new(2, 65, 8), Block::Water, 0));
    assert!(w.set_water_world(IVec3::new(3, 65, 8), Block::Water, flowing(4)));

    let submerged = w.water_flow_at_point(Vec3::new(3.5, 65.2, 8.5));
    assert!(
        submerged.x > 0.99,
        "a submerged probe drifts: {submerged:?}"
    );
    // 15/16 = 0.9375, above even a full source's 8/9 surface.
    let skimming = w.water_flow_at_point(Vec3::new(3.5, 65.9375, 8.5));
    assert_eq!(skimming, Vec3::ZERO, "above the surface there is no water");
    let source_top = w.water_flow_at_point(Vec3::new(2.5, 65.9375, 8.5));
    assert_eq!(source_top, Vec3::ZERO, "a source tops out at 8/9 too");
    // A capped cell fills to the brim and pushes through its whole height.
    assert!(w.set_water_world(IVec3::new(3, 66, 8), Block::Water, flowing(1)));
    let capped = w.water_flow_at_point(Vec3::new(3.5, 65.9375, 8.5));
    assert!(
        capped.length_squared() > 0.0,
        "water above caps the cell full: {capped:?}"
    );
}

#[test]
fn game_tick_advances_and_block_update_schedules_a_water_check() {
    let mut w = flat_world();
    assert_eq!(w.current_tick(), 0);
    w.set_block_world(8, 65, 8, Block::Water);
    // First tick dispatches the placement update and schedules the flow check;
    // the source has not spread yet.
    w.game_tick(&crate::crafting::Recipes::default());
    assert_eq!(w.current_tick(), 1);
    assert_eq!(block(&w, 9, 65, 8), Block::Air);
    // After the flow delay the source has spread to its cardinal neighbours.
    run_ticks(&mut w, WATER_FLOW_DELAY as u32 + 1);
    assert_eq!(block(&w, 9, 65, 8), Block::Water);
}

#[test]
fn source_spreads_one_ring_per_delay_on_a_flat_floor() {
    let mut w = flat_world();
    w.set_block_world(8, 65, 8, Block::Water);
    run_ticks(&mut w, RING);
    for (dx, dz) in [(0, -1), (1, 0), (0, 1), (-1, 0)] {
        let (x, z) = (8 + dx, 8 + dz);
        assert_eq!(block(&w, x, 65, z), Block::Water, "cardinal {dx},{dz}");
        assert_eq!(level(w.water_meta_world(x, 65, z)), 1);
        assert!(!is_source(w.water_meta_world(x, 65, z)));
    }
    // Diagonals are only reached on a later ring.
    assert_eq!(block(&w, 9, 65, 9), Block::Air);
    // The source itself stays a full source.
    assert!(is_source(w.water_meta_world(8, 65, 8)));
}

#[test]
fn flowing_water_dies_out_after_seven_blocks() {
    let mut w = flat_world();
    w.set_block_world(8, 65, 8, Block::Water);
    // Plenty of rings: 7 spreads at 5 ticks each, plus slack.
    run_ticks(&mut w, 200);
    // The level grows by one per block away from the source...
    assert_eq!(level(w.water_meta_world(12, 65, 8)), 4);
    assert_eq!(level(w.water_meta_world(15, 65, 8)), 7);
    // ...and the flow dies past the last level: the 8th block is dry.
    assert_eq!(block(&w, 16, 65, 8), Block::Air);
}

#[test]
fn source_prefers_flowing_toward_a_downhill_drop() {
    let mut w = flat_world();
    // A hole in the floor two blocks east makes (10,65,8) a drop.
    carve(&mut w, 10, 64, 8);
    w.set_block_world(8, 65, 8, Block::Water);
    run_ticks(&mut w, 12);
    // Water heads east toward the drop only — the other cardinals stay dry.
    assert_eq!(block(&w, 9, 65, 8), Block::Water);
    assert_eq!(block(&w, 7, 65, 8), Block::Air);
    assert_eq!(block(&w, 8, 65, 7), Block::Air);
    assert_eq!(block(&w, 8, 65, 9), Block::Air);
}

/// The slope search steers toward a drop up to five cells out — one ring plus
/// [`SLOPE_FIND_DIST`] steps — and no farther: a drop six cells out is invisible
/// and the flow spreads every open way instead.
#[test]
fn slope_search_sees_a_drop_five_cells_out_but_not_six() {
    let mut w = flat_world();
    carve(&mut w, 13, 64, 8); // five cells east of the source at x=8
    w.set_block_world(8, 65, 8, Block::Water);
    run_ticks(&mut w, RING);
    assert_eq!(block(&w, 9, 65, 8), Block::Water, "toward the drop");
    assert_eq!(block(&w, 7, 65, 8), Block::Air, "away from the drop");
    assert_eq!(block(&w, 8, 65, 7), Block::Air);

    let mut w = flat_world();
    carve(&mut w, 14, 64, 8); // six cells east: out of slope-search range
    w.set_block_world(8, 65, 8, Block::Water);
    run_ticks(&mut w, RING);
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
    let mut w = flat_world();
    // Source floats five blocks above the floor (cells y=65..69 are air).
    w.set_block_world(8, 70, 8, Block::Water);

    // Shortly after it begins to pour, only the TOP of the column has filled —
    // the water has not teleported all the way to the floor.
    run_ticks(&mut w, RING);
    assert!(
        is_falling(w.water_meta_world(8, 69, 8)),
        "the block just below the source should be falling"
    );
    assert_eq!(
        block(&w, 8, 65, 8),
        Block::Air,
        "water must not have fallen all the way down in one tick"
    );

    // Given enough ticks it reaches the floor, one block per tick.
    run_ticks(&mut w, 6 * WATER_FLOW_DELAY as u32);
    for y in 65..=69 {
        assert_eq!(block(&w, 8, y, 8), Block::Water, "column y={y}");
        assert!(is_falling(w.water_meta_world(8, y, 8)), "falling y={y}");
    }
    // It rests on the floor, not inside it.
    assert_eq!(block(&w, 8, 64, 8), Block::Stone);
}

/// A source over open air pours straight down on its first check — and only
/// once its stream exists (water below closes the pour) does its next check
/// take the sideways branch reserved for sources, fanning one ring out. The
/// two-phase order is the visible signature of the down-first spread rule.
#[test]
fn a_source_over_air_pours_first_then_fans_out() {
    let mut w = flat_world();
    w.set_block_world(8, 70, 8, Block::Water);

    // First check: the pour, and nothing else.
    run_ticks(&mut w, RING);
    assert!(is_falling(w.water_meta_world(8, 69, 8)), "poured below");
    assert_eq!(block(&w, 9, 70, 8), Block::Air, "no sideways spread yet");

    // Second check (rescheduled by the stream appearing below): the source
    // now sits on water, and a source on water spreads across it.
    run_ticks(&mut w, RING);
    assert_eq!(block(&w, 9, 70, 8), Block::Water, "fans out after pouring");
    assert!(!is_source(w.water_meta_world(9, 70, 8)));
}

/// A source resting on other water spreads across its surface (that is how a
/// poured bucket sheets over a pond) — but the FLOWING ring it creates, also
/// suspended over water, must not creep any farther on its own.
#[test]
fn a_source_on_a_pool_surface_spreads_across_it_but_its_flow_does_not_creep() {
    let mut w = flat_world();
    w.set_block_world(8, 65, 8, Block::Water); // pool cell on the floor
    w.set_block_world(8, 66, 8, Block::Water); // source resting on it
    run_ticks(&mut w, 3 * RING);

    assert_eq!(block(&w, 9, 66, 8), Block::Water, "sheets over the pool");
    assert_eq!(
        block(&w, 10, 66, 8),
        Block::Air,
        "the flowing sheet, over water itself, must not creep onward"
    );
}

/// Flowing water with a way down only goes down — it must NOT also creep
/// sideways, even after its waterfall is established (the cell's later ticks
/// see falling water below, not air).
#[test]
fn flowing_water_over_a_drop_only_goes_down_not_sideways() {
    let mut w = flat_world();
    // A 1-wide channel at z=8 (walls at z=7/z=9) so the only path east is
    // THROUGH the cell over the drop — water can't go around a flat sheet.
    for x in 0..14 {
        for y in 65..=66 {
            w.set_block_world(x, y, 7, Block::Stone);
            w.set_block_world(x, y, 9, Block::Stone);
        }
    }
    // A one-deep hole at (10,64): floor carved out, a floor one block lower.
    carve(&mut w, 10, 64, 8);
    w.set_block_world(10, 63, 8, Block::Stone);
    // Source three blocks west; water runs east to the hole.
    w.set_block_world(7, 65, 8, Block::Water);
    // Long enough that the cell over the hole ticks many times after pouring.
    run_ticks(&mut w, 300);

    // It reached the drop and poured a falling column...
    assert_eq!(
        block(&w, 10, 65, 8),
        Block::Water,
        "stream reached the drop"
    );
    assert!(
        is_falling(w.water_meta_world(10, 64, 8)),
        "the cell over the hole should pour straight down"
    );
    // ...but never crept east past it (the bug: a cell with falling water below
    // it must keep going down, not start spreading once the column exists).
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

/// Flowing water must NOT treat other flowing water as a surface to flow on —
/// otherwise a layer of flowing water creeps across the top of the water below
/// it and the body climbs higher over time. Two stacked sources feed an upper
/// flow sitting directly over a lower sheet; the upper flow must not propagate.
#[test]
fn flowing_water_does_not_flow_on_top_of_flowing_water() {
    let mut w = flat_world(); // stone floor at y=64
                              // Carve the floor and lay a lower one at y=62, in a 1-wide channel: the
                              // lower sheet sits on y=62 (water at y=63), and the upper flow would sit
                              // directly on that lower water (at y=64).
    for x in 5..14 {
        carve(&mut w, x, 64, 8);
        w.set_block_world(x, 62, 8, Block::Stone);
        for y in 63..=66 {
            w.set_block_world(x, y, 7, Block::Stone);
            w.set_block_world(x, y, 9, Block::Stone);
        }
    }
    // A two-high source wall at x=6: y=63 feeds the lower sheet, y=64 the upper.
    w.set_block_world(6, 63, 8, Block::Water);
    w.set_block_world(6, 64, 8, Block::Water);
    run_ticks(&mut w, 400);

    // The lower sheet spreads out along its floor...
    assert_eq!(
        block(&w, 11, 63, 8),
        Block::Water,
        "lower sheet should spread"
    );
    // ...but the upper level must NOT ride along on top of it. (A single cell
    // beside the source is fine; it must not propagate down the channel.)
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

/// The critical invariant: flowing water can never become its own source.
/// A source on a pillar makes water fall off every side and pool on the floor;
/// once the source is cut, EVERY flowing and falling cell must drain away —
/// nothing may sustain itself (no orphaned waterfalls or self-supporting
/// columns).
#[test]
fn cut_off_waterfall_and_pool_fully_drain() {
    let mut w = flat_world();
    // A 2-high stone pillar at (8,8) with a source on top; water spills off all
    // four sides, falls to the floor (y=64) and pools.
    w.set_block_world(8, 65, 8, Block::Stone);
    w.set_block_world(8, 66, 8, Block::Stone);
    w.set_block_world(8, 67, 8, Block::Water);
    run_ticks(&mut w, 250);

    // Sanity: water really did fall and pool (else the test proves nothing).
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

    // Cut the source.
    w.set_block_world(8, 67, 8, Block::Air);
    run_ticks(&mut w, 600);

    // Nothing may remain anywhere in the region.
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
    let mut w = flat_world();
    w.set_block_world(8, 65, 8, Block::Water);
    run_ticks(&mut w, 40); // let the sheet form
    assert_eq!(block(&w, 10, 65, 8), Block::Water);

    // Remove the source; the sheet must drain back to nothing.
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

/// Draining is the re-level rule, not a special path: a cut-off cell steps
/// DOWN through the levels as its (equally doomed) neighbours weaken, rather
/// than vanishing outright — its first re-check leans on the still-stale ring
/// beyond it and lands two levels weaker, not dry.
#[test]
fn a_cut_off_sheet_steps_down_through_levels_rather_than_vanishing() {
    let mut w = flat_world();
    w.set_block_world(8, 65, 8, Block::Water);
    run_ticks(&mut w, 60); // full sheet: level == distance from source
    assert_eq!(level(w.water_meta_world(9, 65, 8)), 1);

    w.set_block_world(8, 65, 8, Block::Air);
    run_ticks(&mut w, RING);
    // One re-check later: fed only by the level-2 ring (amount 6), the old
    // level-1 cell now carries amount 5 — level 3. Still water, weaker.
    assert_eq!(block(&w, 9, 65, 8), Block::Water);
    assert_eq!(level(w.water_meta_world(9, 65, 8)), 3);
}

#[test]
fn flowing_water_washes_away_a_fragile_plant() {
    let mut w = flat_world();
    // A flower standing on the floor, in the path of a source two cells west.
    let flower = IVec3::new(10, 65, 8);
    w.set_block_world(flower.x, flower.y, flower.z, Block::Poppy);
    w.set_block_world(8, 65, 8, Block::Water);
    run_ticks(&mut w, 80); // let the sheet reach the flower's cell

    // Water flowed INTO the fragile cell, displacing the plant (a fragile block
    // counts as fillable; the flower didn't stay standing in the water).
    assert_eq!(
        block(&w, flower.x, flower.y, flower.z),
        Block::Water,
        "water should flood the flower's cell, washing it away"
    );
    // ...and it was recorded as a hand-style break (drop + particle burst).
    let breaks = w.take_natural_breaks();
    assert!(
        breaks
            .iter()
            .any(|&(p, b)| p == flower && b == Block::Poppy),
        "the washed-away flower was recorded for its drop + burst"
    );
}

/// A water write must schedule the matching relight, like every other block
/// update. The water path used to skip it ("water is transparent"), but water
/// can move INTO a cell that held a torch (a light emitter) and wash it away —
/// and then the torch's glow lingered in the now-stale light. We settle the
/// owning section's light first so a still-dirty band can't mask the regression.
#[test]
fn a_water_write_reschedules_the_light() {
    use crate::chunk::SECTION_VOLUME;
    let mut w = flat_world();
    let cell = IVec3::new(10, 65, 8); // section (0,4,0)
    w.set_block_world(cell.x, cell.y, cell.z, Block::Torch);

    // Install a settled skylight cube so the section's `light_dirty` flag is clear —
    // the baseline a fresh block update has to dirty again.
    w.section_at_world_mut_for_test(cell.x, cell.y, cell.z)
        .unwrap()
        .set_skylight(vec![0u8; SECTION_VOLUME].into());
    assert!(
        !w.section_at_world_for_test(cell.x, cell.y, cell.z)
            .unwrap()
            .light_dirty,
        "baseline: the section's light is settled"
    );

    // Water moves into the torch's cell; the announce must re-dirty the light
    // so the lingering emitter glow gets rebaked.
    assert!(w.set_water_world(cell, Block::Water, FALLING));
    assert_eq!(block(&w, cell.x, cell.y, cell.z), Block::Water);
    assert!(
        w.section_at_world_for_test(cell.x, cell.y, cell.z)
            .unwrap()
            .light_dirty,
        "a water write must reschedule the relight"
    );
}

/// The infinite-water-source rule: two sources two cells apart on a solid
/// floor fill the gap with a flow fed from both sides, and that flow settles
/// into a source of its own.
#[test]
fn one_deep_flow_between_two_sources_becomes_a_source() {
    let mut w = flat_world();
    w.set_block_world(7, 65, 8, Block::Water);
    w.set_block_world(9, 65, 8, Block::Water);
    run_ticks(&mut w, 60);

    assert_eq!(
        block(&w, 8, 65, 8),
        Block::Water,
        "the gap filled with water"
    );
    assert!(
        is_source(w.water_meta_world(8, 65, 8)),
        "a flow on solid ground flanked by two sources must become a source"
    );
    // The flanking sources are of course still sources, and the conversion
    // did not run away across the open floor.
    assert!(is_source(w.water_meta_world(7, 65, 8)));
    assert!(is_source(w.water_meta_world(9, 65, 8)));
    assert!(
        !is_source(w.water_meta_world(8, 65, 7)),
        "the surrounding ring (one source neighbour) stays flowing"
    );
}

/// A flow resting on a SOURCE counts as grounded too, so the top layer of a
/// stacked pool can fill in as well. Stage a lower source, two flanking
/// sources above it, and a flow between them: the flow rests on the lower
/// source and converts.
#[test]
fn a_one_deep_flow_resting_on_a_source_becomes_a_source() {
    let mut w = flat_world();
    assert!(w.set_water_world(IVec3::new(8, 65, 8), Block::Water, 0)); // lower source
    assert!(w.set_water_world(IVec3::new(7, 66, 8), Block::Water, 0)); // flank
    assert!(w.set_water_world(IVec3::new(9, 66, 8), Block::Water, 0)); // flank
    assert!(w.set_water_world(IVec3::new(8, 66, 8), Block::Water, flowing(1)));
    run_ticks(&mut w, 30);

    assert!(
        is_source(w.water_meta_world(8, 66, 8)),
        "a flow resting on a source, flanked by two sources, converts"
    );
}

/// Conversion is judged before the falling state: even a FALLING cell flanked
/// by two sources over solid ground settles into a source (the base of a
/// waterfall pouring into an infinite pool heals into the pool).
#[test]
fn a_falling_cell_between_two_sources_converts_to_a_source() {
    let mut w = flat_world();
    w.set_block_world(7, 65, 8, Block::Water);
    w.set_block_world(9, 65, 8, Block::Water);
    assert!(w.set_water_world(IVec3::new(8, 65, 8), Block::Water, FALLING));
    run_ticks(&mut w, 30);

    assert!(
        is_source(w.water_meta_world(8, 65, 8)),
        "a falling cell flanked by two sources over solid ground converts"
    );
}

/// The anti-flood guard: a flow perched over a drop (air below) is the lip of
/// a waterfall, NOT grounded, so it must NOT convert even when flanked by
/// two sources. This is what keeps a flooding cave from turning to sources at
/// an exponential pace.
#[test]
fn a_flow_over_a_drop_never_converts_even_between_two_sources() {
    let mut w = flat_world();
    carve(&mut w, 8, 64, 8); // air below the middle cell — a one-block drop
    w.set_block_world(7, 65, 8, Block::Water);
    w.set_block_world(9, 65, 8, Block::Water);
    run_ticks(&mut w, 80);

    assert_eq!(
        block(&w, 8, 65, 8),
        Block::Water,
        "the gap still carries flowing water poured from the sources"
    );
    assert!(
        !is_source(w.water_meta_world(8, 65, 8)),
        "a flow resting on air (a waterfall lip) must never become a source"
    );
}
