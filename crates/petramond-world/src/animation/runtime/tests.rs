use std::sync::Arc;

use glam::Vec3;

use super::{Animator, PlaySpec, PlayState};
use crate::animation::expr::intern;
use crate::animation::pose::LocalPose;
use crate::animation::graph::Graph;
use crate::animation::library::ClipLibrary;
use crate::animation::test_rig::{clip, rig, with_marker};
use crate::bbmodel::{Channel, Interpolation, Keyframe, Model};

const DT: f32 = 0.01;

fn animator(rig: &Model, clips: ClipLibrary, source: &str) -> Animator {
    let graph = Graph::compile(source, rig, clips).unwrap_or_else(|e| panic!("{e}"));
    Animator::new(Arc::new(graph), 7)
}

/// Update for `seconds`, answering `bone`'s X rotation after each frame.
fn run(a: &mut Animator, seconds: f32, dt: f32, bone: usize) -> Vec<f32> {
    let frames = (seconds / dt).round() as usize;
    (0..frames)
        .map(|_| {
            a.update(dt);
            a.pose().rotation(bone).x
        })
        .collect()
}

fn playing(a: &Animator, slot: &str) -> Option<String> {
    let slot = a.graph().slot(slot).expect("slot");
    a.playing(slot)
        .map(|p| a.graph().clips().name(p.clip).to_string())
}

#[test]
fn a_crossfade_blends_continuously_and_reentering_a_fading_state_never_pops() {
    let m = rig();
    let arm = m.bone_named("leftArm").unwrap();
    let mut lib = ClipLibrary::new();
    lib.insert("idle", clip(&m, 1.0, true, &[("leftArm", 0.0, Vec3::ZERO)]));
    lib.insert("walk", clip(&m, 1.0, true, &[("leftArm", 0.0, Vec3::X * 60.0)]));
    let mut a = animator(
        &m,
        lib,
        r#"{
        "params": { "speed": 0 },
        "layers": [{ "states": { "idle": "idle", "walk": "walk" }, "initial": "idle",
            "transitions": [
                { "from": "idle", "to": "walk", "when": "speed > 1", "fade": 0.2, "ease": "linear" },
                { "from": "walk", "to": "idle", "when": "speed <= 1", "fade": 0.2, "ease": "linear" }
            ] }]
    }"#,
    );
    let speed = a.graph().param("speed").unwrap();
    assert!(run(&mut a, 0.1, DT, arm).iter().all(|x| *x == 0.0));

    a.set(speed, 2.0);
    let fading = run(&mut a, 0.1, DT, arm);
    assert!((fading[4] - 15.0).abs() < 1e-3, "a quarter through a linear fade: {fading:?}");
    assert!(fading.windows(2).all(|w| w[1] > w[0]));

    a.set(speed, 0.0);
    let back = run(&mut a, 0.3, DT, arm);
    let mut prev = fading[fading.len() - 1];
    for x in &back {
        assert!((x - prev).abs() < 4.0, "no pop turning back mid-fade: {prev} -> {x}");
        prev = *x;
    }
    assert!(back.iter().copied().fold(0.0, f32::max) < 40.0, "{back:?}");
    assert!(back[back.len() - 1].abs() < 1.5, "settled: {back:?}");
}

#[test]
fn a_synced_blend_shares_one_phase_and_fires_its_heaviest_gaits_markers_however_frames_are_cut() {
    let m = rig();
    let arm = m.bone_named("leftArm").unwrap();
    let rising = |len: f32| clip(&m, len, true, &[("leftArm", 0.0, Vec3::ZERO), ("leftArm", len, Vec3::X * 100.0)]);
    let walk = with_marker(rising(1.0), 0.5, "step");
    let sprint = with_marker(rising(0.5), 0.25, "step");
    let build = || {
        let mut lib = ClipLibrary::new();
        lib.insert("walk", walk.clone());
        lib.insert("sprint", sprint.clone());
        animator(
            &m,
            lib,
            r#"{ "params": { "speed": 0.5 },
                 "layers": [{ "blend": "speed", "points": [[0, "walk"], [1, "sprint"]], "sync": true }] }"#,
        )
    };

    let mut a = build();
    let x = run(&mut a, 0.3, DT, arm);
    assert!((x[x.len() - 1] - 40.0).abs() < 0.5, "both gaits 40% through a 0.75 s cycle: {}", x[x.len() - 1]);

    for dt in [DT, 0.1, 1.0 / 60.0] {
        let mut a = build();
        let mut steps = 0;
        for _ in 0..(3.0 / dt).round() as usize {
            a.update(dt);
            for marker in a.markers() {
                assert_eq!(a.graph().clips().name(marker.clip), "walk");
                steps += 1;
            }
        }
        assert_eq!(steps, 4, "four cycles' footsteps at dt {dt}");
    }
}

