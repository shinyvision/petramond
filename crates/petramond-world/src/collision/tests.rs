use super::*;

/// A full unit cube (a normal solid block's shape).
const FULL: &[Aabb] = &[Aabb {
    min: [0.0; 3],
    max: [1.0; 3],
}];
/// A chest-style inset box: 1/16 margin on the sides, 14/16 tall.
const INSET: &[Aabb] = &[Aabb {
    min: [0.0625, 0.0, 0.0625],
    max: [0.9375, 0.875, 0.9375],
}];

/// One solid cell at `(0,0,0)` of the given shape; everything else empty.
fn one_cell(shape: &'static [Aabb]) -> impl Fn(i32, i32, i32) -> &'static [Aabb] {
    move |x, y, z| if (x, y, z) == (0, 0, 0) { shape } else { &[] }
}

#[test]
fn sweep_stops_at_a_full_cube_face() {
    // A unit body at x∈[2,3] moving -X toward the cube at cell 0 (its +X face is x=1)
    // travels until its min meets x=1: from 2 to 1 → -1.0, not the requested -5.
    let travel = sweep_axis([2.0, 0.2, 0.2], [3.0, 0.8, 0.8], 0, -5.0, one_cell(FULL));
    assert!(
        (travel - (-1.0)).abs() < 1e-3,
        "stops at the cube face, got {travel}"
    );
    // With no cross-axis overlap (body above the cube) it passes freely.
    let free = sweep_axis([2.0, 5.0, 0.2], [3.0, 6.0, 0.8], 0, -5.0, one_cell(FULL));
    assert!(
        (free - (-5.0)).abs() < 1e-6,
        "no overlap → full travel, got {free}"
    );
}

#[test]
fn sweep_respects_an_inset_box_margin() {
    // Falling onto the inset block: a body resting at y just above stops at the box top
    // (y = 0.875), not the cell top (y = 1.0).
    let travel = sweep_axis([0.3, 1.5, 0.3], [0.7, 2.5, 0.7], 1, -2.0, one_cell(INSET));
    assert!(
        (travel - (0.875 - 1.5)).abs() < 1e-3,
        "rests on the inset top, got {travel}"
    );
    // A thin body wholly inside the 1/16 side MARGIN (x ∈ [0, 0.05], left of the inset's
    // -X face at 0.0625) falls straight past it — no cross-axis overlap, full travel —
    // where a full cube would have stopped it.
    let margin = sweep_axis([0.0, 1.5, 0.3], [0.05, 2.5, 0.7], 1, -2.0, one_cell(INSET));
    assert!(
        (margin - (-2.0)).abs() < 1e-6,
        "falls through the side margin, got {margin}"
    );
}

#[test]
fn resolve_body_lands_grounded_on_a_floor() {
    // A 0.4-wide, 0.6-tall body falling onto the floor cell (0,0,0): it lands on y=1
    // (the cube top) and reports grounded.
    let floor = |_x: i32, y: i32, _z: i32| if y == 0 { FULL } else { &[][..] };
    let (moved, grounded, hit) = resolve_body(
        [0.3, 1.4, 0.3],
        [0.7, 2.0, 0.7],
        [0.0, -5.0, 0.0],
        0.1,
        0.0,
        &mut Default::default(),
        floor,
    );
    assert!(grounded, "a downward stop is grounded");
    assert!(hit[1] && !hit[0] && !hit[2], "only Y blocked");
    // Wanted -0.5; clamped so the body bottom (1.4) meets the floor top (1.0) → -0.4.
    assert!(
        (moved[1] - (-0.4)).abs() < 1e-3,
        "clamped to the floor, got {}",
        moved[1]
    );
}

/// A 15/16 block (farmland) whose cell becomes a FULL cube under standing
/// feet: without the depenetration heal the downward sweep skips the box
/// it starts inside and the body tunnels through the world; with it the
/// body lifts the missing texel and lands grounded on the new top.
#[test]
fn a_block_growing_underfoot_lifts_the_body_instead_of_tunnelling() {
    let floor = |_x: i32, y: i32, _z: i32| if y == 0 { FULL } else { &[][..] };
    // Feet at the old farmland top (15/16), now 1/16 inside the dirt cube.
    // The escape must finish INSIDE this tick — the downward sweep runs
    // straight after it and would otherwise pass through the box the body
    // started in (sweeps ignore boxes a body already overlaps).
    let (min, max) = ([0.2, 0.9375, 0.2], [0.8, 2.7375, 0.8]);
    let (moved, grounded, _) = resolve_body(
        min,
        max,
        [0.0, -5.0, 0.0],
        0.1,
        0.0,
        &mut Default::default(),
        floor,
    );
    assert!(grounded, "the healed body lands on the grown block");
    assert!(
        (moved[1] - 0.0625).abs() < 1e-3,
        "net movement is the upward heal, not a fall, got {}",
        moved[1]
    );

    // A body flush ON a box top is not inside it: nothing to escape.
    let mut route = EscapeRoute::default();
    let rest = route.advance(&[([0.2, 1.0, 0.2], [0.8, 2.8, 0.8])], 0.05, &floor, &[], 0);
    assert_eq!(rest, [0.0; 3], "standing on top never moves");
}

#[test]
fn step_horizontal_climbs_a_half_block_but_not_a_full_one() {
    // A 0.5-tall ledge at cell x=1 (a box world-y ∈ [1, 1.5]) plus the floor (y=0).
    let half_step = |_x: i32, y: i32, _z: i32| -> &'static [Aabb] {
        if y == 0 {
            FULL
        } else if y == 1 {
            &[Aabb {
                min: [0.0, 0.0, 0.0],
                max: [1.0, 0.5, 1.0],
            }]
        } else {
            &[]
        }
    };
    // Body standing on the floor at x∈[0.2,0.8], feet y=1, walking +X by 0.5 into x=1.
    let (moved, hit_x, _) = step_horizontal(
        [0.2, 1.0, 0.2],
        [0.8, 2.0, 0.8],
        0.5,
        0.0,
        STEP_HEIGHT,
        half_step,
    );
    assert!(!hit_x, "a 0.5 step is climbed, not blocked");
    assert!(moved[0] > 0.4, "it advanced over the step, dx={}", moved[0]);
    assert!(
        (moved[1] - 0.5).abs() < 0.05,
        "it rose onto the step top, dy={}",
        moved[1]
    );

    // A FULL block at cell x=1 (y∈[1,2]) is NOT climbed.
    let full_step = |_x: i32, y: i32, _z: i32| -> &'static [Aabb] {
        if y == 0 || y == 1 {
            FULL
        } else {
            &[]
        }
    };
    let (moved2, hit_x2, _) = step_horizontal(
        [0.2, 1.0, 0.2],
        [0.8, 2.0, 0.8],
        0.5,
        0.0,
        STEP_HEIGHT,
        full_step,
    );
    assert!(hit_x2, "a full block blocks");
    assert!(
        moved2[1] < 1e-3,
        "no rise over a full block, dy={}",
        moved2[1]
    );
    assert!(
        moved2[0] < 0.3,
        "only slid up to the wall face, dx={}",
        moved2[0]
    );

    // With step_height = 0 (the airborne / no-step case), even the 0.5 ledge blocks.
    let (moved3, hit_x3, _) =
        step_horizontal([0.2, 1.0, 0.2], [0.8, 2.0, 0.8], 0.5, 0.0, 0.0, half_step);
    assert!(hit_x3, "no step-up when not grounded");
    assert!(moved3[1] < 1e-3, "no rise when step disabled");
}

