use super::launch::{nock, ray_box};
use super::rows::{ArrowRow, Draw};
use super::*;
use crate::strike::Aim;

const BOW: ItemId = ItemId(11);
const OTHER: ItemId = ItemId(12);

/// A synthetic bow: twelve ticks to full, eight of strain, four pull
/// frames with one missing.
fn draw() -> Draw {
    Draw {
        full_ticks: 12,
        strain_ticks: 8,
        speed_scale: 0.7,
        launch_speed: [5.0, 45.0],
    }
}

fn rows() -> Rows {
    Rows {
        bows: vec![BowRow {
            id: BOW,
            draw: draw(),
            pull: vec![Some("m:pull_1".into()), None, Some("m:pull_3".into())],
        }],
        arrows: vec![ArrowRow {
            id: ItemId(13),
            name: "m:arrow".into(),
            damage_weak: [1.0, 2.0],
            damage_full: [9.0, 18.0],
            speed_weak: 5.0,
            speed_full: 45.0,
        }],
    }
}

fn actor(held: Option<ItemId>, holds_use: bool) -> PlayerSnapshot {
    PlayerSnapshot {
        id: Some(PlayerId(0)),
        pos: [0.0; 3],
        vel: [0.0; 3],
        yaw: 0.0,
        pitch: 0.0,
        health: 20,
        on_ground: true,
        spectator: false,
        sneak: false,
        use_held: holds_use,
        holds_use,
        held,
        off_held: None,
        held_count: 1,
        pose_anchor: None,
        swing: Default::default(),
        half_width: 0.3,
        height: 1.8,
        eye_height: 1.62,
    }
}

/// The law under the press the composition hands it: the snapshot's own
/// `holds_use`.
fn bow<'a>(rows: &'a Rows, state: &PlayerSnapshot, clock: State) -> Bow<'a> {
    bow_of(rows, state, state.holds_use, clock)
}

fn drawing(ticks: f32) -> State {
    State::Drawing(ticks)
}

#[test]
fn a_draw_needs_the_press_and_the_bow_in_the_main_hand() {
    let rows = rows();
    let t = drawing(5.0);
    assert!(bow(&rows, &actor(Some(BOW), true), t).drawing);
    assert!(!bow(&rows, &actor(Some(BOW), false), t).drawing);
    assert!(!bow(&rows, &actor(Some(OTHER), true), t).drawing);
    assert!(
        !bow_of(&rows, &actor(Some(BOW), true), false, t).drawing,
        "a press an earlier rule holds is not a draw"
    );
    let mut spectator = actor(Some(BOW), true);
    spectator.spectator = true;
    assert!(!bow(&rows, &spectator, t).drawing);
}

/// A press the strain SPENT stays the bow's until the button comes up —
/// inert, but held — or the off-hand shield pops up unasked the instant
/// the arrow leaves.
#[test]
fn a_spent_press_stays_the_bows_and_claims_nothing_else() {
    let rows = rows();
    let spent = bow(&rows, &actor(Some(BOW), true), State::Spent);
    assert!(!spent.drawing);
    let claims = spent.claims();
    assert!(claims.holds_press, "nothing later in the list gets it");
    assert_eq!(claims.speed, 1.0);
    assert!(claims.denied.is_empty());
    assert_eq!(claims.display, [None, None]);
    assert!(claims.main.is_none() && claims.bones.is_empty());

    let released = bow(&rows, &actor(Some(BOW), false), State::Idle);
    assert!(!released.claims().holds_press);
}

/// The bow HOLDS its place through the draw (a bow that crept read as
/// the hand wandering) and only the strain moves it — a tremor that
/// grows, and rests exactly at the drawn pose when it starts.
#[test]
fn the_bow_holds_still_through_the_draw_and_shakes_only_under_strain() {
    let rows = rows();
    let at = |ticks: f32| bow(&rows, &actor(Some(BOW), true), drawing(ticks));
    let full = draw().full_ticks as f32;
    assert_eq!(at(1.0).pose(), at(full).pose(), "no creep");
    assert_eq!(at(full).pose(), at(0.5).pose());
    assert_eq!(at(full).shake(), [0.0; 2]);
    let late = at(full + draw().strain_ticks as f32 * 0.9);
    assert_ne!(late.pose(), at(full).pose(), "the strain trembles");
    let amp = |b: Bow| b.shake().iter().map(|v| v.abs()).sum::<f32>();
    assert!(amp(late) > 0.5, "and grows toward the end: {}", amp(late));
    assert_eq!(
        at(1.0).claims().speed,
        draw().speed_scale,
        "the draw claims the row's speed"
    );
}

