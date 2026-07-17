use super::*;

fn long_body() -> MobSize {
    MobSize {
        half_width: 0.5,
        height: 1.0,
        half_length: Some(1.5),
    }
}

fn bodies_overlap(
    a_pos: Vec3,
    a_yaw: f32,
    a_size: MobSize,
    b_pos: Vec3,
    b_yaw: f32,
    b_size: MobSize,
) -> bool {
    let mut b_boxes = Vec::new();
    super::super::solid_boxes(2, b_pos, b_yaw, b_size, &mut b_boxes);
    body_boxes(a_pos, a_yaw, a_size).any(|(min, max)| {
        crate::collision::aabb_hits_dynamic(min.to_array(), max.to_array(), &b_boxes, 1)
    })
}

#[test]
fn a_long_body_does_not_fill_its_enclosing_square_corners() {
    let size = long_body();
    let pos = Vec3::ZERO;
    let eye = Vec3::new(1.0, 0.5, 3.0);
    let dir = -Vec3::Z;
    let enclosing_extent = size.half_length.unwrap();
    let enclosing_min = Vec3::new(pos.x - enclosing_extent, pos.y, pos.z - enclosing_extent);
    let enclosing_max = Vec3::new(
        pos.x + enclosing_extent,
        pos.y + size.height,
        pos.z + enclosing_extent,
    );

    assert!(
        crate::player::ray_vs_aabb(eye, dir, enclosing_min, enclosing_max).is_some(),
        "the former enclosing square would select this empty corner"
    );
    assert_eq!(
        closest_body_ray_hit(eye, dir, crate::player::REACH, [(7, pos, 0.0, size)]),
        None,
        "segmented body geometry leaves the corner empty"
    );
}

#[test]
fn a_long_body_blocks_placement_at_its_bow_and_stern() {
    let size = long_body();
    let pos = Vec3::ZERO;
    let bow_cell = crate::mathh::IVec3::new(0, 0, -2);
    let full = crate::block::Block::Stone.collision_boxes();

    assert!(
        !crate::body::Body::new(pos, size.half_width, size.height)
            .overlaps_block_boxes(bow_cell, full),
        "the legacy centre square does not reach the bow cell"
    );
    assert!(
        body_overlaps_block_boxes(pos, 0.0, size, bow_cell, full),
        "the shared segmented body prevents bow-clipping placement"
    );
    assert!(
        body_overlaps_block_boxes(pos, 0.0, size, crate::mathh::IVec3::new(0, 0, 1), full,),
        "the stern participates too"
    );
}

#[test]
fn a_bow_only_soft_contact_produces_one_compound_shove() {
    let hull = long_body();
    let soft = MobSize {
        half_width: 0.25,
        height: 0.8,
        half_length: None,
    };
    let hull_pos = Vec3::ZERO;
    let soft_pos = Vec3::new(0.0, 0.0, -1.6);

    assert!(
        crate::body::separation(
            crate::body::Body::new(hull_pos, hull.half_width, hull.height),
            crate::body::Body::new(soft_pos, soft.half_width, soft.height),
        )
        .is_none(),
        "the former centre-only snapshot misses this bow contact"
    );
    let shove = body_separation(hull_pos, 0.0, hull, soft_pos, 0.0, soft)
        .expect("the compound bow touches the soft body");
    assert!(
        shove.z > 0.0,
        "the hull is separated away from the bow contact"
    );

    let deepest_single = segment_centres(hull_pos, 0.0, hull)
        .filter_map(|centre| {
            crate::body::separation(
                crate::body::Body::new(centre, hull.half_width, hull.height),
                crate::body::Body::new(soft_pos, soft.half_width, soft.height),
            )
        })
        .max_by(|a, b| a.length_squared().total_cmp(&b.length_squared()))
        .unwrap();
    assert_eq!(
        shove, deepest_single,
        "segment contacts select one pair separation instead of accumulating"
    );
}

#[test]
fn diagonal_square_contact_survives_the_push_broadphase() {
    let size = MobSize {
        half_width: 0.5,
        height: 1.0,
        half_length: None,
    };
    assert!(
        body_separation(Vec3::ZERO, 0.0, size, Vec3::new(0.9, 0.0, 0.9), 0.0, size,).is_some(),
        "overlapping square corners cannot be culled by a circular broadphase"
    );
}

