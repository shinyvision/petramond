use super::*;
use crate::bbmodel::Model;
use crate::mob::model_meta::{self, SkBone};

fn boxed(pivot: Vec3, min: Vec3, max: Vec3, parent: Option<usize>) -> SkBone {
    SkBone {
        pivot,
        bbox_min: min,
        bbox_max: max,
        parent,
        welded: false,
    }
}

fn welded_box(pivot: Vec3, min: Vec3, max: Vec3, parent: Option<usize>) -> SkBone {
    SkBone {
        welded: true,
        ..boxed(pivot, min, max, parent)
    }
}

/// A solid floor (every cell below world y = 0) for the ragdoll tests.
fn floor(c: IVec3) -> bool {
    c.y < 0
}

/// A root box with one child box stacked above it, jointed at their shared face.
fn two_bone_skeleton() -> Skeleton {
    Skeleton {
        bones: vec![
            boxed(
                Vec3::new(0.0, 1.0, 0.0),
                Vec3::new(-0.5, 0.5, -0.5),
                Vec3::new(0.5, 1.5, 0.5),
                None,
            ),
            boxed(
                Vec3::new(0.0, 1.5, 0.0),
                Vec3::new(-0.5, 1.5, -0.5),
                Vec3::new(0.5, 2.5, 0.5),
                Some(0),
            ),
        ],
    }
}

fn sheep_skeleton() -> Skeleton {
    let src = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/assets/models/sheep.bbmodel"
    ));
    let model = Model::load(src).expect("sheep.bbmodel parses");
    model_meta::skeleton(&model)
}

#[test]
fn ragdoll_stays_connected_settles_above_ground_and_finishes() {
    let skel = two_bone_skeleton();
    let joint_rest = (skel.bones[1].pivot - skel.bones[0].pivot).length();
    // Launch + tumble — the realistic case — must NOT pull the joint apart.
    let mut rag = Ragdoll::pending(42, Vec3::X);
    rag.init(&skel, 0.25, Vec3::ZERO, 0.0);
    assert!(rag.is_initialized());

    for _ in 0..(LIFETIME / 0.05) as usize + 1 {
        rag.step(0.05, 0.25, Vec3::ZERO, 0.0, &floor);
        let p = rag.positions();
        // The joint pass keeps the child's joint locked to the root's; a rigid
        // rotation preserves the pivot-to-pivot distance, so this stays tight even as
        // the corpse flies and somersaults.
        let d = (p[1] - p[0]).length();
        assert!(
            (d - joint_rest).abs() < 0.1,
            "joints stay connected: {d} vs {joint_rest}"
        );
    }
    assert!(rag.is_done(), "the ragdoll finishes after its lifetime");
}

#[test]
fn the_body_tumbles_and_falls_over() {
    // A tall box dropped onto the floor must rotate (tip/tumble) — the whole rigid
    // body is simulated, not just joints.
    let skel = Skeleton {
        bones: vec![boxed(
            Vec3::new(0.0, 3.0, 0.0),
            Vec3::new(-0.5, 3.0, -0.5),
            Vec3::new(0.5, 6.0, 0.5),
            None,
        )],
    };
    let mut rag = Ragdoll::pending(7, Vec3::X);
    rag.init(&skel, 0.25, Vec3::ZERO, 0.0);
    let mut max_angle = 0.0f32;
    for _ in 0..36 {
        rag.step(0.05, 0.25, Vec3::ZERO, 0.0, &floor);
        let rot = rag.pose(1.0)[0].1;
        max_angle = max_angle.max(rot.angle_between(Quat::IDENTITY));
    }
    assert!(
        max_angle > 0.2,
        "the body rotated under physics (tipped/tumbled): {max_angle}"
    );
}

#[test]
fn the_killing_blow_flings_the_corpse_in_the_punched_direction() {
    // A box flung toward +X (high up, so it stays airborne) should travel +X.
    let skel = Skeleton {
        bones: vec![boxed(
            Vec3::new(0.0, 10.0, 0.0),
            Vec3::new(-0.5, 10.0, -0.5),
            Vec3::new(0.5, 11.0, 0.5),
            None,
        )],
    };
    let mut rag = Ragdoll::pending(3, Vec3::X);
    rag.init(&skel, 0.25, Vec3::ZERO, 0.0);
    let x0 = rag.pose(1.0)[0].0.x;
    for _ in 0..8 {
        rag.step(0.05, 0.25, Vec3::ZERO, 0.0, &floor);
    }
    let x1 = rag.pose(1.0)[0].0.x;
    assert!(
        x1 > x0 + 1.0,
        "the corpse flies in the punched (+X) direction: {x0} -> {x1}"
    );
}

