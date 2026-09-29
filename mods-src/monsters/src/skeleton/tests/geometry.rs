use std::f32::consts::FRAC_PI_2;

use mod_sdk::mob_facing_xz;

use crate::skeleton::geometry::{
    gap, segment_meets_box, turn_toward, within_arc, yaw_toward, Upright,
};

const AT: [f64; 3] = [10.0, 64.0, -3.0];

fn toward(dx: f64, dz: f64) -> [f64; 3] {
    [AT[0] + dx, AT[1], AT[2] + dz]
}

#[test]
fn a_body_turned_toward_a_point_faces_it() {
    for (dx, dz) in [(0.0, -5.0), (5.0, 0.0), (-3.0, 4.0), (2.0, 2.0), (0.0, 7.0)] {
        let yaw = yaw_toward(AT, toward(dx, dz)).expect("the point has a bearing");
        let [fx, fz] = mob_facing_xz(yaw);
        let len = (dx * dx + dz * dz).sqrt();
        assert!(
            (f64::from(fx) - dx / len).abs() < 1e-5 && (f64::from(fz) - dz / len).abs() < 1e-5,
            "yaw {yaw} faces ({fx}, {fz}), not ({dx}, {dz})"
        );
    }
    assert_eq!(yaw_toward(AT, [AT[0], 70.0, AT[2]]), None);
}

#[test]
fn the_guard_arc_covers_the_front_only() {
    let facing_north = 0.0;
    assert!(
        within_arc(facing_north, AT, toward(0.0, -4.0), 70.0),
        "dead ahead is −Z"
    );
    assert!(
        !within_arc(facing_north, AT, toward(0.0, 4.0), 70.0),
        "behind"
    );
    let at = |deg: f32| {
        let r = deg.to_radians();
        toward(f64::from(r.sin()) * 3.0, -f64::from(r.cos()) * 3.0)
    };
    assert!(within_arc(facing_north, AT, at(60.0), 70.0));
    assert!(within_arc(facing_north, AT, at(-60.0), 70.0));
    assert!(!within_arc(facing_north, AT, at(80.0), 70.0));
    assert!(!within_arc(facing_north, AT, at(-80.0), 70.0));
    let facing_east = -FRAC_PI_2;
    assert!(
        within_arc(facing_east, AT, toward(4.0, 0.0), 70.0),
        "turned east, +X is the front"
    );
    assert!(!within_arc(facing_east, AT, toward(-4.0, 0.0), 70.0));
    assert!(
        !within_arc(facing_north, AT, [AT[0], AT[1] + 1.0, AT[2]], 180.0),
        "a point with no bearing is in no arc"
    );
}

#[test]
fn turning_takes_the_short_way_and_stops_on_the_target() {
    let across_the_seam = turn_toward(3.0, -3.0, 0.1);
    assert!(
        (across_the_seam - 3.1).abs() < 1e-5,
        "went the long way: {across_the_seam}"
    );
    let landed = turn_toward(3.0, -3.0, 0.3);
    assert!((landed + 3.0).abs() < 1e-5, "overshot: {landed}");
    assert!((turn_toward(1.0, 1.1, 0.3) - 1.1).abs() < 1e-6);
}

#[test]
fn the_gap_between_bodies_is_edge_to_edge() {
    let a = Upright {
        feet: [0.0, 64.0, 0.0],
        half_width: 0.3,
        height: 1.8,
    };
    let beside = Upright {
        feet: [2.0, 64.0, 0.0],
        ..a
    };
    assert!((gap(&a, &beside) - 1.4).abs() < 1e-5);
    let above = Upright {
        feet: [0.0, 67.0, 0.0],
        ..a
    };
    assert!((gap(&a, &above) - 1.2).abs() < 1e-5);
    let touching = Upright {
        feet: [0.5, 64.5, 0.0],
        ..a
    };
    assert_eq!(gap(&a, &touching), 0.0);
}

#[test]
fn a_segment_meets_only_the_boxes_it_crosses() {
    let (min, max) = ([4.0, 0.0, -1.0], [5.0, 2.0, 1.0]);
    assert!(segment_meets_box(
        [0.0, 1.0, 0.0],
        [10.0, 1.0, 0.0],
        min,
        max
    ));
    assert!(
        !segment_meets_box([0.0, 1.0, 0.0], [3.0, 1.0, 0.0], min, max),
        "stops short"
    );
    assert!(
        !segment_meets_box([0.0, 3.0, 0.0], [10.0, 3.0, 0.0], min, max),
        "passes over"
    );
}