#[test]
fn two_driven_long_solids_share_toi_and_never_pass_through_on_later_ticks() {
    let size = long_body();
    let yaw = -std::f32::consts::FRAC_PI_2;
    let mut a_pos = Vec3::new(-3.0, 0.0, 0.0);
    let mut b_pos = Vec3::new(3.0, 0.0, 0.0);
    let mut solver = SolidMotionSolver::default();

    let first = [
        BodyMotion {
            id: 10,
            start_pos: a_pos,
            start_yaw: yaw,
            end_pos: a_pos + Vec3::X * 2.0,
            end_yaw: yaw,
            size,
        },
        BodyMotion {
            id: 20,
            start_pos: b_pos,
            start_yaw: yaw,
            end_pos: b_pos - Vec3::X * 2.0,
            end_yaw: yaw,
            size,
        },
    ];
    let forward = solver.resolve(&first).to_vec();
    let reverse = solver.resolve(&[first[1], first[0]]).to_vec();
    assert!((forward[0] - forward[1]).abs() < 1e-6);
    assert!((forward[0] - reverse[1]).abs() < 1e-6);
    assert!((forward[1] - reverse[0]).abs() < 1e-6);

    for _ in 0..8 {
        let motions = [
            BodyMotion {
                id: 10,
                start_pos: a_pos,
                start_yaw: yaw,
                end_pos: a_pos + Vec3::X * 2.0,
                end_yaw: yaw,
                size,
            },
            BodyMotion {
                id: 20,
                start_pos: b_pos,
                start_yaw: yaw,
                end_pos: b_pos - Vec3::X * 2.0,
                end_yaw: yaw,
                size,
            },
        ];
        let fractions = solver.resolve(&motions);
        a_pos = motions[0].pose_at(fractions[0]).0;
        b_pos = motions[1].pose_at(fractions[1]).0;

        assert!(a_pos.x < b_pos.x, "the stable identities never cross");
        assert!(
            !bodies_overlap(a_pos, yaw, size, b_pos, yaw, size),
            "committed compound bodies remain non-overlapping: {a_pos:?} {b_pos:?}"
        );
    }
}

#[test]
fn two_turning_long_solids_stop_before_their_bows_overlap() {
    let size = long_body();
    let motions = [
        BodyMotion {
            id: 10,
            start_pos: Vec3::new(-1.4, 0.0, 0.0),
            start_yaw: 0.0,
            end_pos: Vec3::new(-1.4, 0.0, 0.0),
            end_yaw: -std::f32::consts::FRAC_PI_2,
            size,
        },
        BodyMotion {
            id: 20,
            start_pos: Vec3::new(1.4, 0.0, 0.0),
            start_yaw: 0.0,
            end_pos: Vec3::new(1.4, 0.0, 0.0),
            end_yaw: std::f32::consts::FRAC_PI_2,
            size,
        },
    ];
    let mut solver = SolidMotionSolver::default();
    let forward = solver.resolve(&motions).to_vec();
    let reverse = solver.resolve(&[motions[1], motions[0]]).to_vec();

    assert!(forward.iter().all(|fraction| *fraction < 1.0));
    assert!((forward[0] - forward[1]).abs() < 1e-5);
    assert!((forward[0] - reverse[1]).abs() < 1e-5);
    assert!((forward[1] - reverse[0]).abs() < 1e-5);
    let (a_pos, a_yaw) = motions[0].pose_at(forward[0]);
    let (b_pos, b_yaw) = motions[1].pose_at(forward[1]);
    assert!(
        !bodies_overlap(a_pos, a_yaw, size, b_pos, b_yaw, size),
        "simultaneous yaw commits a non-overlapping prefix"
    );
}