#[test]
fn the_launch_never_drags_a_bone_toward_the_attacker() {
    // Two boxes stacked vertically (the lower one below the mob centre), flung +X high
    // up. The spin is bounded below the launch, so EVERY bone must travel +X (away) —
    // none swings back toward the attacker (the bug this guards).
    let skel = Skeleton {
        bones: vec![
            boxed(
                Vec3::new(0.0, 11.0, 0.0),
                Vec3::new(-0.5, 11.0, -0.5),
                Vec3::new(0.5, 12.0, 0.5),
                None,
            ),
            boxed(
                Vec3::new(0.0, 10.0, 0.0),
                Vec3::new(-0.5, 9.0, -0.5),
                Vec3::new(0.5, 10.0, 0.5),
                Some(0),
            ),
        ],
    };
    let mut rag = Ragdoll::pending(5, Vec3::X);
    rag.init(&skel, 0.25, Vec3::ZERO, 0.0);
    let x0: Vec<f32> = rag.pose(1.0).iter().map(|p| p.0.x).collect();
    for _ in 0..6 {
        rag.step(0.05, 0.25, Vec3::ZERO, 0.0, &floor);
    }
    let x1: Vec<f32> = rag.pose(1.0).iter().map(|p| p.0.x).collect();
    for (a, b) in x0.iter().zip(&x1) {
        assert!(
            b > a,
            "every bone flies away (+X), none toward the attacker: {a} -> {b}"
        );
    }
}

#[test]
fn the_launch_is_world_space_regardless_of_facing() {
    // The corpse must fly in the WORLD launch direction even when the mob faced some
    // other way at death — the renderer re-applies the mob's yaw, so the sim stores
    // the launch un-rotated into model space. (Without this, flight direction depends
    // on facing and looks random.)
    let skel = Skeleton {
        bones: vec![boxed(
            Vec3::new(0.0, 10.0, 0.0),
            Vec3::new(-0.5, 10.0, -0.5),
            Vec3::new(0.5, 11.0, 0.5),
            None,
        )],
    };
    let yaw = 1.3; // a non-zero facing
    let mut rag = Ragdoll::pending(3, Vec3::X); // world launch = +X
    rag.init(&skel, 0.25, Vec3::ZERO, yaw);
    let p0 = rag.pose(1.0)[0].0;
    for _ in 0..8 {
        rag.step(0.05, 0.25, Vec3::ZERO, yaw, &floor);
    }
    let p1 = rag.pose(1.0)[0].0;
    // The render applies `Ry(yaw)` to the model-space position, so transform the
    // displacement the same way and check it points along world +X.
    let disp = glam::Quat::from_rotation_y(yaw) * (p1 - p0);
    assert!(disp.x > 1.0, "flies along world +X: {disp:?}");
    assert!(
        disp.z.abs() < disp.x,
        "mostly +X, not flung sideways: {disp:?}"
    );
}

#[test]
fn a_corpse_rests_on_a_block_and_does_not_sink_through() {
    // A box dropped onto a solid floor (cells below world y=0) must settle on top, not
    // pass through it. scale 1.0 → model space == world space.
    let skel = Skeleton {
        bones: vec![boxed(
            Vec3::new(0.0, 3.0, 0.0),
            Vec3::new(-0.5, 3.0, -0.5),
            Vec3::new(0.5, 4.0, 0.5),
            None,
        )],
    };
    let mut rag = Ragdoll::pending(2, Vec3::ZERO); // no launch: drops straight down
    rag.init(&skel, 1.0, Vec3::ZERO, 0.0);
    for _ in 0..80 {
        rag.step(0.05, 1.0, Vec3::ZERO, 0.0, &floor);
    }
    assert!(
        rag.lowest_node_y() > -0.2,
        "corpse rests on the block top, doesn't sink through: {}",
        rag.lowest_node_y()
    );
}

