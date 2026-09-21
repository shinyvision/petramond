use std::sync::Arc;

use glam::Vec3;
use petramond::player::{AnimatorClock, AnimatorPlay, RigId};
use petramond_world::animation::library::ClipLibrary;
use petramond_world::animation::test_rig::{clip, rig};
use petramond_world::animation::{Animator, Graph};

use super::ClaimDriver;
use crate::views::AnimatorParamRow;
use crate::AnimatorInputs;

const RIG: RigId = RigId(3);
const OTHER_RIG: RigId = RigId(4);
const DT: f32 = 1.0 / 60.0;

/// A param `x` (default 0), one slot `s`, an event `e` that plays `b` in it,
/// an event `cut` that plays `b` there at a priority no claim outranks, and
/// two one-second clips: `a` turns the left arm 0 → 100°, `b` holds 10°.
fn animator() -> Animator {
    let m = rig();
    let mut lib = ClipLibrary::new();
    lib.insert(
        "a",
        clip(
            &m,
            1.0,
            false,
            &[
                ("leftArm", 0.0, Vec3::ZERO),
                ("leftArm", 1.0, Vec3::X * 100.0),
            ],
        ),
    );
    lib.insert(
        "b",
        clip(&m, 1.0, false, &[("leftArm", 0.0, Vec3::X * 10.0)]),
    );
    let graph = Graph::compile(
        r#"{ "params": { "x": 0 }, "events": ["e", "cut"], "slots": ["s"],
             "layers": [{ "slot": "s" }],
             "rules": [
                { "on": "e", "slot": "s", "play": "b" },
                { "on": "cut", "slot": "s", "play": "b", "priority": 5,
                  "blend": "inertial", "fade_in": 0, "fade_out": 0 }
             ] }"#,
        &m,
        lib,
    )
    .unwrap_or_else(|e| panic!("{e}"));
    Animator::new(Arc::new(graph), 1)
}

fn param(rig: RigId, value: f32) -> AnimatorParamRow {
    AnimatorParamRow {
        rig,
        param: 0,
        value,
    }
}

fn play(rig: RigId, clip: u16, clock: AnimatorClock) -> AnimatorPlay {
    AnimatorPlay {
        rig,
        slot: 0,
        clip,
        clock,
        mirror: false,
        priority: 0,
    }
}

/// One frame in driver order: release, the engine's own write, the claims.
fn frame(driver: &mut ClaimDriver, a: &mut Animator, engine: f32, inputs: AnimatorInputs<'_>) {
    driver.release(a);
    a.set(a.graph().param("x").unwrap(), engine);
    driver.claim(a, inputs);
    a.update(DT);
}

fn x(a: &Animator) -> f32 {
    a.param(a.graph().param("x").unwrap())
}

#[test]
fn a_released_param_shows_the_engines_value_the_same_frame_never_the_default() {
    let mut a = animator();
    let mut driver = ClaimDriver::new(RIG, a.graph());
    let claimed = [param(RIG, 5.0)];
    frame(
        &mut driver,
        &mut a,
        3.0,
        AnimatorInputs {
            params: &claimed,
            ..Default::default()
        },
    );
    assert_eq!(x(&a), 5.0, "a standing claim wins over the engine's value");
    frame(&mut driver, &mut a, 3.0, AnimatorInputs::default());
    assert_eq!(
        x(&a),
        3.0,
        "the release uncovers the engine's value, not the default 0"
    );
}

#[test]
fn claims_on_another_rig_are_not_this_drivers() {
    let mut a = animator();
    let mut driver = ClaimDriver::new(RIG, a.graph());
    let params = [param(OTHER_RIG, 5.0)];
    let plays = [play(OTHER_RIG, 0, AnimatorClock::Scrub(0.5))];
    frame(
        &mut driver,
        &mut a,
        3.0,
        AnimatorInputs {
            params: &params,
            plays: &plays,
            events: &[],
        },
    );
    assert_eq!(x(&a), 3.0);
    assert!(a.playing(a.graph().slot("s").unwrap()).is_none());
}