#[test]
fn a_peer_truncated_turn_and_translation_stays_clear_of_shore() {
    let size = long_body();
    let square = MobSize {
        half_width: 0.5,
        height: 1.0,
        half_length: None,
    };
    let shore = |x: i32, y: i32, z: i32| {
        if (x, y, z) == (2, 0, -2) {
            crate::block::Block::Stone.collision_boxes()
        } else {
            crate::block::Block::Air.collision_boxes()
        }
    };
    let start = Vec3::ZERO;
    let requested_yaw = -std::f32::consts::FRAC_PI_2;
    let end_yaw = clamp_body_yaw(start, 0.0, requested_yaw, size, &shore, &[], 10);
    assert!(
        (end_yaw - requested_yaw).abs() < 1e-6,
        "the yaw stage clears shore"
    );
    let (moved, _, _, _) = resolve_body_motion(
        start,
        end_yaw,
        size,
        [2.0, 0.0, 0.0],
        1.0,
        0.0,
        &shore,
        &[],
        &[],
        10,
    );
    assert!((moved[0] - 2.0).abs() < 1e-6, "the X sweep clears shore");
    let motion = BodyMotion {
        id: 10,
        start_pos: start,
        start_yaw: 0.0,
        end_pos: start + Vec3::from(moved),
        end_yaw,
        size,
    };
    let peer = BodyMotion {
        id: 20,
        start_pos: Vec3::new(2.6, 0.0, -0.4),
        start_yaw: 0.0,
        end_pos: Vec3::new(2.6, 0.0, -0.4),
        end_yaw: 0.0,
        size: square,
    };
    let mut solver = SolidMotionSolver::default();
    let fraction = solver.resolve(&[motion, peer])[0];
    assert!(
        (0.5..1.0).contains(&fraction),
        "the peer stops the translation after yaw completes: {fraction}"
    );
    let safe = terrain_safe_motion_prefix(motion, fraction, &shore);
    assert!(
        (safe - fraction).abs() < 1e-5,
        "the staged peer prefix follows the terrain-validated yaw then sweep"
    );
    let (pos, yaw) = motion.pose_at(safe);
    assert!(body_pose_fits(pos, yaw, size, &shore, &|_, _, _| true, &[],));

    let old_coupled_pos = start.lerp(motion.end_pos, fraction);
    let old_coupled_yaw = wrap_angle(motion.start_yaw + motion.yaw_delta() * fraction);
    assert!(
        !body_pose_fits(
            old_coupled_pos,
            old_coupled_yaw,
            size,
            &shore,
            &|_, _, _| true,
            &[],
        ),
        "the former coupled prefix clipped this shore corner"
    );
}

#[test]
fn a_peer_truncated_axis_slide_is_clamped_before_its_terrain_corner() {
    let size = MobSize {
        half_width: 0.2,
        height: 1.0,
        half_length: None,
    };
    let corner = |x: i32, y: i32, z: i32| {
        if (x, y, z) == (1, 0, 1) {
            crate::block::Block::Stone.collision_boxes()
        } else {
            crate::block::Block::Air.collision_boxes()
        }
    };
    let start = Vec3::new(0.5, 0.0, 0.5);
    let (moved, _, _, _) = resolve_body_motion(
        start,
        0.0,
        size,
        [2.0, 0.0, 2.0],
        1.0,
        0.0,
        &corner,
        &[],
        &[],
        10,
    );
    assert_eq!(
        moved,
        [2.0, 0.0, 2.0],
        "the terrain resolver's X-then-Z route goes around the block"
    );
    let motion = BodyMotion {
        id: 10,
        start_pos: start,
        start_yaw: 0.0,
        end_pos: start + Vec3::from(moved),
        end_yaw: 0.0,
        size,
    };
    let peer = BodyMotion {
        id: 20,
        start_pos: motion.end_pos,
        start_yaw: 0.0,
        end_pos: motion.end_pos,
        end_yaw: 0.0,
        size,
    };
    let mut solver = SolidMotionSolver::default();
    let peer_fraction = solver.resolve(&[motion, peer])[0];
    assert!(peer_fraction < 1.0);
    let safe = terrain_safe_motion_prefix(motion, peer_fraction, &corner);
    assert!(
        safe + 1e-4 < peer_fraction,
        "the straight peer path is stopped before cutting the X/Z corner"
    );

    let fractions = solver.resolve_with_limits(&[motion, peer], &[safe, 1.0]);
    let (pos, yaw) = motion.pose_at(fractions[0]);
    let (peer_pos, peer_yaw) = peer.pose_at(fractions[1]);
    assert!(body_pose_fits(
        pos,
        yaw,
        size,
        &corner,
        &|_, _, _| true,
        &[],
    ));
    assert!(
        !bodies_overlap(pos, yaw, size, peer_pos, peer_yaw, size),
        "pair solving is rerun after the terrain limit"
    );
}