#[test]
fn a_long_fall_lands_on_thick_ground_instead_of_sinking_in() {
    // Dropped from high up, the corpse crosses more than a cell per tick when it
    // lands — landing must park it on the surface, never inside the terrain.
    let skel = Skeleton {
        bones: vec![boxed(
            Vec3::new(0.0, 28.0, 0.0),
            Vec3::new(-0.5, 28.0, -0.5),
            Vec3::new(0.5, 29.0, 0.5),
            None,
        )],
    };
    let mut rag = Ragdoll::pending(4, Vec3::ZERO);
    rag.init(&skel, 1.0, Vec3::ZERO, 0.0);
    for _ in 0..(LIFETIME / 0.05) as usize {
        rag.step(0.05, 1.0, Vec3::ZERO, 0.0, &floor);
        assert!(
            rag.lowest_node_y() > -0.2,
            "no corner ever sinks into the ground: {}",
            rag.lowest_node_y()
        );
    }
}

#[test]
fn a_fast_falling_corpse_does_not_skip_through_a_thin_floor() {
    // A one-cell-thick floor far below: by landing time the corpse moves well over
    // a cell per tick, and an endpoint-only collision test never sees the floor.
    let thin = |c: IVec3| c.y == 0;
    let skel = Skeleton {
        bones: vec![boxed(
            Vec3::new(0.0, 26.0, 0.0),
            Vec3::new(-0.5, 26.0, -0.5),
            Vec3::new(0.5, 27.0, 0.5),
            None,
        )],
    };
    let mut rag = Ragdoll::pending(4, Vec3::ZERO);
    rag.init(&skel, 1.0, Vec3::ZERO, 0.0);
    for _ in 0..(LIFETIME / 0.05) as usize {
        rag.step(0.05, 1.0, Vec3::ZERO, 0.0, &thin);
        assert!(
            rag.lowest_node_y() > 0.8,
            "no corner ever passes into or below the thin floor: {}",
            rag.lowest_node_y()
        );
    }
}

#[test]
fn a_corner_embedded_at_death_heals_out_instead_of_falling_through() {
    // A mob can die with geometry slightly inside a movement-blocking cell (standing
    // on a partial block like farmland; a joint slide can also embed a corner
    // mid-life). Embedded corners must be pushed out of the nearest open face — if
    // collision is simply disabled for them, the limb sinks through the floor.
    let skel = Skeleton {
        bones: vec![boxed(
            Vec3::new(0.0, 0.5, 0.0),
            Vec3::new(-0.5, -0.1, -0.5),
            Vec3::new(0.5, 0.9, 0.5),
            None,
        )],
    };
    let mut rag = Ragdoll::pending(6, Vec3::ZERO);
    rag.init(&skel, 1.0, Vec3::ZERO, 0.0);
    for _ in 0..(LIFETIME / 0.05) as usize {
        rag.step(0.05, 1.0, Vec3::ZERO, 0.0, &floor);
    }
    assert!(
        rag.lowest_node_y() > -0.15,
        "embedded corners heal out and the corpse rests on the ground: {}",
        rag.lowest_node_y()
    );
}

#[test]
fn a_corpse_falls_off_the_edge_of_a_block() {
    // The floor only covers x < 0. A box dropped straddling the edge must drape/fall
    // off the unsupported (+X) side — its lowest corner ends well below the floor top.
    let solid = |c: IVec3| c.y < 0 && c.x < 0;
    let skel = Skeleton {
        bones: vec![boxed(
            Vec3::new(0.0, 4.0, 0.0),
            Vec3::new(-0.5, 4.0, -0.5),
            Vec3::new(0.5, 5.0, 0.5),
            None,
        )],
    };
    let mut rag = Ragdoll::pending(9, Vec3::ZERO);
    rag.init(&skel, 1.0, Vec3::ZERO, 0.0);
    for _ in 0..80 {
        rag.step(0.05, 1.0, Vec3::ZERO, 0.0, &solid);
    }
    assert!(
        rag.lowest_node_y() < -0.5,
        "corpse drops off the unsupported edge: lowest {}",
        rag.lowest_node_y()
    );
}