#[test]
fn a_scrubbed_claim_poses_the_rig_as_its_clip_at_the_claimed_progress() {
    let mut a = animator();
    let mut driver = ClaimDriver::new(RIG, a.graph());
    let arm = a
        .graph()
        .clips()
        .get(a.graph().clips().id("a").unwrap())
        .clone();
    let bone = rig().bone_named("leftArm").unwrap();
    let claimed = [play(RIG, 0, AnimatorClock::Scrub(0.4))];
    for _ in 0..30 {
        frame(
            &mut driver,
            &mut a,
            0.0,
            AnimatorInputs {
                plays: &claimed,
                ..Default::default()
            },
        );
    }
    let want = arm
        .sample(
            bone,
            petramond_world::bbmodel::Channel::Rotation,
            0.4 * arm.length,
        )
        .unwrap();
    let got = a.pose().rotation(bone);
    assert!(
        (got - want).length() < 0.1,
        "drawn {got}, the clip at 40% is {want}"
    );
}

#[test]
fn a_clip_change_keeps_the_slot_and_a_dropped_slot_stops() {
    let mut a = animator();
    let mut driver = ClaimDriver::new(RIG, a.graph());
    let slot = a.graph().slot("s").unwrap();
    let first = [play(RIG, 0, AnimatorClock::Scrub(0.25))];
    frame(
        &mut driver,
        &mut a,
        0.0,
        AnimatorInputs {
            plays: &first,
            ..Default::default()
        },
    );
    let playing = a.playing(slot).expect("the claimed clip plays");
    assert_eq!(a.graph().clips().name(playing.clip), "a");
    assert!(
        (playing.progress - 0.25).abs() < 1e-3,
        "sought to the claimed progress"
    );

    let second = [play(RIG, 1, AnimatorClock::Scrub(0.5))];
    frame(
        &mut driver,
        &mut a,
        0.0,
        AnimatorInputs {
            plays: &second,
            ..Default::default()
        },
    );
    let playing = a
        .playing(slot)
        .expect("the slot stays claimed across a clip change");
    assert_eq!(a.graph().clips().name(playing.clip), "b");

    for _ in 0..60 {
        frame(&mut driver, &mut a, 0.0, AnimatorInputs::default());
    }
    assert!(a.playing(slot).is_none(), "an unclaimed slot fades out");
}

#[test]
fn a_run_restarts_when_its_clock_changes_and_a_scrub_never_does() {
    let mut a = animator();
    let mut driver = ClaimDriver::new(RIG, a.graph());
    let slot = a.graph().slot("s").unwrap();
    let run = |rate| {
        [play(
            RIG,
            0,
            AnimatorClock::Run {
                rate,
                looping: true,
            },
        )]
    };
    for _ in 0..30 {
        frame(
            &mut driver,
            &mut a,
            0.0,
            AnimatorInputs {
                plays: &run(1.0),
                ..Default::default()
            },
        );
    }
    let before = a.playing(slot).unwrap().time;
    assert!(before > 0.4, "a run advances on its own: {before}");
    frame(
        &mut driver,
        &mut a,
        0.0,
        AnimatorInputs {
            plays: &run(2.0),
            ..Default::default()
        },
    );
    let restarted = a.playing(slot).unwrap().time;
    assert!(
        restarted < 0.1,
        "a rate change re-plays from the start: {restarted}"
    );

    for progress in [0.2, 0.6] {
        let scrub = [play(RIG, 0, AnimatorClock::Scrub(progress))];
        frame(
            &mut driver,
            &mut a,
            0.0,
            AnimatorInputs {
                plays: &scrub,
                ..Default::default()
            },
        );
        let at = a.playing(slot).unwrap().progress;
        assert!(
            (at - progress).abs() < 1e-3,
            "a scrub is sought, not restarted: {at}"
        );
    }
}