#[test]
fn a_montage_fades_in_over_its_mask_and_finishes_fading_as_its_clip_ends() {
    let m = rig();
    let (left, right) = (m.bone_named("leftArm").unwrap(), m.bone_named("rightArm").unwrap());
    let both = |deg: f32| [("leftArm", 0.0, Vec3::X * deg), ("rightArm", 0.0, Vec3::X * deg)];
    let mut lib = ClipLibrary::new();
    lib.insert("base", clip(&m, 1.0, true, &both(10.0)));
    lib.insert("swing", clip(&m, 0.5, false, &both(90.0)));
    let mut a = animator(
        &m,
        lib,
        r#"{
        "events": ["swing"], "slots": ["action"],
        "masks": { "left": { "leftArm": 1 } },
        "layers": [
            { "clip": "base" },
            { "slot": "action", "mask": "left" },
            { "mode": "additive", "weight": 0.5, "bones": { "rightArm": { "rotation": [20, 0, 0] } } }
        ],
        "rules": [{ "on": "swing", "slot": "action", "play": "swing",
                    "fade_in": 0.1, "fade_out": 0.1, "ease": "linear" }]
    }"#,
    );
    a.fire_named("swing");
    let mut lefts = Vec::new();
    for _ in 0..60 {
        a.update(DT);
        lefts.push(a.pose().rotation(left).x);
        let r = a.pose().rotation(right).x;
        assert!((r - 20.0).abs() < 1e-4, "right arm: base plus half the additive, never the swing ({r})");
    }
    assert!((lefts[0] - 18.0).abs() < 1e-3, "{}", lefts[0]);
    assert!((lefts[19] - 90.0).abs() < 1e-3, "{}", lefts[19]);
    assert!((lefts[44] - 50.0).abs() < 1.0, "halfway out: {}", lefts[44]);
    assert!((lefts[49] - 10.0).abs() < 1.0, "the fade lands with the clip's end: {}", lefts[49]);
    assert!((lefts[55] - 10.0).abs() < 1e-4);
    assert_eq!(playing(&a, "action"), None);
}

#[test]
fn an_interrupting_montage_covers_the_one_it_cut_off_without_dipping_to_the_ground() {
    let m = rig();
    let arm = m.bone_named("leftArm").unwrap();
    let mut lib = ClipLibrary::new();
    lib.insert("base", clip(&m, 1.0, true, &[("leftArm", 0.0, Vec3::X * 10.0)]));
    lib.insert("swing", clip(&m, 0.5, false, &[("leftArm", 0.0, Vec3::X * 90.0)]));
    let mut a = animator(
        &m,
        lib,
        r#"{ "events": ["swing"], "slots": ["action"],
             "layers": [{ "clip": "base" }, { "slot": "action" }],
             "rules": [{ "on": "swing", "slot": "action", "play": "swing",
                         "fade_in": 0.3, "fade_out": 0.1, "ease": "linear" }] }"#,
    );
    a.fire_named("swing");
    run(&mut a, 0.35, DT, arm);
    a.fire_named("swing");
    // The first swing ends at 0.5 under the second's 0.3 s fade-in.
    let covered = run(&mut a, 0.4, DT, arm);
    assert!(covered.iter().all(|x| *x > 89.99), "{covered:?}");
}