#[test]
fn clamp_to_supported_holds_the_edge_but_allows_a_step_down() {
    /// A half-height slab (top at 0.5).
    const HALF: &[Aabb] = &[Aabb {
        min: [0.0, 0.0, 0.0],
        max: [1.0, 0.5, 1.0],
    }];
    // A body standing on the single floor cell (0,0,0), moving +X into the void:
    // pulled back so the feet keep support (body min.x stays over the cell).
    let (mn, mx) = ([0.2, 1.0, 0.2], [0.8, 2.8, 0.8]);
    let (cx, cz) = clamp_to_supported(mn, mx, 1.0, 0.0, STEP_HEIGHT, one_cell(FULL));
    assert_eq!(cz, 0.0);
    assert!(
        cx > 0.0 && cx < 0.8,
        "slides to the lip, never past it: {cx}"
    );

    // A half-slab in the next cell: a step-down within STEP_HEIGHT — free travel.
    let slab_next = |x: i32, y: i32, z: i32| -> &'static [Aabb] {
        match (x, y, z) {
            (0, 0, 0) => FULL,
            (1, 0, 0) => HALF,
            _ => &[],
        }
    };
    let (cx, _) = clamp_to_supported(mn, mx, 0.4, 0.0, STEP_HEIGHT, slab_next);
    assert_eq!(cx, 0.4, "a step-down within the allowance is not clamped");

    // Diagonal along a floor strip (all z at x=0): the off-edge X component is
    // clamped, the along-edge Z component survives.
    let strip = |x: i32, y: i32, _z: i32| -> &'static [Aabb] {
        if x == 0 && y == 0 {
            FULL
        } else {
            &[]
        }
    };
    let (cx, cz) = clamp_to_supported(mn, mx, 1.0, 1.0, STEP_HEIGHT, strip);
    assert!(cx < 1.0, "the off-edge axis is pulled back: {cx}");
    assert_eq!(cz, 1.0, "the along-edge axis keeps its full travel");

    // An already-unsupported body (mid-air) is left alone.
    let (cx, cz) = clamp_to_supported(
        [5.0, 8.0, 5.0],
        [5.6, 9.8, 5.6],
        1.0,
        -0.5,
        STEP_HEIGHT,
        one_cell(FULL),
    );
    assert_eq!((cx, cz), (1.0, -0.5));
}

