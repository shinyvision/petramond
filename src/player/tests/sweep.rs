use super::*;

#[test]
fn falls_and_lands_on_floor() {
    // Solid everywhere y < 64 (a thick floor), air above.
    let solid = |_x: i32, y: i32, _z: i32| y < 64;
    let mut pl = p(Vec3::new(0.0, 70.0, 0.0));
    // Large downward sweep: must clamp feet to the top of cell 63 (y=64).
    let blocked = pl.sweep(Axis::Y, -20.0, &solid);
    assert!(blocked);
    assert_eq!(pl.pos.y, 64.0);
}

#[test]
fn does_not_tunnel_through_one_block_floor() {
    // Only y == 0 is solid (a 1-block-thick platform).
    let solid = |_x: i32, y: i32, _z: i32| y == 0;
    let mut pl = p(Vec3::new(0.0, 5.0, 0.0));
    let blocked = pl.sweep(Axis::Y, -20.0, &solid);
    assert!(blocked, "must not fall through a 1-thick floor");
    assert_eq!(pl.pos.y, 1.0, "feet rest on top of cell 0");
}

#[test]
fn stops_at_wall_moving_positive_x() {
    // Wall at x >= 5.
    let solid = |x: i32, _y: i32, _z: i32| x >= 5;
    let mut pl = p(Vec3::new(4.0, 64.0, 0.0)); // max.x = 4.3
    let blocked = pl.sweep(Axis::X, 2.0, &solid);
    assert!(blocked);
    // max.x clamped to 5.0 => centre at 4.7.
    assert!((pl.pos.x - 4.7).abs() < 1e-5, "pos.x = {}", pl.pos.x);
}

#[test]
fn stops_at_wall_moving_negative_x() {
    // Wall at x <= 1 (cells 1 and below solid).
    let solid = |x: i32, _y: i32, _z: i32| x <= 1;
    let mut pl = p(Vec3::new(4.0, 64.0, 0.0)); // min.x = 3.7
    let blocked = pl.sweep(Axis::X, -3.0, &solid);
    assert!(blocked);
    // min.x clamped to 2.0 (top of cell 1) => centre at 2.3.
    assert!((pl.pos.x - 2.3).abs() < 1e-5, "pos.x = {}", pl.pos.x);
}

#[test]
fn moves_freely_in_open_air() {
    let solid = |_x: i32, _y: i32, _z: i32| false;
    let mut pl = p(Vec3::new(0.0, 64.0, 0.0));
    assert!(!pl.sweep(Axis::Z, 3.0, &solid));
    assert_eq!(pl.pos.z, 3.0);
}

#[test]
fn grounded_player_auto_steps_up_a_half_block_but_not_a_full_one() {
    use crate::block::Aabb;
    let still = |_: Vec3| Vec3::ZERO;
    let walk_x = Input {
        wishdir: Vec3::new(1.0, 0.0, 0.0),
        jump: false,
        sprint: false,
        sneak: false,
    };

    // Floor at y=0 (full cubes) + a 0.5-tall ledge filling cells x>=1 at y=1 (world y∈[1,1.5]).
    let half_step = |x: i32, y: i32, _z: i32| -> &'static [Aabb] {
        if y == 0 {
            Block::Stone.collision_boxes()
        } else if y == 1 && x >= 1 {
            &[Aabb {
                min: [0.0, 0.0, 0.0],
                max: [1.0, 0.5, 1.0],
            }]
        } else {
            &[]
        }
    };
    let mut pl = p(Vec3::new(0.5, 1.0, 0.5)); // feet on the floor top, walking +X into the ledge
    for _ in 0..180 {
        pl.update_core_with_current(
            1.0 / 60.0,
            &half_step,
            &dry,
            &still,
            &no_ladder,
            &no_slip,
            walk_x,
            &[],
        );
    }
    assert!(
        pl.pos.x > 1.2,
        "grounded player steps onto the ledge: x={}",
        pl.pos.x
    );
    assert!(
        pl.pos.y > 1.4,
        "and rises onto the 0.5 ledge top (y≈1.5): y={}",
        pl.pos.y
    );
    assert!(pl.on_ground, "player is grounded on the ledge");

    // A FULL block (cells x>=1 at y=1 AND y=2) is NOT climbed — it's a wall.
    let full_block = |x: i32, y: i32, _z: i32| -> &'static [Aabb] {
        if y == 0 || ((y == 1 || y == 2) && x >= 1) {
            Block::Stone.collision_boxes()
        } else {
            &[]
        }
    };
    let mut pl2 = p(Vec3::new(0.5, 1.0, 0.5));
    for _ in 0..180 {
        pl2.update_core_with_current(
            1.0 / 60.0,
            &full_block,
            &dry,
            &still,
            &no_ladder,
            &no_slip,
            walk_x,
            &[],
        );
    }
    assert!(
        pl2.pos.y < 1.1,
        "player does NOT climb a full block: y={}",
        pl2.pos.y
    );
    assert!(
        pl2.pos.x < 1.0,
        "player is stopped by the full block: x={}",
        pl2.pos.x
    );
}