#[test]
fn a_looping_segment_finishes_its_cycle_before_chaining_to_the_end() {
    let m = rig();
    let mut lib = ClipLibrary::new();
    let arm = |len: f32, looping: bool, from: f32, to: f32| {
        clip(&m, len, looping, &[("leftArm", 0.0, Vec3::X * from), ("leftArm", len, Vec3::X * to)])
    };
    lib.insert("start", arm(0.2, false, 0.0, 20.0));
    lib.insert("loop", with_marker(arm(0.3, true, 20.0, 20.0), 0.15, "hit"));
    lib.insert("end", arm(0.2, false, 20.0, 0.0));
    let mut a = animator(
        &m,
        lib,
        r#"{ "params": { "mining": 0 }, "events": ["mine"], "slots": ["action"],
             "layers": [{ "slot": "action" }],
             "rules": [{ "on": "mine", "slot": "action", "play": "start",
                         "then": [{ "clip": "loop", "while": "mining" }, "end"],
                         "fade_in": 0, "fade_out": 0 }] }"#,
    );
    let mut hits = 0;
    let mut advance = |a: &mut Animator, frames: usize| {
        for _ in 0..frames {
            a.update(DT);
            hits += a.markers().len();
        }
    };
    a.set_named("mining", 1.0);
    a.fire_named("mine");
    advance(&mut a, 85);
    assert_eq!(playing(&a, "action").as_deref(), Some("loop"), "third cycle, from 0.8 s");
    a.set_named("mining", 0.0);
    advance(&mut a, 15);
    assert_eq!(playing(&a, "action").as_deref(), Some("loop"), "finishing the cycle it is in");
    advance(&mut a, 15);
    assert_eq!(playing(&a, "action").as_deref(), Some("end"));
    advance(&mut a, 20);
    assert_eq!(playing(&a, "action"), None);
    assert_eq!(hits, 3, "one hit per cycle, the unfinished one included");
}

#[test]
fn picks_cycle_and_reset_after_a_pause_and_random_picks_never_repeat() {
    let m = rig();
    let mut lib = ClipLibrary::new();
    for name in ["a", "b", "c"] {
        lib.insert(name, clip(&m, 0.2, false, &[("leftArm", 0.0, Vec3::ZERO)]));
    }
    let mut a = animator(
        &m,
        lib,
        r#"{ "events": ["swing", "roll"], "slots": ["action"],
             "layers": [{ "slot": "action" }],
             "rules": [
                { "on": "swing", "slot": "action", "play": ["a", "b", "c"], "reset": 0.5 },
                { "on": "roll", "slot": "action", "play": ["a", "b", "c"], "pick": "random" }
             ] }"#,
    );
    let fire = |a: &mut Animator, event: &str| {
        a.fire_named(event);
        a.update(DT);
        let picked = playing(a, "action").expect("playing");
        for _ in 0..10 {
            a.update(DT);
        }
        picked
    };
    let combo: Vec<String> = (0..4).map(|_| fire(&mut a, "swing")).collect();
    assert_eq!(combo, ["a", "b", "c", "a"]);
    run(&mut a, 1.0, DT, 0);
    assert_eq!(fire(&mut a, "swing"), "a", "a pause starts the combo over");

    let rolls: Vec<String> = (0..40).map(|_| fire(&mut a, "roll")).collect();
    assert!(rolls.windows(2).all(|w| w[0] != w[1]), "{rolls:?}");
    for name in ["a", "b", "c"] {
        assert!(rolls.iter().any(|r| r == name), "{name} never picked: {rolls:?}");
    }
}

#[test]
fn a_mirrored_montage_plays_on_the_partner_bones_reflected() {
    let m = rig();
    let (left, right) = (m.bone_named("leftArm").unwrap(), m.bone_named("rightArm").unwrap());
    let mut raise = clip(&m, 1.0, false, &[("leftArm", 0.0, Vec3::new(30.0, 20.0, 10.0))]);
    raise.set_track(
        left,
        Channel::Position,
        vec![Keyframe::new(0.0, Vec3::new(1.0, 2.0, 3.0), Interpolation::Linear)],
    );
    let mut lib = ClipLibrary::new();
    lib.insert("raise", raise);
    let mut a = animator(
        &m,
        lib,
        r#"{ "params": { "offhand": 0 }, "events": ["use"], "slots": ["action"],
             "layers": [{ "slot": "action" }],
             "rules": [{ "on": "use", "slot": "action", "play": "raise",
                         "mirror": "offhand", "fade_in": 0 }] }"#,
    );
    a.set_named("offhand", 1.0);
    a.fire_named("use");
    a.update(DT);
    let pose = a.pose();
    assert_eq!(pose.rotation(right), Vec3::new(30.0, -20.0, -10.0));
    assert_eq!(pose.position(right), Vec3::new(-1.0, 2.0, 3.0));
    assert_eq!(pose.rotation(left), Vec3::ZERO);
    assert_eq!(pose.position(left), Vec3::ZERO);
}