#[test]
fn dynamic_boxes_block_land_and_skip_their_owner() {
    let empty = |_: i32, _: i32, _: i32| -> &'static [Aabb] { &[] };
    let hull = DynBox {
        id: 7,
        min: [2.0, 0.0, -1.0],
        max: [4.0, 0.75, 1.0],
    };
    // Walking +X into the hull stops at its face (x = 2.0).
    let t = sweep_axis_dyn([0.5, 0.1, -0.3], [1.1, 1.9, 0.3], 0, 3.0, empty, &[hull], 0);
    assert!((t - 0.9).abs() < 1e-3, "stops at the hull face: {t}");
    // The owning entity skips its own box.
    let own = sweep_axis_dyn([0.5, 0.1, -0.3], [1.1, 1.9, 0.3], 0, 3.0, empty, &[hull], 7);
    assert!((own - 3.0).abs() < 1e-6, "the owner passes freely: {own}");
    // No cross overlap (body beside the hull on Z) passes freely.
    let miss = sweep_axis_dyn([0.5, 0.1, 2.0], [1.1, 1.9, 2.6], 0, 3.0, empty, &[hull], 0);
    assert!(
        (miss - 3.0).abs() < 1e-6,
        "no overlap → full travel: {miss}"
    );
    // Falling onto the deck lands grounded on its top (y = 0.75).
    let (moved, grounded, hit) = resolve_body_dyn(
        [2.5, 2.0, -0.3],
        [3.1, 3.8, 0.3],
        [0.0, -5.0, 0.0],
        1.0,
        0.0,
        &mut Default::default(),
        empty,
        &[hull],
        0,
    );
    assert!(
        grounded && hit[1],
        "a downward stop on the deck is grounded"
    );
    assert!(
        (moved[1] - (0.75 - 2.0)).abs() < 1e-3,
        "lands on the deck: {}",
        moved[1]
    );
    // The deck counts as sneak support.
    let (cx, _) = clamp_to_supported_dyn(
        [2.5, 0.75, -0.3],
        [3.1, 2.55, 0.3],
        5.0,
        0.0,
        STEP_HEIGHT,
        empty,
        &[hull],
        0,
    );
    assert!(cx < 5.0, "the edge guard holds at the deck lip: {cx}");
}

#[test]
fn padded_segment_clamps_at_a_wall_and_passes_free_air() {
    // A boom retreating along -Z from (0.5, 0.5, 0.5) toward the full cube at
    // cell z = -3 (its near face is world z = -2): with pad 0.2 the clamp is
    // where the padded point meets z = -2 + 0.2 → travel 0.5 - (-1.8) = 2.3.
    let wall = |_x: i32, _y: i32, z: i32| if z == -3 { FULL } else { &[][..] };
    let d = clamp_padded_segment([0.5, 0.5, 0.5], [0.0, 0.0, -1.0], 4.0, 0.2, wall);
    assert!((d - 2.3).abs() < 1e-3, "clamped just before the wall: {d}");

    // Free air: the full boom length comes back.
    let free = clamp_padded_segment(
        [0.5, 0.5, 0.5],
        [0.0, 0.0, -1.0],
        4.0,
        0.2,
        |_, _, _| &[][..],
    );
    assert!((free - 4.0).abs() < 1e-6, "unblocked boom is full length");
}