/// Trusted, slow reference: does the player AABB centred at `pos` overlap any
/// solid cell? Shrinks the box by a symmetric tol on every side (so it is
/// direction-agnostic by construction — any asymmetry in `sweep` shows up as
/// a disagreement with this).
fn ref_overlaps<F: Fn(i32, i32, i32) -> bool>(pos: Vec3, solid: &F) -> bool {
    let t = 1e-4;
    let x0 = (pos.x - HALF_W + t).floor() as i32;
    let x1 = (pos.x + HALF_W - t).floor() as i32;
    let y0 = (pos.y + t).floor() as i32;
    let y1 = (pos.y + HEIGHT - t).floor() as i32;
    let z0 = (pos.z - HALF_W + t).floor() as i32;
    let z1 = (pos.z + HALF_W - t).floor() as i32;
    for x in x0..=x1 {
        for y in y0..=y1 {
            for z in z0..=z1 {
                if solid(x, y, z) {
                    return true;
                }
            }
        }
    }
    false
}

/// Reference separated-axis move (X then Z, like `sweep`) advancing in
/// ~0.5 mm micro-steps and stopping before the first overlap. Moves *exactly*
/// `disp` in open space (the final sub-step takes up the remainder, so there
/// is no rounding drift). Obviously correct; the slow oracle for `sweep`.
fn ref_move<F: Fn(i32, i32, i32) -> bool>(mut pos: Vec3, disp: Vec3, solid: &F) -> Vec3 {
    let step = 5e-4f32;
    for axis in [0, 1] {
        let d = if axis == 0 { disp.x } else { disp.z };
        let mut moved = 0.0f32;
        while moved < d.abs() {
            let this = step.min(d.abs() - moved) * d.signum();
            let mut next = pos;
            if axis == 0 {
                next.x += this;
            } else {
                next.z += this;
            }
            if ref_overlaps(next, solid) {
                break;
            }
            pos = next;
            moved += this.abs();
        }
    }
    pos
}