#[test]
fn sheep_scale_ragdoll_goes_limp_and_does_not_spin() {
    // The real sheep skeleton at its in-game scale, simulated for its full lifetime.
    // A corpse must tumble briefly and settle limp: bounded per-tick rotation is not
    // enough — the tornado bug span at the per-tick clamp EVERY tick (26+ rad total
    // per bone over the lifetime), so also bound each bone's CUMULATIVE rotation.
    let skel = sheep_skeleton();
    let mut rag = Ragdoll::pending(11, Vec3::X);
    rag.init(&skel, 0.0625, Vec3::ZERO, 0.0);

    let mut prev = rag.pose(1.0);
    let mut total = vec![0.0f32; skel.bones.len()];
    for _ in 0..(LIFETIME / 0.05) as usize {
        rag.step(0.05, 0.0625, Vec3::ZERO, 0.0, &floor);
        let pose = rag.pose(1.0);
        for (i, ((pos, rot), (_, old_rot))) in pose.iter().zip(&prev).enumerate() {
            assert!(pos.is_finite(), "ragdoll positions stay finite");
            assert!(rot.is_finite(), "ragdoll rotations stay finite");
            let turn = rot.angle_between(*old_rot);
            assert!(
                turn < 1.5,
                "ragdoll rotation is bounded per tick, not a tornado: {turn}"
            );
            total[i] += turn;
        }
        prev = pose;
    }
    for (i, t) in total.iter().enumerate() {
        assert!(
            *t < 10.0,
            "bone {i} settles instead of spinning: {t} rad total over the lifetime"
        );
    }
}

#[test]
fn hushjaw_corpse_collapses_to_the_ground_and_neither_freezes_nor_flips() {
    // The real hushjaw at its in-game scale: a huge geometry-bearing skull under a
    // cube-less rig root. The corpse must COLLAPSE — the physical root visibly drops
    // from its standing height instead of hanging in a statue pose off the rig
    // placeholder — and must settle without the placeholder-noise flip that turned
    // corpses upside down. Both were live bugs.
    let src = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/mods-src/monsters/pack/models/hushjaw.bbmodel"
    ));
    let model = Model::load(src).expect("hushjaw parses");
    let skel = model_meta::skeleton(&model);
    let body = (0..skel.bones.len())
        .find(|&i| skel.bones[i].parent.is_none())
        .expect("physical root");
    let rest_y = skel.bones[body].pivot.y;
    for seed in [1u64, 9] {
        let mut rag = Ragdoll::pending(seed, Vec3::X);
        rag.init(&skel, 0.04, Vec3::ZERO, 0.0);
        let mut prev = rag.pose(1.0);
        let mut total = 0.0f32;
        for _ in 0..(LIFETIME / 0.05) as usize {
            rag.step(0.05, 0.04, Vec3::ZERO, 0.0, &floor);
            let pose = rag.pose(1.0);
            let (_, rot) = pose[body];
            assert!(
                rot.angle_between(Quat::IDENTITY) < 2.0,
                "seed {seed}: the body never flips upside down"
            );
            total += rot.angle_between(prev[body].1);
            prev = pose;
        }
        assert!(
            total < 10.0,
            "seed {seed}: the body settles instead of spinning: {total} rad"
        );
        assert!(
            prev[body].0.y < rest_y - 5.0,
            "seed {seed}: the corpse collapsed to the ground, no statue: pivot y {} vs rest {rest_y}",
            prev[body].0.y
        );
        // The joint pass (last, by design) may leave leg tips slightly dug in when
        // the collapsed pose conflicts with the swing limit — but never deeper than
        // a fraction of a block, and never through the floor. World units.
        assert!(
            rag.lowest_node_y() * 0.04 > -0.2,
            "seed {seed}: the corpse rests on the ground, not through it: {} model units",
            rag.lowest_node_y()
        );
    }
}