#[test]
fn padded_segment_respects_partial_shapes_and_a_solid_start() {
    // The inset box occupies y ∈ [0, 0.875]: a boom passing OVER it (y = 1.2,
    // pad 0.1 → clearance above 0.975) is unblocked, exactly like the swept
    // body respecting a model's real shape.
    let over = clamp_padded_segment([0.5, 1.2, 3.0], [0.0, 0.0, -1.0], 4.0, 0.1, one_cell(INSET));
    assert!(
        (over - 4.0).abs() < 1e-6,
        "passes over the inset top: {over}"
    );
    // The same boom at y = 0.5 runs straight into it.
    let into = clamp_padded_segment([0.5, 0.5, 3.0], [0.0, 0.0, -1.0], 4.0, 0.1, one_cell(INSET));
    assert!(into < 4.0 - 1e-3, "blocked through the box: {into}");

    // Starting already inside an expanded box clamps to zero, never negative.
    let inside = clamp_padded_segment([0.5, 0.5, 0.5], [0.0, 0.0, -1.0], 4.0, 0.2, one_cell(FULL));
    assert_eq!(inside, 0.0, "a start inside solid stays at the eye");
}

#[test]
fn point_in_solid_respects_the_inset_margin() {
    // A point inside the inset box is solid; one in the 1/16 side margin is free.
    assert!(point_in_solid([0.5, 0.5, 0.5], one_cell(INSET)));
    assert!(
        !point_in_solid([0.02, 0.5, 0.5], one_cell(INSET)),
        "side margin is free"
    );
    // A point in an empty cell is never solid.
    assert!(!point_in_solid([0.5, 5.5, 0.5], one_cell(INSET)));
}

#[test]
fn a_far_body_resolves_exactly_like_one_at_the_origin() {
    // At 2^29 an f32 coordinate cannot even hold the 0.3 offsets below.
    let run = |base: i32| {
        let o = f64::from(base);
        let floor = move |x: i32, y: i32, z: i32| {
            if (x, y, z) == (base, 0, base) {
                FULL
            } else {
                &[][..]
            }
        };
        resolve_body(
            [o + 0.3, 1.4, o + 0.3],
            [o + 0.7, 2.0, o + 0.7],
            [0.05, -5.0, 0.0],
            0.1,
            0.0,
            &mut Default::default(),
            floor,
        )
    };
    assert_eq!(run(0), run(1 << 29));
}

#[test]
fn an_escape_leaves_by_an_open_face_not_the_nearest_one() {
    // The body is inside the cube at x = 1. Its SHALLOWEST way out is −x
    // (0.7 m), but the cube at x = 0 is sitting there: a shortest-penetration
    // guess walks straight into it and the body never gets out. The open side
    // is +x, 0.9 m away.
    let boxes = |x: i32, y: i32, _z: i32| {
        if y == 0 && (x == 0 || x == 1) {
            FULL
        } else {
            &[][..]
        }
    };
    let body = [([1.1, 0.0, 0.2], [1.7, 1.8, 0.8])];
    let Escape::Route(off) = escape_pose(&body, &boxes, &[], 0) else {
        panic!("a body with an open face beside it is not sealed");
    };
    assert!(
        off[0] > 0.0,
        "escaped toward the open side, not into the neighbouring cube: {off:?}"
    );
    assert!(
        pose_is_free(&body, off.map(f64::from), &boxes, &[], 0),
        "the destination is genuinely empty"
    );
}