#[test]
fn sweep_matches_reference_from_all_directions() {
    let configs: [(&str, &[IVec3]); 6] = [
        ("single", &[IVec3::new(10, 64, 10)]),
        (
            "wall_x",
            &[
                IVec3::new(10, 64, 8),
                IVec3::new(10, 64, 9),
                IVec3::new(10, 64, 10),
                IVec3::new(10, 64, 11),
                IVec3::new(10, 64, 12),
            ],
        ),
        (
            "wall_z",
            &[
                IVec3::new(8, 64, 10),
                IVec3::new(9, 64, 10),
                IVec3::new(10, 64, 10),
                IVec3::new(11, 64, 10),
                IVec3::new(12, 64, 10),
            ],
        ),
        ("pillar2", &[IVec3::new(10, 64, 10), IVec3::new(10, 65, 10)]),
        ("head", &[IVec3::new(10, 65, 10)]),
        (
            "Lcorner",
            &[
                IVec3::new(10, 64, 10),
                IVec3::new(11, 64, 10),
                IVec3::new(10, 64, 11),
            ],
        ),
    ];
    let dirs: [(f32, f32, &str); 8] = [
        (1.0, 0.0, "+X"),
        (-1.0, 0.0, "-X"),
        (0.0, 1.0, "+Z"),
        (0.0, -1.0, "-Z"),
        (1.0, 1.0, "+X+Z"),
        (1.0, -1.0, "+X-Z"),
        (-1.0, 1.0, "-X+Z"),
        (-1.0, -1.0, "-X-Z"),
    ];
    // Translate the whole scene to probe positive, origin-crossing, and
    // negative coordinates (floor()/cast/>>4 behave differently around 0).
    let bases: [(i32, i32, &str); 3] = [(0, 0, "pos"), (-10, -10, "origin"), (-21, -21, "neg")];
    let mut failures = Vec::new();
    for (bx, bz, bname) in bases {
        for (cname0, cells0) in configs {
            let cells_v: Vec<IVec3> = cells0
                .iter()
                .map(|c| IVec3::new(c.x + bx, c.y, c.z + bz))
                .collect();
            let cname = format!("{bname}/{cname0}");
            let solid = {
                let cells_v = cells_v.clone();
                move |x: i32, y: i32, z: i32| {
                    y < 64 || cells_v.iter().any(|c| c.x == x && c.y == y && c.z == z)
                }
            };
            let centre = Vec3::new(10.5 + bx as f32, 64.0, 10.5 + bz as f32);
            for (dx, dz, name) in dirs {
                let len = (dx * dx + dz * dz).sqrt();
                let wishdir = Vec3::new(dx / len, 0.0, dz / len);
                let lateral = Vec3::new(-wishdir.z, 0.0, wishdir.x);
                for k in -19..=19 {
                    let off = k as f32 * 0.05;
                    let start = centre - wishdir * 3.5 + lateral * off;
                    let (dt, speed) = (0.02f32, WALK);
                    // sweep path. Start at full walk speed so the friction
                    // ramp-up doesn't lag the reference mover (which moves at
                    // exactly speed·dt from step one); this test probes the
                    // collision sweep, not the acceleration curve.
                    let mut pl = p(start);
                    pl.on_ground = true;
                    pl.vel = wishdir * WALK;
                    let input = Input {
                        wishdir,
                        jump: false,
                        sprint: false,
                        sneak: false,
                    };
                    // reference path (kept at floor height, like the grounded body)
                    let mut rpos = start;
                    for _ in 0..150 {
                        pl.update_core(dt, &solid, &dry, input);
                        rpos = ref_move(rpos, wishdir * (speed * dt), &solid);
                    }
                    let d = ((pl.pos.x - rpos.x).powi(2) + (pl.pos.z - rpos.z).powi(2)).sqrt();
                    // Cardinals must track the reference tightly (the property the
                    // float-boundary bug broke: phantom/pass-through collisions).
                    // Diagonals slide along walls, where the two integrators round
                    // a corner up to one sub-step apart — allow that discretisation.
                    let tol = if dx == 0.0 || dz == 0.0 { 0.02 } else { 0.12 };
                    if d > tol {
                        failures.push(format!(
                            "{cname} {name} off={off:+.2}: sweep=({:.3},{:.3}) ref=({:.3},{:.3}) d={d:.3}",
                            pl.pos.x, pl.pos.z, rpos.x, rpos.z
                        ));
                    }
                }
            }
        }
    }
    assert!(
        failures.is_empty(),
        "{} mismatches:\n{}",
        failures.len(),
        failures
            .iter()
            .take(40)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
}

#[test]
fn sweep_does_not_skip_flush_wall_at_far_coordinates() {
    let bases = [2048, -2048, 8192, -8192];
    for base in bases {
        let wx = base + 10;
        let wz = base + 20;

        let wall_x = |x: i32, y: i32, z: i32| x == wx && (64..=65).contains(&y) && z == wz;
        let mut plus_x = p(Vec3::new(wx as f32 - HALF_W, 64.0, wz as f32 + 0.5));
        assert!(
            plus_x.sweep(Axis::X, 0.1, &wall_x),
            "+X should still hit a flush wall at base {base}"
        );
        assert!(
            (plus_x.pos.x - (wx as f32 - HALF_W)).abs() <= 0.001,
            "+X moved through flush wall at base {base}: x={}",
            plus_x.pos.x
        );

        let mut minus_x = p(Vec3::new((wx + 1) as f32 + HALF_W, 64.0, wz as f32 + 0.5));
        assert!(
            minus_x.sweep(Axis::X, -0.1, &wall_x),
            "-X should still hit a flush wall at base {base}"
        );
        assert!(
            (minus_x.pos.x - ((wx + 1) as f32 + HALF_W)).abs() <= 0.001,
            "-X moved through flush wall at base {base}: x={}",
            minus_x.pos.x
        );

        let wall_z = |x: i32, y: i32, z: i32| x == wx && (64..=65).contains(&y) && z == wz;
        let mut plus_z = p(Vec3::new(wx as f32 + 0.5, 64.0, wz as f32 - HALF_W));
        assert!(
            plus_z.sweep(Axis::Z, 0.1, &wall_z),
            "+Z should still hit a flush wall at base {base}"
        );
        assert!(
            (plus_z.pos.z - (wz as f32 - HALF_W)).abs() <= 0.001,
            "+Z moved through flush wall at base {base}: z={}",
            plus_z.pos.z
        );

        let mut minus_z = p(Vec3::new(wx as f32 + 0.5, 64.0, (wz + 1) as f32 + HALF_W));
        assert!(
            minus_z.sweep(Axis::Z, -0.1, &wall_z),
            "-Z should still hit a flush wall at base {base}"
        );
        assert!(
            (minus_z.pos.z - ((wz + 1) as f32 + HALF_W)).abs() <= 0.001,
            "-Z moved through flush wall at base {base}: z={}",
            minus_z.pos.z
        );
    }
}

#[test]
fn chest_collides_as_its_inset_box() {
    // A single chest at cell (0, 64, 0); every other cell is empty. Exercises the
    // general collision-box sweep with a non-full-cube shape.
    let boxes = |x: i32, y: i32, z: i32| {
        if (x, y, z) == (0, 64, 0) {
            Block::Chest
        } else {
            Block::Air
        }
        .collision_boxes()
    };
    let top = 64.0 + 14.0 / 16.0; // the chest's 14/16 collision top

    // Falling onto the chest lands the feet on its 14/16 top, not the full cell top.
    let mut faller = p(Vec3::new(0.5, 66.0, 0.5));
    assert!(
        faller.sweep_boxes(Axis::Y, -5.0, &boxes),
        "lands on the chest"
    );
    assert!(
        (faller.pos.y - top).abs() < 1e-3,
        "feet rest on the 14/16 top, got {}",
        faller.pos.y
    );

    // Standing on the chest top, you can walk off it (no full-cell wall blocking the
    // body just because its feet share the chest's cell).
    let mut on_top = p(Vec3::new(0.5, top, 0.5));
    assert!(
        !on_top.sweep_boxes(Axis::X, 1.0, &boxes),
        "walking off the top is not blocked"
    );
    assert!(
        (on_top.pos.x - 1.5).abs() < 1e-3,
        "walked the full step, got {}",
        on_top.pos.x
    );

    // At ground level beside the chest, walking into it stops at the 1/16 inset face,
    // not the cell boundary.
    let mut walker = p(Vec3::new(-1.0, 64.0, 0.5));
    assert!(
        walker.sweep_boxes(Axis::X, 2.0, &boxes),
        "hits the chest side"
    );
    assert!(
        (walker.aabb_max().x - 1.0 / 16.0).abs() < 1e-3,
        "stops at the inset -X face (1/16), got {}",
        walker.aabb_max().x
    );
}