#[test]
fn hitstop_freezes_a_slots_clip_then_lets_it_run_on() {
    let m = rig();
    let arm = m.bone_named("leftArm").unwrap();
    let mut lib = ClipLibrary::new();
    lib.insert(
        "swing",
        clip(&m, 1.0, false, &[("leftArm", 0.0, Vec3::ZERO), ("leftArm", 1.0, Vec3::X * 100.0)]),
    );
    let mut a = animator(
        &m,
        lib,
        r#"{ "events": ["swing", "hit"], "slots": ["action"],
             "layers": [{ "slot": "action" }],
             "rules": [
                { "on": "swing", "slot": "action", "play": "swing", "fade_in": 0, "fade_out": 0 },
                { "on": "hit", "slot": "action", "hitstop": 0.1 }
             ] }"#,
    );
    a.fire_named("swing");
    assert!((run(&mut a, 0.3, DT, arm)[29] - 30.0).abs() < 1e-3);
    a.fire_named("hit");
    assert!((run(&mut a, 0.1, DT, arm)[9] - 30.0).abs() < 1e-3, "frozen on the hit");
    let after = run(&mut a, 0.1, DT, arm)[9];
    assert!((after - 40.0).abs() <= 1.01, "runs on from where it froze: {after}");
}

#[test]
fn an_inertial_montage_starts_from_the_old_pose_and_settles_without_overshoot() {
    let m = rig();
    let arm = m.bone_named("leftArm").unwrap();
    let mut lib = ClipLibrary::new();
    lib.insert("base", clip(&m, 1.0, true, &[("leftArm", 0.0, Vec3::X * 10.0)]));
    lib.insert("hold", clip(&m, 2.0, false, &[("leftArm", 0.0, Vec3::X * 90.0)]));
    let mut a = animator(
        &m,
        lib,
        r#"{ "slots": ["action"], "layers": [{ "clip": "base" }, { "slot": "action" }] }"#,
    );
    run(&mut a, 0.1, DT, arm);
    let mut spec = PlaySpec::new(a.graph().clips().id("hold").unwrap());
    spec.inertial = true;
    spec.fade_in = 0.2;
    assert!(a.play(a.graph().slot("action").unwrap(), &spec).is_some());
    let x = run(&mut a, 0.3, DT, arm);
    assert!((x[0] - 10.0).abs() < 1e-3, "continuous at the cut: {}", x[0]);
    assert!(x.windows(2).all(|w| w[1] >= w[0] - 1e-4), "{x:?}");
    assert!(x.iter().all(|v| *v <= 90.0 + 1e-3), "{x:?}");
    assert!(x[24] > 87.0, "within 5% soon after its settle time: {}", x[24]);
}

#[test]
fn a_ground_pose_shows_through_everything_the_graph_does_not_own() {
    let m = rig();
    let (left, right) = (m.bone_named("leftArm").unwrap(), m.bone_named("rightArm").unwrap());
    let mut lib = ClipLibrary::new();
    lib.insert("raise", clip(&m, 1.0, false, &[("leftArm", 0.0, Vec3::X * 90.0)]));
    let mut a = animator(
        &m,
        lib,
        r#"{ "events": ["use"], "slots": ["action"],
             "layers": [
                { "slot": "action", "mask": { "leftArm": 1 } },
                { "mode": "additive", "bones": { "rightArm": { "rotation": [0, 5, 0] } } }
             ],
             "rules": [{ "on": "use", "slot": "action", "play": "raise", "fade_in": 0 }] }"#,
    );
    let mut walk = LocalPose::rest(m.bones().len());
    walk.add_bone(left, Vec3::X * 10.0, Vec3::ZERO);
    walk.add_bone(right, Vec3::X * 10.0, Vec3::Y);
    a.update_over(DT, &walk);
    assert_eq!(a.pose().rotation(left), Vec3::X * 10.0, "nothing playing: the ground");
    a.fire_named("use");
    a.update_over(DT, &walk);
    assert_eq!(a.pose().rotation(left), Vec3::X * 90.0, "the montage owns its masked channel");
    assert_eq!(a.pose().rotation(right), Vec3::new(10.0, 5.0, 0.0), "additive layers add onto the ground");
    assert_eq!(a.pose().position(right), Vec3::Y);
}

