use super::*;
use petramond::net::protocol::Transform;

fn row(id: u8, pos: Vec3) -> PlayerStateRow {
    PlayerStateRow {
        id: PlayerId(id),
        transform: Transform {
            pos,
            vel: Vec3::ZERO,
            yaw: 0.0,
            pitch: 0.0,
        },
        on_ground: true,
        sneaking: false,
        sleeping: false,
        sleep_yaw: None,
        alive: true,
        visible: true,
        held_item: None,
        held_data: None,
        off_hand_item: None,
        off_hand_data: None,
        mining: None,
        eating: false,
        eating_off_hand: false,
        held_pose_main: None,
        held_pose_off: None,
        held_display: [None; 2],
        bone_poses: Vec::new(),
        motion_claims: [Default::default(); 2],
        hurt_recent: false,
        snap: false,
        mount: None,
    }
}

fn apply(store: &mut RemotePlayers, rows: &[PlayerStateRow]) {
    store.apply(rows, &[], PlayerId(0));
}

#[test]
fn store_pairs_batches_skips_own_id_and_drops_absent_ids() {
    let mut store = RemotePlayers::default();
    let p1 = Vec3::new(1.0, 70.0, 1.0);
    let p2 = Vec3::new(2.0, 70.0, 1.0);

    // Own id (0) skipped entirely; fresh ids start prev == curr.
    apply(&mut store, &[row(0, p1), row(1, p1), row(2, p1)]);
    assert_eq!(store.len(), 2, "the recipient's own row is never stored");
    let fresh = store.iter().next().expect("id 1 stored");
    assert_eq!(
        fresh.prev.transform.pos, p1,
        "a fresh id interpolates from itself"
    );

    apply(&mut store, &[row(1, p2)]);
    assert_eq!(store.len(), 1, "id 2 absent from the batch: dropped");
    let paired = store.iter().next().unwrap();
    assert_eq!(
        paired.prev.transform.pos, p1,
        "previous batch became the prev row"
    );
    assert_eq!(paired.curr.transform.pos, p2);
    // Midpoint interpolation over the pair.
    let (mid, _, _) = interpolate(&paired.prev, &paired.curr, 0.5);
    assert_eq!(mid, Vec3::new(1.5, 70.0, 1.0));
}

#[test]
fn interpolation_lerps_yaw_across_the_wrap_seam() {
    use std::f32::consts::PI;
    let mut a = row(1, Vec3::ZERO);
    a.transform.yaw = PI - 0.1;
    let mut b = row(1, Vec3::ZERO);
    b.transform.yaw = -PI + 0.1;
    let (_, yaw, _) = interpolate(&a, &b, 0.5);
    assert!(
        petramond_math::math::wrap_angle(yaw - PI).abs() < 1e-5,
        "yaw crosses the seam the short way: {yaw}"
    );
}

#[test]
fn snap_rows_skip_interpolation() {
    let mut store = RemotePlayers::default();
    let here = Vec3::new(1.0, 70.0, 1.0);
    let far = Vec3::new(500.0, 90.0, -300.0);
    apply(&mut store, &[row(1, here)]);
    let mut tp = row(1, far);
    tp.snap = true;
    apply(&mut store, &[tp]);
    let p = store.iter().next().unwrap();
    assert_eq!(
        p.prev.transform.pos, far,
        "a snap row adopts into BOTH pair slots"
    );
    let (pos, _, _) = interpolate(&p.prev, &p.curr, 0.25);
    assert_eq!(pos, far, "no frame lerps across the teleport");
}

#[test]
fn a_broke_action_latches_exactly_one_animator_jab() {
    let mut store = RemotePlayers::default();
    store.apply(
        &[row(1, Vec3::ZERO)],
        &[(PlayerId(1), PlayerActionKind::Broke)],
        PlayerId(0),
    );
    store.advance(1.0 / 60.0, 1.0, |_| MovementMedium::Land);
    let s1 = store.iter().next().unwrap().view.swing;
    assert!(s1 > 0.0, "the latched break starts a swing");

    // The latch is consumed: the next frame CONTINUES the same swing
    // (advances forward) rather than restarting a new jab at phase 0.
    store.advance(1.0 / 60.0, 1.0, |_| MovementMedium::Land);
    let s2 = store.iter().next().unwrap().view.swing;
    assert!(s2 > s1, "one jab continues, no re-trigger: {s2} vs {s1}");

    // And it completes back to rest within a swing period.
    store.advance(0.5, 1.0, |_| MovementMedium::Land);
    assert_eq!(store.iter().next().unwrap().view.swing, 0.0);
}

#[test]
fn hurt_edge_runs_a_decaying_flash_envelope() {
    let mut store = RemotePlayers::default();
    let mut hurt = row(1, Vec3::ZERO);
    hurt.hurt_recent = true;
    apply(&mut store, &[hurt]);
    assert_eq!(store.iter().next().unwrap().hurt_flash01(), 1.0);
    store.advance(HURT_FLASH_SECS * 0.5, 1.0, |_| MovementMedium::Land);
    let mid = store.iter().next().unwrap().hurt_flash01();
    assert!(mid > 0.0 && mid < 1.0, "the flash decays: {mid}");
    store.advance(HURT_FLASH_SECS, 1.0, |_| MovementMedium::Land);
    assert_eq!(store.iter().next().unwrap().hurt_flash01(), 0.0);
}