#[test]
fn checked_body_fit_rejects_shore_and_solid_entity_overlap() {
    let size = long_body();
    let pos = Vec3::ZERO;
    let known = |_: i32, _: i32, _: i32| true;
    let shore = |x: i32, y: i32, z: i32| {
        if (x, y, z) == (0, 0, -2) {
            crate::block::Block::Stone.collision_boxes()
        } else {
            crate::block::Block::Air.collision_boxes()
        }
    };
    assert!(
        !body_pose_fits(pos, 0.0, size, &shore, &known, &[]),
        "terrain touching only the bow rejects the complete pose"
    );

    let air = |_: i32, _: i32, _: i32| crate::block::Block::Air.collision_boxes();
    let obstacle = crate::collision::DynBox {
        id: 7,
        min: [-0.5, 0.0, -1.5],
        max: [0.5, 1.0, -0.5],
    };
    assert!(
        !body_pose_fits(pos, 0.0, size, &air, &known, &[obstacle]),
        "another solid body rejects the spawn atomically"
    );
    assert!(body_pose_fits(pos, 0.0, size, &air, &known, &[]));

    let face_touch_is_not_covered = |x: i32, _: i32, _: i32| x != 1;
    assert!(body_pose_fits(
        Vec3::new(0.5, 0.0, 0.0),
        0.0,
        size,
        &air,
        &face_touch_is_not_covered,
        &[],
    ));
}

#[test]
fn equal_distance_ray_hits_choose_the_lower_stable_id_in_any_input_order() {
    let size = long_body();
    let pos = Vec3::ZERO;
    let eye = Vec3::new(0.0, 0.5, 3.0);
    let dir = -Vec3::Z;

    let forward = closest_body_ray_hit(
        eye,
        dir,
        crate::player::REACH,
        [(2_u64, pos, 0.0, size), (9_u64, pos, 0.0, size)],
    );
    let reverse = closest_body_ray_hit(
        eye,
        dir,
        crate::player::REACH,
        [(9_u64, pos, 0.0, size), (2_u64, pos, 0.0, size)],
    );

    assert_eq!(forward.map(|(id, _)| id), Some(2));
    assert_eq!(reverse.map(|(id, _)| id), Some(2));
}

#[test]
fn a_long_body_bow_stops_at_shore_before_its_centre_box_arrives() {
    let size = long_body();
    let boxes = |x: i32, y: i32, _z: i32| {
        if x == 2 && y == 0 {
            crate::block::Block::Stone.collision_boxes()
        } else {
            &[]
        }
    };
    // yaw -PI/2 faces +X. The bow already reaches x=1.5, leaving only
    // half a block before the shore at x=2; the old centre square would
    // have travelled 1.5 blocks before noticing it.
    let (moved, _, hit, _) = resolve_body_motion(
        Vec3::ZERO,
        -std::f32::consts::FRAC_PI_2,
        size,
        [2.0, 0.0, 0.0],
        1.0,
        crate::collision::STEP_HEIGHT,
        &boxes,
        &[],
        &[],
        1,
    );

    assert!(hit[0], "the bow, not just the centre, hits the shore");
    assert!(
        (0.49..=0.51).contains(&moved[0]),
        "the hull stops flush at shore: moved {}",
        moved[0]
    );
}

#[test]
fn a_long_body_cannot_rotate_its_bow_through_shore() {
    let size = long_body();
    let boxes = |x: i32, y: i32, _z: i32| {
        if x == 1 && y == 0 {
            crate::block::Block::Stone.collision_boxes()
        } else {
            &[]
        }
    };
    let pos = Vec3::ZERO;
    let requested = -std::f32::consts::FRAC_PI_2;
    assert!(
        body_boxes(pos, requested, size).any(|(min, max)| {
            crate::collision::aabb_hits_cells(min.to_array(), max.to_array(), boxes)
        }),
        "the unvalidated quarter-turn would put the bow in shore"
    );

    let accepted = clamp_body_yaw(pos, 0.0, requested, size, &boxes, &[], 1);
    assert_ne!(accepted, requested, "the clipping rotation is clamped");
    assert!(
        body_boxes(pos, accepted, size).all(|(min, max)| {
            !crate::collision::aabb_hits_cells(min.to_array(), max.to_array(), boxes)
        }),
        "every accepted body segment remains outside terrain"
    );
}

#[test]
fn a_long_body_touching_shore_can_rotate_away() {
    let size = long_body();
    let boxes = |x: i32, y: i32, _z: i32| {
        if x == 2 && y == 0 {
            crate::block::Block::Stone.collision_boxes()
        } else {
            &[]
        }
    };
    let pos = Vec3::new(0.5, 0.0, 0.0);
    let current = -std::f32::consts::FRAC_PI_2;
    let accepted = clamp_body_yaw(pos, current, 0.0, size, &boxes, &[], 1);

    assert_eq!(accepted, 0.0, "contact does not pin a hull turning away");
}