#[test]
fn a_scrubbed_montage_samples_where_its_caller_puts_it_and_fires_what_it_passes() {
    let m = rig();
    let arm = m.bone_named("leftArm").unwrap();
    let mut lib = ClipLibrary::new();
    let swing = with_marker(
        clip(&m, 1.0, false, &[("leftArm", 0.0, Vec3::ZERO), ("leftArm", 1.0, Vec3::X * 100.0)]),
        0.5,
        "impact",
    );
    let id = lib.insert("swing", swing);
    let mut a = animator(&m, lib, r#"{ "slots": ["action"], "layers": [{ "slot": "action" }] }"#);
    let slot = a.graph().slot("action").unwrap();
    let mut spec = PlaySpec::new(id);
    spec.rate = 0.0;
    spec.fade_in = 0.0;
    let play = a.play(slot, &spec).expect("an empty slot admits it");
    for (seconds, want, fired) in [(0.2, 20.0, 0), (0.6, 60.0, 1), (0.3, 30.0, 0), (0.9, 90.0, 1)] {
        a.seek(slot, play, seconds);
        a.update(DT);
        assert!((a.pose().rotation(arm).x - want).abs() < 1e-3, "at {seconds}: {}", a.pose().rotation(arm).x);
        assert_eq!(a.markers().len(), fired, "at {seconds}");
    }
}

#[test]
fn a_rule_the_slot_refuses_keeps_its_cooldown_and_its_pick() {
    let m = rig();
    let mut lib = ClipLibrary::new();
    for name in ["guard", "a", "b", "flinch"] {
        lib.insert(name, clip(&m, 1.0, false, &[("leftArm", 0.0, Vec3::ZERO)]));
    }
    let mut a = animator(
        &m,
        lib,
        r#"{ "events": ["block", "swing"], "slots": ["action", "other"],
             "layers": [{ "slot": "action" }, { "slot": "other" }],
             "rules": [
                { "on": "block", "slot": "action", "play": "guard", "priority": 5 },
                { "on": "swing", "slot": "action", "play": ["a", "b"], "cooldown": 10, "reset": 100 },
                { "on": "swing", "slot": "other", "play": "flinch" }
             ] }"#,
    );
    let slot = a.graph().slot("action").unwrap();
    a.fire_named("block");
    a.update(DT);
    a.fire_named("swing");
    a.update(DT);
    assert_eq!(playing(&a, "action").as_deref(), Some("guard"), "refused by priority");
    assert_eq!(playing(&a, "other").as_deref(), Some("flinch"), "a refused rule is no match: the next fires");
    a.stop(slot, 0.0);
    a.update(DT);
    a.fire_named("swing");
    a.update(DT);
    assert_eq!(playing(&a, "action").as_deref(), Some("a"), "neither the cooldown nor the combo moved");
}

#[test]
fn an_event_fired_twice_before_an_update_runs_its_rules_once() {
    let m = rig();
    let mut lib = ClipLibrary::new();
    for name in ["a", "b", "c"] {
        lib.insert(name, clip(&m, 1.0, false, &[("leftArm", 0.0, Vec3::ZERO)]));
    }
    let mut a = animator(
        &m,
        lib,
        r#"{ "events": ["swing"], "slots": ["action"],
             "layers": [{ "slot": "action" }],
             "rules": [{ "on": "swing", "slot": "action", "play": ["a", "b", "c"], "reset": 100 }] }"#,
    );
    a.fire_named("swing");
    a.fire_named("swing");
    a.update(DT);
    assert_eq!(playing(&a, "action").as_deref(), Some("a"));
    a.fire_named("swing");
    a.update(DT);
    assert_eq!(playing(&a, "action").as_deref(), Some("b"), "the double fire advanced the combo once");
}