#[test]
fn an_escape_never_crosses_geometry_the_body_is_not_already_in() {
    // Solid everywhere but a pocket two cells away: the only free pose in
    // reach sits BEHIND a wall. A clean route may not pop through it — but
    // the body is buried, so it BORES toward that pocket rather than being
    // left standing in rock, and says it is entombed while it does.
    let boxes = |x: i32, y: i32, z: i32| {
        if x == 2 && (0..2).contains(&y) && z == 0 {
            &[][..]
        } else {
            FULL
        }
    };
    let body = [([0.2, 0.0, 0.2], [0.8, 1.8, 0.8])];
    let Escape::Bore(off) = escape_pose(&body, &boxes, &[], 0) else {
        panic!("a buried body bores; it is never left standing in rock");
    };
    assert!(off[0] > 0.0, "bored toward the only open air: {off:?}");
    assert!(
        pose_is_free(&body, off.map(f64::from), &boxes, &[], 0),
        "and it is aimed at a genuinely free pose"
    );

    // Open that wall and the same pocket becomes reachable.
    let opened = |x: i32, y: i32, z: i32| {
        if x >= 1 && (0..2).contains(&y) && z == 0 {
            &[][..]
        } else {
            FULL
        }
    };
    let Escape::Route(off) = escape_pose(&body, &opened, &[], 0) else {
        panic!("a clear corridor is an escape");
    };
    assert!(off[0] > 0.0, "left along the corridor: {off:?}");
}

#[test]
fn a_committed_route_walks_out_without_re_deciding() {
    // A 3-high column grew around the body (the classic bob): it slides out
    // along ONE committed route. Re-deciding every tick is what oscillates,
    // so the offsets must never reverse, and the walk must finish.
    let boxes = |x: i32, y: i32, z: i32| {
        if y < 0 || (x == 0 && z == 0 && (0..3).contains(&y)) {
            FULL
        } else {
            &[][..]
        }
    };
    let body = [([0.1, 0.0, 0.2], [0.7, 1.8, 0.8])];
    let mut route = EscapeRoute::default();
    let mut total = [0.0f32; 3];
    let mut ticks = 0;
    loop {
        let shifted = [(
            std::array::from_fn(|axis| body[0].0[axis] + f64::from(total[axis])),
            std::array::from_fn(|axis| body[0].1[axis] + f64::from(total[axis])),
        )];
        let off = route.advance(&shifted, ESCAPE_SPEED * 0.05, &boxes, &[], 0);
        if off == [0.0; 3] {
            break;
        }
        assert!(off[0] <= 0.0, "the route never reverses: {off:?}");
        assert!(off[1] <= 0.0, "the feet never lift on the way out: {off:?}");
        for axis in 0..3 {
            total[axis] += off[axis];
        }
        ticks += 1;
        assert!(ticks < 100, "the escape finished");
    }
    assert!(!route.entombed(), "it got out");
    assert!(total[0] < -0.5, "it left the column sideways: {total:?}");
}

#[test]
fn a_body_buried_in_solid_rock_climbs_out_instead_of_sitting_there() {
    // THE underground case: stone in every direction, nothing free within
    // reach. There is no clean way out and there never will be one until the
    // body moves, so "hold still" loses the body for good. It heads up, and
    // stops the moment a pose is genuinely free.
    let bedrock_to_y = 8;
    let boxes = move |_x: i32, y: i32, _z: i32| {
        if y < bedrock_to_y {
            FULL
        } else {
            &[][..]
        }
    };
    let body = ([0.2, 2.0, 0.2], [0.8, 3.8, 0.8]);
    let mut route = EscapeRoute::default();
    let mut lift = 0.0f32;
    let mut ticks = 0;
    let (mut was_entombed, mut escaped_cleanly) = (false, false);
    loop {
        let shifted = [(
            [body.0[0], body.0[1] + f64::from(lift), body.0[2]],
            [body.1[0], body.1[1] + f64::from(lift), body.1[2]],
        )];
        let off = route.advance(&shifted, 0.05, &boxes, &[], 0);
        if off == [0.0; 3] {
            break;
        }
        assert!(off[1] > 0.0, "it climbs, tick after tick: {off:?}");
        // Buried while there is no clean way out; once open air comes within
        // reach the ordinary verified route takes over and finishes the job,
        // and the body stops reporting itself entombed.
        was_entombed |= route.entombed();
        assert!(
            !route.entombed() || !escaped_cleanly,
            "entombed never comes BACK once a clean route was found"
        );
        escaped_cleanly |= !route.entombed();
        lift += off[1];
        ticks += 1;
        assert!(ticks < 200, "it got out in bounded time");
    }
    assert!(
        (f64::from(lift) + body.0[1] - f64::from(bedrock_to_y)).abs() < 0.2,
        "it stopped at the surface, not one block short or a mile up: {lift}"
    );
    assert!(was_entombed, "it knew it was buried on the way");
    assert!(!route.entombed(), "free bodies are not entombed");
}