#[test]
fn welded_bones_ride_their_anchor_rigidly_through_the_tumble() {
    // A welded bone — and a welded bone welded to it — must stay EXACTLY rigid with
    // its nearest physical ancestor for the whole flight: no sag, no joint swing, at
    // tick boundaries and mid-tick render alphas alike (hushjaw teeth on the jaw).
    let skel = Skeleton {
        bones: vec![
            boxed(
                Vec3::new(0.0, 4.0, 0.0),
                Vec3::new(-1.0, 4.0, -1.0),
                Vec3::new(1.0, 6.0, 1.0),
                None,
            ),
            welded_box(
                Vec3::new(0.0, 5.0, 1.0),
                Vec3::new(-0.5, 4.5, 1.0),
                Vec3::new(0.5, 5.5, 2.0),
                Some(0),
            ),
            welded_box(
                Vec3::new(0.0, 5.0, 2.0),
                Vec3::new(-0.25, 4.75, 2.0),
                Vec3::new(0.25, 5.25, 2.5),
                Some(1),
            ),
        ],
    };
    let mut rag = Ragdoll::pending(11, Vec3::X);
    rag.init(&skel, 0.25, Vec3::ZERO, 0.0);
    let mut tumbled = 0.0f32;
    for _ in 0..(LIFETIME / 0.05) as usize {
        rag.step(0.05, 0.25, Vec3::ZERO, 0.0, &floor);
        for alpha in [0.25, 1.0] {
            let pose = rag.pose(alpha);
            let (anchor_pos, anchor_rot) = pose[0];
            tumbled = tumbled.max(anchor_rot.angle_between(Quat::IDENTITY));
            for i in 1..3 {
                let (pos, rot) = pose[i];
                assert!(
                    rot.angle_between(anchor_rot) < 1e-3,
                    "welded bone {i} keeps its anchor's orientation"
                );
                let expected =
                    anchor_pos + anchor_rot * (skel.bones[i].pivot - skel.bones[0].pivot);
                assert!(
                    (pos - expected).length() < 1e-3,
                    "welded bone {i} rides the anchor's rigid transform: {pos:?} vs {expected:?}"
                );
            }
        }
    }
    assert!(
        tumbled > 0.2,
        "the anchor actually tumbled, so the rigidity was exercised: {tumbled}"
    );
}

#[test]
fn limbs_never_swing_past_the_joint_limit() {
    // The real sheep at its in-game scale, over its full lifetime: every child bone's
    // orientation relative to its parent stays within the joint swing limit — legs sag
    // with gravity but never fold into the body.
    let skel = sheep_skeleton();
    let mut rag = Ragdoll::pending(11, Vec3::X);
    rag.init(&skel, 0.0625, Vec3::ZERO, 0.0);
    let mut max_swing = 0.0f32;
    for _ in 0..(LIFETIME / 0.05) as usize {
        rag.step(0.05, 0.0625, Vec3::ZERO, 0.0, &floor);
        let pose = rag.pose(1.0);
        for (i, b) in skel.bones.iter().enumerate() {
            let Some(p) = b.parent else { continue };
            let rel = (pose[p].1.inverse() * pose[i].1).normalize();
            let swing = rel.angle_between(Quat::IDENTITY);
            assert!(
                swing <= MAX_JOINT_SWING + 0.05,
                "bone {i} stays within the joint swing limit: {swing}"
            );
            max_swing = max_swing.max(swing);
        }
    }
    // The joints are limp within the limit, not welded: some limb visibly sags.
    assert!(
        max_swing > 0.2,
        "limbs still swing under gravity within the limit: max {max_swing}"
    );
}

#[test]
fn rotation_extraction_is_exact_regardless_of_box_size() {
    // A perfectly rigid rotation must be recovered accurately even for a LARGE box
    // (a fine-grid model like the sheep, 16 units/m): the cross-covariance magnitude
    // grows with box size, and a scale-sensitive polar iteration returns garbage for
    // big boxes — the cause of the corpse-tornado bug. Cold start (identity) is the
    // worst case; in the sim it warm-starts from last tick's rotation.
    for half in [0.5f32, 2.0, 7.5] {
        let b = corners(Vec3::splat(-half), Vec3::new(half, half * 0.9, half * 1.5));
        let r = Quat::from_rotation_y(0.3) * Quat::from_rotation_x(0.2);
        let mut a = Mat3::ZERO;
        for q in b {
            a += outer(r * q, q);
        }
        let err = extract_rotation(a, Quat::IDENTITY).angle_between(r);
        assert!(
            err < 0.02,
            "rigid rotation recovered for box half-extent {half}: error {err} rad"
        );
    }
}

#[test]
fn uninitialised_ragdoll_has_no_pose() {
    let rag = Ragdoll::pending(1, Vec3::ZERO);
    assert!(!rag.is_initialized());
    assert!(rag.pose(0.5).is_empty(), "no bones before init");
}