/// A handle addresses its own montage and nothing else: once a rule cuts in
/// over it the handle reads displaced, and seeking it moves nothing — a
/// scrubbing caller can never drive the rule's clip.
#[test]
fn a_seek_moves_only_the_play_it_names_and_a_cut_off_play_reads_displaced() {
    let m = rig();
    let arm = m.bone_named("leftArm").unwrap();
    let mut lib = ClipLibrary::new();
    let rising = |deg: f32| clip(&m, 1.0, false, &[("leftArm", 0.0, Vec3::ZERO), ("leftArm", 1.0, Vec3::X * deg)]);
    let held = lib.insert("held", rising(100.0));
    lib.insert("swing", rising(10.0));
    let mut a = animator(
        &m,
        lib,
        r#"{ "events": ["swing"], "slots": ["action"], "layers": [{ "slot": "action" }],
             "rules": [{ "on": "swing", "slot": "action", "play": "swing",
                         "blend": "inertial", "fade_in": 0, "fade_out": 0 }] }"#,
    );
    let slot = a.graph().slot("action").unwrap();
    let mut spec = PlaySpec::new(held);
    spec.rate = 0.0;
    spec.looping = true;
    spec.inertial = true;
    spec.fade_in = 0.0;
    let play = a.play(slot, &spec).expect("an empty slot admits it");
    a.seek(slot, play, 0.3);
    a.update(DT);
    assert!((a.pose().rotation(arm).x - 30.0).abs() < 1e-3);
    assert_eq!(a.play_state(slot, play), PlayState::Playing);

    a.fire_named("swing");
    a.update(DT);
    assert_eq!(a.play_state(slot, play), PlayState::Displaced);
    let before = a.playing(slot).expect("the rule's swing").time;
    a.seek(slot, play, 0.9);
    a.update(DT);
    assert_eq!(playing(&a, "action").as_deref(), Some("swing"));
    let after = a.playing(slot).unwrap().time;
    assert!((after - before - DT).abs() < 1e-4, "the swing ran on at its own rate: {before} -> {after}");
}

/// A montage that plays to its end reads finished, not displaced, after it
/// has left; stopping one play leaves the montage over it playing.
#[test]
fn a_play_that_ends_reads_finished_and_stopping_one_play_spares_the_other() {
    let m = rig();
    let mut lib = ClipLibrary::new();
    let short = lib.insert("short", clip(&m, 0.05, false, &[("leftArm", 0.0, Vec3::ZERO)]));
    let long = lib.insert("long", clip(&m, 1.0, false, &[("leftArm", 0.0, Vec3::X * 5.0)]));
    let mut a = animator(&m, lib, r#"{ "slots": ["action"], "layers": [{ "slot": "action" }] }"#);
    let slot = a.graph().slot("action").unwrap();
    let quick = |clip| {
        let mut spec = PlaySpec::new(clip);
        spec.fade_in = 0.0;
        spec.fade_out = 0.0;
        spec
    };
    let ended = a.play(slot, &quick(short)).unwrap();
    run(&mut a, 0.1, DT, 0);
    assert_eq!(a.play_state(slot, ended), PlayState::Finished);

    let under = a.play(slot, &quick(long)).unwrap();
    let over = a.play(slot, &quick(long)).unwrap();
    a.stop_play(slot, under, 0.0);
    a.update(DT);
    assert_eq!(a.play_state(slot, over), PlayState::Playing);
    assert_eq!(a.play_state(slot, under), PlayState::Displaced);
}

/// A gate stands down, in one place, every rule on its events that plays
/// into its slot — and nothing else: a rule on the same event into another
/// slot still plays.
#[test]
fn a_gate_stands_down_the_rules_it_names_and_no_other() {
    let m = rig();
    let mut lib = ClipLibrary::new();
    for name in ["swing", "chop", "flash"] {
        lib.insert(name, clip(&m, 1.0, false, &[("leftArm", 0.0, Vec3::ZERO)]));
    }
    let mut a = animator(
        &m,
        lib,
        r#"{ "params": { "claimed": 0 }, "events": ["swing", "break"], "slots": ["action", "feedback"],
             "layers": [{ "slot": "action" }, { "slot": "feedback" }],
             "gates": [{ "id": "claim", "on": ["swing", "break"], "slot": "action", "when": "!claimed" }],
             "rules": [
                { "on": "swing", "slot": "action", "play": "swing" },
                { "on": "break", "slot": "feedback", "play": "flash", "fallthrough": true },
                { "on": "break", "slot": "action", "play": "chop" }
             ] }"#,
    );
    a.set_named("claimed", 1.0);
    a.fire_named("swing");
    a.fire_named("break");
    a.update(DT);
    assert_eq!(playing(&a, "action"), None, "both gated rules stand down");
    assert_eq!(playing(&a, "feedback").as_deref(), Some("flash"), "the other slot's rule plays");

    a.set_named("claimed", 0.0);
    a.fire_named("break");
    a.update(DT);
    assert_eq!(playing(&a, "action").as_deref(), Some("chop"));
}

