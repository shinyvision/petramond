use super::*;
use petramond::net::protocol::Transform;
use petramond_math::math::Vec3;
use petramond_math::world_pos::WorldPos;

fn row(id: u8, pos: WorldPos) -> PlayerStateRow {
    PlayerStateRow {
        conditions: Vec::new(),
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
        animator: Default::default(),
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
    let p1 = WorldPos::new(1.0, 70.0, 1.0);
    let p2 = WorldPos::new(2.0, 70.0, 1.0);

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
    assert_eq!(mid, WorldPos::new(1.5, 70.0, 1.0));
}

#[test]
fn interpolation_lerps_yaw_across_the_wrap_seam() {
    use std::f32::consts::PI;
    let mut a = row(1, WorldPos::ZERO);
    a.transform.yaw = PI - 0.1;
    let mut b = row(1, WorldPos::ZERO);
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
    let here = WorldPos::new(1.0, 70.0, 1.0);
    let far = WorldPos::new(500.0, 90.0, -300.0);
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

/// A fired graph event — the engine's own break resolved on the body rig,
/// exactly as the server emits it — reaches the remote body's animator
/// inputs once, through the same lane a mod's fire rides, and never twice.
#[test]
fn a_fired_action_reaches_the_body_animator_exactly_once() {
    use petramond::player::one_shot::{self, OneShot};
    use petramond::player::rigs;
    use petramond_world::inventory::Hand;

    let body = rigs::id(rigs::PLAYER_BODY).expect("body rig");
    let event = one_shot::resolve(body, Hand::Main, OneShot::Break).expect("break event");
    let action = PlayerActionKind::Animator { rig: body, event };
    let mut store = RemotePlayers::default();
    // Two batches in one window both carry the row: one edge this frame.
    store.apply(
        &[row(1, WorldPos::ZERO)],
        &[(PlayerId(1), action), (PlayerId(1), action)],
        PlayerId(0),
    );
    store.advance(1.0 / 60.0, 1.0, |_| MovementMedium::Land);
    let events = |store: &RemotePlayers| store.iter().next().unwrap().events.clone();
    assert_eq!(events(&store), vec![(body, event)], "one edge per frame");

    // The latch is consumed: the next frame carries no second break.
    store.advance(1.0 / 60.0, 1.0, |_| MovementMedium::Land);
    assert!(events(&store).is_empty(), "one edge, no re-trigger");
}

fn scrub(slot: u16, clip: u16, progress: f32) -> AnimatorPlay {
    AnimatorPlay {
        rig: RigId(0),
        slot,
        clip,
        clock: AnimatorClock::Scrub(progress),
        mirror: false,
        priority: 0,
    }
}

fn presented(
    prev: &[AnimatorPlay],
    curr: &[AnimatorPlay],
    last: &[AnimatorPlay],
    alpha: f32,
) -> Vec<f32> {
    let mut out = Vec::new();
    present_plays(prev, curr, last, alpha, 0.5, &mut out);
    out.iter().map(|p| p.progress().unwrap()).collect()
}

/// A scrub the rows show climbing is drawn where it is NOW, not where the
/// tick-old row left it: carried forward by the step the last two rows took.
#[test]
fn a_climbing_scrub_is_carried_forward_by_the_step_the_last_two_rows_took() {
    let prev = [scrub(0, 1, 0.9), scrub(1, 1, 0.2)];
    let got = presented(&prev, &[scrub(1, 1, 0.3)], &[], 0.5);
    assert!(
        (got[0] - 0.35).abs() < 1e-5,
        "halfway into the next step: {got:?}"
    );
    let got = presented(&[scrub(1, 1, 0.8)], &[scrub(1, 1, 0.95)], &[], 1.0);
    assert_eq!(got, [1.0], "never past the clip's end");
}

#[test]
fn a_restarted_or_changed_scrub_is_never_carried_backward() {
    let got = presented(
        &[scrub(0, 1, 0.9)],
        &[scrub(0, 1, 0.1)],
        &[scrub(0, 1, 0.97)],
        0.5,
    );
    assert_eq!(got, [0.1], "a restart snaps to the row");
    let got = presented(
        &[scrub(0, 1, 0.5)],
        &[scrub(0, 1, 0.45)],
        &[scrub(0, 1, 0.55)],
        0.5,
    );
    assert!(
        (got[0] - 0.5).abs() < 1e-5,
        "a small step back eases from what was drawn: {got:?}"
    );
    let got = presented(
        &[scrub(0, 1, 0.2)],
        &[scrub(0, 2, 0.3)],
        &[scrub(0, 1, 0.25)],
        0.5,
    );
    assert_eq!(got, [0.3], "another clip in the slot is a new play");
}

#[test]
fn hurt_edge_runs_a_decaying_flash_envelope() {
    let mut store = RemotePlayers::default();
    let mut hurt = row(1, WorldPos::ZERO);
    hurt.hurt_recent = true;
    apply(&mut store, &[hurt]);
    assert_eq!(store.iter().next().unwrap().hurt_flash01(), 1.0);
    store.advance(HURT_FLASH_SECS * 0.5, 1.0, |_| MovementMedium::Land);
    let mid = store.iter().next().unwrap().hurt_flash01();
    assert!(mid > 0.0 && mid < 1.0, "the flash decays: {mid}");
    store.advance(HURT_FLASH_SECS, 1.0, |_| MovementMedium::Land);
    assert_eq!(store.iter().next().unwrap().hurt_flash01(), 0.0);
}