/// The draw's states show through the row's frames, the last of which
/// is ALWAYS the full draw: a frame that came a tick early would show a
/// full bow that looses a weak arrow.
#[test]
fn the_last_frame_is_the_full_draw_and_nothing_less() {
    let rows = rows();
    let full = draw().full_ticks as f32;
    let stage = |ticks: f32| bow(&rows, &actor(Some(BOW), true), drawing(ticks)).stage();
    assert_eq!(stage(0.0), 0);
    assert_eq!(
        stage(full - 1.0),
        2,
        "one tick short is still the third frame"
    );
    assert_eq!(stage(full), 3);
    assert_eq!(stage(300.0), 3, "held past full stays full");
    let mut seen: Vec<usize> = (0..=draw().full_ticks).map(|t| stage(t as f32)).collect();
    seen.dedup();
    assert_eq!(seen, vec![0, 1, 2, 3], "every frame shows, in order");
    // A missing pull frame holds the previous one rather than blanking.
    let b = bow(&rows, &actor(Some(BOW), true), drawing(full * 0.7));
    assert_eq!(b.stage(), 2);
    assert_eq!(b.display(), Some("m:pull_1"));
    assert_eq!(
        bow(&rows, &actor(Some(BOW), true), drawing(full)).display(),
        Some("m:pull_3")
    );
    assert_eq!(
        bow(&rows, &actor(Some(BOW), false), drawing(full)).display(),
        None
    );
}

/// The clock reports a loose ONCE per press — on the release with the
/// draw held there, capped at full, or by itself when a full draw has
/// been held through the whole strain window, after which the press is
/// SPENT until the button comes up. A tap under a tick looses nothing.
#[test]
fn the_clock_looses_on_the_release_or_when_the_strain_runs_out() {
    let d = draw();
    let held = Press::Held(&d);
    let mut c = Clock::default();
    for _ in 0..(d.full_ticks + 5) {
        assert_eq!(c.step(held, 1.0), None);
    }
    assert_eq!(c.step(Press::Released, 1.0), Some(d.full_ticks));
    assert_eq!(c.step(Press::Released, 1.0), None, "one edge, one arrow");

    for _ in 0..7 {
        c.step(held, 1.0);
    }
    assert_eq!(c.step(Press::Released, 1.0), Some(7));

    c.step(held, 0.4);
    assert_eq!(c.step(Press::Released, 1.0), None, "a tap is not a draw");

    // Held on past the strain: it looses itself, once, and shows spent.
    let mut fired = Vec::new();
    for _ in 0..(d.full_ticks + d.strain_ticks + 40) {
        if let Some(t) = c.step(held, 1.0) {
            fired.push(t);
        }
    }
    assert_eq!(fired, vec![d.full_ticks], "the strain looses exactly once");
    assert_eq!(c.state(), State::Spent, "and the press is spent");
    assert_eq!(
        c.step(Press::Released, 1.0),
        None,
        "letting go of a spent press looses nothing"
    );
    c.step(held, 1.0);
    assert_eq!(c.state(), State::Drawing(1.0), "a fresh press draws again");
}

/// A press LOST mid-draw — the bow scrolled out of the hand, the body
/// dead or spectating — is a cancel, never a release: no arrow, the clock
/// reset, and the next press draws from nothing. Reading it as a release
/// loosed the arrow at the current draw with a pickaxe in hand.
#[test]
fn a_lost_press_cancels_the_draw_without_loosing() {
    let d = draw();
    let mut c = Clock::default();
    for _ in 0..(d.full_ticks + 2) {
        c.step(Press::Held(&d), 1.0);
    }
    assert_eq!(c.step(Press::Lost, 1.0), None, "no arrow");
    assert_eq!(c.state(), State::Idle);
    assert_eq!(c.step(Press::Released, 1.0), None, "nothing left to loose");
    c.step(Press::Held(&d), 1.0);
    assert_eq!(
        c.state(),
        State::Drawing(1.0),
        "a fresh draw starts from nothing"
    );

    // Lost while spent clears the spent latch too.
    let mut c = Clock::default();
    for _ in 0..(d.full_ticks + d.strain_ticks + 1) {
        c.step(Press::Held(&d), 1.0);
    }
    assert_eq!(c.state(), State::Spent);
    c.step(Press::Lost, 1.0);
    assert_eq!(c.state(), State::Idle);
}