#[test]
fn a_fired_event_runs_its_rule_only_on_its_own_rig() {
    let mut a = animator();
    let mut driver = ClaimDriver::new(RIG, a.graph());
    let slot = a.graph().slot("s").unwrap();
    let e = a.graph().event("e").unwrap().index() as u16;
    frame(
        &mut driver,
        &mut a,
        0.0,
        AnimatorInputs {
            events: &[(OTHER_RIG, e)],
            ..Default::default()
        },
    );
    assert!(a.playing(slot).is_none(), "another rig's event");
    frame(
        &mut driver,
        &mut a,
        0.0,
        AnimatorInputs {
            events: &[(RIG, e)],
            ..Default::default()
        },
    );
    let playing = a.playing(slot).expect("the rule answered the event");
    assert_eq!(a.graph().clips().name(playing.clip), "b");
}

/// A rule that takes a claimed slot never drives the claim's scrub — the
/// rule's clip runs at its own rate whatever progress the claim states —
/// and once the rule has played out the claim comes back on its own
/// montage, at its claimed progress.
#[test]
fn a_rule_in_a_claimed_slot_never_takes_the_scrub_and_the_claim_comes_back_after() {
    let mut a = animator();
    let mut driver = ClaimDriver::new(RIG, a.graph());
    let slot = a.graph().slot("s").unwrap();
    let cut = a.graph().event("cut").unwrap().index() as u16;
    let mut frame_at = |a: &mut Animator, progress: f32, events: &[(RigId, u16)]| {
        let claimed = [play(RIG, 0, AnimatorClock::Scrub(progress))];
        frame(
            &mut driver,
            a,
            0.0,
            AnimatorInputs {
                plays: &claimed,
                events,
                ..Default::default()
            },
        );
    };
    let clip = |a: &Animator| {
        a.graph()
            .clips()
            .name(a.playing(slot).expect("playing").clip)
            .to_string()
    };

    frame_at(&mut a, 0.5, &[]);
    assert_eq!(clip(&a), "a");
    frame_at(&mut a, 0.5, &[(RIG, cut)]);
    assert_eq!(clip(&a), "b", "the higher-priority rule takes the slot");
    let mut last = a.playing(slot).unwrap().time;
    for progress in [0.9, 0.1, 0.7] {
        frame_at(&mut a, progress, &[]);
        assert_eq!(clip(&a), "b");
        let time = a.playing(slot).unwrap().time;
        assert!(
            (time - last - DT).abs() < 1e-4,
            "the rule's clip runs unscrubbed: {last} -> {time}"
        );
        last = time;
    }

    for _ in 0..70 {
        frame_at(&mut a, 0.3, &[]);
    }
    assert_eq!(
        clip(&a),
        "a",
        "the claim is back once the rule has played out"
    );
    assert!((a.playing(slot).unwrap().progress - 0.3).abs() < 1e-3);
}

/// A run claimed to play once plays to its end and stays ended while the
/// claim stands: a standing claim is not a loop.
#[test]
fn a_run_that_plays_out_stays_ended_while_its_claim_stands() {
    let mut a = animator();
    let mut driver = ClaimDriver::new(RIG, a.graph());
    let slot = a.graph().slot("s").unwrap();
    let once = [play(
        RIG,
        0,
        AnimatorClock::Run {
            rate: 4.0,
            looping: false,
        },
    )];
    frame(
        &mut driver,
        &mut a,
        0.0,
        AnimatorInputs {
            plays: &once,
            ..Default::default()
        },
    );
    assert!(a.playing(slot).is_some(), "the run starts");
    for _ in 0..40 {
        frame(
            &mut driver,
            &mut a,
            0.0,
            AnimatorInputs {
                plays: &once,
                ..Default::default()
            },
        );
    }
    assert!(
        a.playing(slot).is_none(),
        "played out in a quarter second and never restarted"
    );
}
