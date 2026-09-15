use glam::Vec3;

use super::Graph;
use crate::animation::library::ClipLibrary;
use crate::animation::test_rig::{clip, rig};
use crate::bbmodel::Model;

fn library(rig: &Model) -> ClipLibrary {
    let mut lib = ClipLibrary::new();
    lib.insert("walk", clip(rig, 1.0, true, &[("leftArm", 0.0, Vec3::ZERO)]));
    lib
}

#[test]
fn a_bad_graph_is_refused_naming_what_is_wrong() {
    let m = rig();
    for (source, says) in [
        (r#"{ "layers": [{ "clip": "wlak" }] }"#, "no clip named `wlak`"),
        (
            r#"{ "layers": [{ "clip": "walk", "speed": 1 }] }"#,
            "`speed` is not a key of a clip node",
        ),
        (
            r#"{ "params": { "speed": 0 }, "layers": [{ "clip": "walk", "rate": "sped * 2" }] }"#,
            "sped",
        ),
        (
            r#"{ "layers": [{ "clip": "walk", "mask": { "tail": 1 } }] }"#,
            "no bone `tail`",
        ),
        (
            r#"{ "layers": [{ "states": { "idle": "walk" }, "initial": "idle",
                 "transitions": [{ "from": "idle", "to": "run", "when": 1 }] }] }"#,
            "no state `run`",
        ),
        (
            r#"{ "slots": ["action"], "layers": [{ "slot": "action" }, { "slot": "action" }] }"#,
            "already plays",
        ),
        (
            r#"{ "slots": ["action"], "layers": [],
                 "rules": [{ "on": "swing", "slot": "action", "play": "walk" }] }"#,
            "`swing` is not a declared event",
        ),
        (
            r#"{ "params": { "tool": 0 }, "events": ["swing"], "slots": ["action"], "layers": [],
                 "rules": [{ "on": "swing", "slot": "action", "play": "run_{tool}" }] }"#,
            "no clip matches `run_{tool}`",
        ),
        (
            r#"{ "events": ["swing"], "slots": ["action"], "layers": [],
                 "rules": [{ "on": "swing", "slot": "action", "play": "wa{speed}" }] }"#,
            "names no declared param `speed`",
        ),
        (
            r#"{ "params": { "a": 0, "b": 0 }, "events": ["swing"], "slots": ["action"], "layers": [],
                 "rules": [{ "on": "swing", "slot": "action", "play": "{a}_{b}" }] }"#,
            "names one param",
        ),
        (
            r#"{ "slots": ["action"], "layers": [], "gates": [{ "on": "swing", "when": 1 }] }"#,
            "gates[0].on: `swing` is not a declared event",
        ),
        (
            r#"{ "events": ["swing"], "layers": [], "gates": [{ "id": "g", "on": "swing" }] }"#,
            "gates[g]: a gate needs `when`",
        ),
        (
            r#"{ "layers": [], "markers": { "impact": "strike" } }"#,
            "markers.impact: `strike` is not a declared event",
        ),
    ] {
        match Graph::compile(source, &m, library(&m)) {
            Ok(_) => panic!("compiled: {source}"),
            Err(e) => assert!(e.contains(says), "{source}\n  said: {e}"),
        }
    }
}

#[test]
fn a_mask_weights_a_bone_with_its_descendants_unless_they_carry_their_own() {
    let m = rig();
    let g = Graph::compile(
        r#"{ "masks": { "arm": { "leftArm": 1, "leftHand": 0.25 } },
             "layers": [{ "clip": "walk", "mask": "arm" }] }"#,
        &m,
        library(&m),
    )
    .unwrap_or_else(|e| panic!("{e}"));
    let weight = |bone: &str| g.masks[0][m.bone_named(bone).expect(bone)];
    assert_eq!(weight("root"), 0.0);
    assert_eq!(weight("leftArm"), 1.0);
    assert_eq!(weight("leftHand"), 0.25);
    assert_eq!(weight("leftFinger"), 0.25, "inherits the nearest listed ancestor");
    assert_eq!(weight("rightHand"), 0.0);
}
