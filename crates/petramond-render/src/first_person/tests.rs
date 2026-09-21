use std::sync::Arc;

use glam::Vec3;
use petramond::player::RigId;
use petramond_world::animation::library::ClipLibrary;
use petramond_world::animation::test_rig::{clip, rig, with_marker};
use petramond_world::animation::Graph;

use super::driver::Driver;
use crate::views::{AimTarget, LocalMotion};
use crate::{AnimatorInputs, HeldItemFrame};

const RIG: RigId = RigId(2);
const DT: f32 = 1.0 / 60.0;

#[test]
fn the_shipped_viewmodel_rig_and_graph_load() {
    assert!(super::FirstPersonHand::shipped().is_some());
}

/// A driver over `source` on the synthetic rig, with a half-second `swing`
/// whose `impact` marker passes at 0.2 s and a looping `dig`.
fn driver(source: &str) -> Driver {
    let m = rig();
    let rising = |len: f32, deg: f32| {
        clip(
            &m,
            len,
            false,
            &[
                ("leftArm", 0.0, Vec3::ZERO),
                ("leftArm", len, Vec3::X * deg),
            ],
        )
    };
    let mut lib = ClipLibrary::new();
    lib.insert("swing", with_marker(rising(0.5, 90.0), 0.2, "impact"));
    lib.insert("dig", rising(0.5, 45.0));
    let graph = Graph::compile(source, &m, lib).unwrap_or_else(|e| panic!("{e}"));
    Driver::new(RIG, Arc::new(graph))
}

fn hands(mining: bool) -> [HeldItemFrame; 2] {
    let frame = HeldItemFrame::default();
    [HeldItemFrame { mining, ..frame }, frame]
}

/// The row an engine gesture or a mod's fire on this rig resolves to.
fn fired(driver: &Driver, event: &str) -> [(RigId, u16); 1] {
    [(
        RIG,
        driver.animator().graph().event(event).unwrap().index() as u16,
    )]
}

fn standing(target: AimTarget) -> LocalMotion {
    LocalMotion {
        grounded: true,
        target,
        ..Default::default()
    }
}

/// The graph's `markers` table fires `strike` once a playing clip's `impact`
/// marker passes; what a strike does is the graph's rule, here a hit-stop
/// that only a strike landing on something — the driver's `target` — takes.
#[test]
fn a_strike_that_lands_holds_the_swing_for_the_graphs_hitstop_and_a_whiff_does_not() {
    let frozen_frames = |target| {
        let mut d = driver(
            r#"{ "params": { "target": 0 }, "events": ["main.swing", "strike"], "slots": ["main"],
                 "markers": { "impact": "strike" },
                 "layers": [{ "slot": "main" }],
                 "rules": [
                    { "on": "main.swing", "slot": "main", "play": "swing", "fade_in": 0, "fade_out": 0 },
                    { "on": "strike", "when": "target > 0.5", "slot": "main", "hitstop": 0.1 }
                 ] }"#,
        );
        let slot = d.animator().graph().slot("main").unwrap();
        let swing = fired(&d, "main.swing");
        d.update(
            &hands(false),
            &standing(target),
            AnimatorInputs {
                events: &swing,
                ..Default::default()
            },
            DT,
        );
        let (mut last, mut frozen) = (None, 0);
        for _ in 0..40 {
            d.update(
                &hands(false),
                &standing(target),
                AnimatorInputs::default(),
                DT,
            );
            let time = d.animator().playing(slot).map(|p| p.time);
            if time.is_some() && time == last {
                frozen += 1;
            }
            last = time;
        }
        frozen
    };
    assert_eq!(frozen_frames(AimTarget::Nothing), 0, "a whiff never pauses");
    assert!(
        frozen_frames(AimTarget::Creature) > 0,
        "a landed strike holds the swing"
    );
}

/// A click on a block fires the swing and starts the dig, in either order
/// across frames. The hand publishes the mining level in the same frame as
/// the fired gesture, so a rule can refuse the swing while digging and the
/// dig loop keeps playing for as long as the dig holds.
#[test]
fn a_click_that_starts_mining_keeps_the_dig_loop_over_the_swing() {
    for mining_on_click in [false, true] {
        let mut d = driver(
            r#"{ "params": { "main.mining": 0 }, "events": ["main.swing", "main.mine"], "slots": ["main"],
                 "layers": [{ "slot": "main" }],
                 "rules": [
                    { "on": "main.mine", "slot": "main", "priority": 2,
                      "play": { "clip": "dig", "while": "main.mining" } },
                    { "on": "main.swing", "when": "!main.mining", "slot": "main", "priority": 2,
                      "play": "swing" }
                 ] }"#,
        );
        let slot = d.animator().graph().slot("main").unwrap();
        let motion = standing(AimTarget::Block);
        let swing = fired(&d, "main.swing");
        d.update(
            &hands(mining_on_click),
            &motion,
            AnimatorInputs {
                events: &swing,
                ..Default::default()
            },
            DT,
        );
        for _ in 0..90 {
            d.update(&hands(true), &motion, AnimatorInputs::default(), DT);
        }
        let playing = d.animator().playing(slot).expect("the dig is animating");
        assert_eq!(
            d.animator().graph().clips().name(playing.clip),
            "dig",
            "mining on the click: {mining_on_click}"
        );
    }
}