/// Damage follows arrival speed between the arrow row's two rungs, so a
/// half draw lands halfway and a spent arrow lands soft; speed follows the
/// draw between the bow row's two.
#[test]
fn damage_runs_between_the_arrows_rungs_by_speed_and_speed_by_draw() {
    let rows = rows();
    let arrow = &rows.arrows[0];
    let d = draw();
    assert_eq!(arrow.damage_at(d.launch_speed(1)), arrow.damage_weak);
    assert_eq!(
        arrow.damage_at(d.launch_speed(d.full_ticks)),
        arrow.damage_full
    );
    let mid = arrow.damage_at(d.launch_speed(d.full_ticks / 2));
    assert!(mid[0] > arrow.damage_weak[0] && mid[0] < arrow.damage_full[0]);
    assert!(mid[1] > arrow.damage_weak[1] && mid[1] < arrow.damage_full[1]);
    assert_eq!(
        arrow.damage_at(0.0),
        arrow.damage_weak,
        "slower than any launch is the floor"
    );
    assert_eq!(
        arrow.damage_at(1e6),
        arrow.damage_full,
        "and nothing exceeds the ceiling"
    );
    assert!(d.launch_speed(1) < d.launch_speed(6) && d.launch_speed(6) < d.launch_speed(12));
    assert_eq!(d.launch_speed(0), d.launch_speed(1), "clamped below");
    assert_eq!(d.launch_speed(99), d.launch_speed(12), "clamped above");
}

/// A row missing any of its numbers is refused whole — a bow with half a
/// draw would draw under defaults nobody authored.
#[test]
fn an_incomplete_row_is_refused_whole() {
    let full = json::Value::parse(
        r#"{"draw_ticks": 12, "strain_ticks": 8, "draw_speed_scale": 0.7, "launch_speed": [5, 45]}"#,
    )
    .unwrap();
    assert_eq!(Draw::parse(&full), Some(draw()));
    let short = json::Value::parse(r#"{"draw_ticks": 12, "strain_ticks": 8}"#).unwrap();
    assert_eq!(Draw::parse(&short), None);
    let no_draw = json::Value::parse(
        r#"{"draw_ticks": 0, "strain_ticks": 8, "draw_speed_scale": 0.7, "launch_speed": [5, 45]}"#,
    )
    .unwrap();
    assert_eq!(Draw::parse(&no_draw), None, "a zero-tick draw is no draw");

    let arrow = json::Value::parse(
        r#"{"damage_weak": [1, 2], "damage_full": [9, 18], "speed_weak": 5, "speed_full": 45}"#,
    )
    .unwrap();
    assert_eq!(
        ArrowRow::parse(ItemId(13), "m:arrow".into(), &arrow),
        Some(rows().arrows[0].clone())
    );
    let inverted = json::Value::parse(
        r#"{"damage_weak": [1, 2], "damage_full": [9, 18], "speed_weak": 45, "speed_full": 5}"#,
    )
    .unwrap();
    assert_eq!(
        ArrowRow::parse(ItemId(13), "m:arrow".into(), &inverted),
        None
    );
}

/// The nock sits beside the eye (the player's right; getting the yaw
/// convention backwards shoots from the left, or behind, and no other
/// test would notice) and the shot CONVERGES on the crosshair: aimed at
/// what is under it, a shot from the offset nock passes through that
/// exact point.
#[test]
fn the_arrow_leaves_beside_the_eye_and_converges_on_the_crosshair() {
    let mut a = actor(Some(BOW), true);
    a.pos = [1.0, 2.0, 3.0];
    let (from, dir) = nock(&a, None);
    assert!(from[1] < 2.0 + a.eye_height && from[1] > 2.0 + a.eye_height - 0.5);
    assert!(dir[2] > 0.99, "yaw 0 looks down +Z: {dir:?}");
    assert!(
        from[0] < 1.0 && (from[2] - 3.0).abs() < 1e-6,
        "off to the right: {from:?}"
    );
    a.yaw = std::f32::consts::FRAC_PI_2;
    assert!(nock(&a, None).1[0] > 0.99);

    a.yaw = 0.0;
    let aim = Aim::of(&a);
    let target = 4.0;
    let at = [aim.eye[0], aim.eye[1], aim.eye[2] + aim.forward[2] * target];
    let (from, dir) = nock(&a, Some(target));
    let t = (at[2] - from[2]) / dir[2];
    let hit = [
        from[0] + dir[0] * t,
        from[1] + dir[1] * t,
        from[2] + dir[2] * t,
    ];
    for axis in 0..3 {
        assert!(
            (hit[axis] - at[axis]).abs() < 1e-4,
            "converges: {hit:?} vs {at:?}"
        );
    }
}

#[test]
fn a_ray_enters_a_box_at_its_near_face_and_misses_beside_it() {
    let b = ([2.0, 0.0, -1.0], [3.0, 2.0, 1.0]);
    assert!((ray_box([0.0, 1.0, 0.0], [1.0, 0.0, 0.0], b).unwrap() - 2.0).abs() < 1e-6);
    assert_eq!(ray_box([0.0, 1.0, 5.0], [1.0, 0.0, 0.0], b), None);
    assert_eq!(
        ray_box([0.0, 1.0, 0.0], [-1.0, 0.0, 0.0], b),
        None,
        "behind"
    );
}