/// A clip template picks its clip by its param's value when the rule fires;
/// a value naming no clip is no match, so the next rule gets the event.
#[test]
fn a_clip_template_plays_what_its_param_names_and_falls_through_where_nothing_is_named() {
    let m = rig();
    let mut lib = ClipLibrary::new();
    for name in ["swing_axe", "swing_pick", "punch"] {
        lib.insert(name, clip(&m, 1.0, false, &[("leftArm", 0.0, Vec3::ZERO)]));
    }
    let mut a = animator(
        &m,
        lib,
        r#"{ "params": { "tool": "none" }, "events": ["swing"], "slots": ["action"],
             "layers": [{ "slot": "action" }],
             "rules": [
                { "on": "swing", "slot": "action", "play": "swing_{tool}" },
                { "on": "swing", "slot": "action", "play": "punch" }
             ] }"#,
    );
    for (tool, plays) in [("axe", "swing_axe"), ("spoon", "punch"), ("pick", "swing_pick")] {
        a.set_named("tool", intern(tool));
        a.fire_named("swing");
        a.update(DT);
        assert_eq!(playing(&a, "action").as_deref(), Some(plays), "holding {tool}");
    }
}

/// A marker the graph maps to an event fires it on the update after a clip
/// crosses it, and a mirrored clip reads its marker under the other side's
/// name.
#[test]
fn a_mapped_marker_fires_its_event_next_update_and_a_mirrored_clip_swaps_its_side() {
    let m = rig();
    let mut lib = ClipLibrary::new();
    let still = |len: f32| clip(&m, len, false, &[("leftArm", 0.0, Vec3::ZERO)]);
    lib.insert("swing", with_marker(still(0.5), 0.05, "impact"));
    lib.insert("step", with_marker(still(0.5), 0.05, "left_foot"));
    for name in ["flash", "left", "right"] {
        lib.insert(name, still(1.0));
    }
    let mut a = animator(
        &m,
        lib,
        r#"{ "params": { "off": 0 }, "events": ["swing", "stride", "strike", "left", "right"],
             "slots": ["action", "hit"],
             "markers": { "impact": "strike", "left_foot": "left", "right_foot": "right" },
             "layers": [{ "slot": "action" }, { "slot": "hit" }],
             "rules": [
                { "on": "swing", "slot": "action", "play": "swing", "fade_in": 0 },
                { "on": "stride", "slot": "action", "play": "step", "mirror": "off", "fade_in": 0 },
                { "on": "strike", "slot": "hit", "play": "flash" },
                { "on": "left", "slot": "hit", "play": "left" },
                { "on": "right", "slot": "hit", "play": "right" }
             ] }"#,
    );
    let cross = |a: &mut Animator, event: &str| {
        a.fire_named(event);
        for _ in 0..100 {
            a.update(DT);
            if !a.markers().is_empty() {
                return;
            }
        }
        panic!("{event}: no marker crossed");
    };
    cross(&mut a, "swing");
    assert_eq!(playing(&a, "hit"), None, "not on the crossing update");
    a.update(DT);
    assert_eq!(playing(&a, "hit").as_deref(), Some("flash"));

    a.set_named("off", 1.0);
    cross(&mut a, "stride");
    a.update(DT);
    assert_eq!(playing(&a, "hit").as_deref(), Some("right"), "a mirrored left foot is the right");
}

/// A frame nobody draws still runs the graph's rules and montages — a swing
/// fired then is under way when the body is next posed — without evaluating
/// a pose.
#[test]
fn advancing_unposed_runs_rules_and_montages_and_leaves_the_pose() {
    let m = rig();
    let arm = m.bone_named("leftArm").unwrap();
    let mut lib = ClipLibrary::new();
    lib.insert(
        "swing",
        clip(&m, 1.0, false, &[("leftArm", 0.0, Vec3::ZERO), ("leftArm", 1.0, Vec3::X * 100.0)]),
    );
    let mut a = animator(
        &m,
        lib,
        r#"{ "events": ["swing"], "slots": ["action"], "layers": [{ "slot": "action" }],
             "rules": [{ "on": "swing", "slot": "action", "play": "swing", "fade_in": 0 }] }"#,
    );
    a.fire_named("swing");
    for _ in 0..30 {
        a.advance(DT);
    }
    assert_eq!(a.pose().rotation(arm), Vec3::ZERO, "nothing posed");
    let slot = a.graph().slot("action").unwrap();
    assert!((a.playing(slot).unwrap().time - 0.3).abs() < 1e-3);
    a.update(DT);
    assert!((a.pose().rotation(arm).x - 31.0).abs() < 1e-2, "{}", a.pose().rotation(arm).x);
}
