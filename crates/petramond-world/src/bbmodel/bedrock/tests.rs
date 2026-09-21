use glam::Vec3;

use super::parse_library;
use crate::bbmodel::{Channel, Interpolation, MarkerKind};

const LIBRARY: &str = r#"{ "format_version": "1.8.0", "animations": {
    "clip.a": { "loop": true, "animation_length": 2.0,
        "bones": {
            "Arm": {
                "rotation": {
                    "0.0": [10, 20, 30],
                    "1.0": { "post": [1, 2, 3], "lerp_mode": "catmullrom" },
                    "1.5": { "pre": [1, 2, 3], "post": [7, 8, 9] },
                    "2.0": [7, 8, 9]
                },
                "position": [4, 5, 6]
            },
            "Missing": { "rotation": [1, 1, 1] }
        },
        "sound_effects": { "0.25": { "effect": "petramond:swing" } },
        "timeline": { "0.5": "impact;\nshake;" }
    },
    "clip.b": { "loop": "hold_on_last_frame", "bones": { "arm": { "rotation": 5 } } }
} }"#;

fn arm(name: &str) -> Option<usize> {
    (name == "arm").then_some(0)
}

/// Blockbench's importer, reproduced: exported arrays flip back into
/// Blockbench's axes (rotation X/Y, position X) while a uniform number does
/// not, a key whose leaving value is the next key's arriving value becomes a
/// step (its `lerp_mode` notwithstanding), bone names resolve verbatim then
/// lowercased, unknown bones drop, and effect tracks become markers.
#[test]
fn a_library_reads_back_into_blockbench_axes_with_steps_modes_and_markers() {
    let lib = parse_library(LIBRARY, arm).expect("a valid library");
    let a = &lib.iter().find(|(n, _)| n == "clip.a").expect("clip.a").1;
    assert!(a.looping && !a.hold);
    assert_eq!(a.length, 2.0);
    let rot = |t| {
        a.sample(0, Channel::Rotation, t)
            .expect("arm rotation keyed")
    };
    assert_eq!(rot(0.0), Vec3::new(-10.0, -20.0, 30.0));
    assert_eq!(rot(1.25), Vec3::new(-1.0, -2.0, 3.0), "the step holds");
    assert_eq!(rot(1.5), Vec3::new(-7.0, -8.0, 9.0));
    assert_eq!(
        a.sample(0, Channel::Position, 0.0),
        Some(Vec3::new(-4.0, 5.0, 6.0))
    );
    let markers: Vec<_> = a
        .markers()
        .iter()
        .map(|m| (m.time, m.kind, m.name.as_str()))
        .collect();
    assert_eq!(
        markers,
        [
            (0.25, MarkerKind::Sound, "petramond:swing"),
            (0.5, MarkerKind::Timeline, "impact"),
            (0.5, MarkerKind::Timeline, "shake"),
        ]
    );

    let b = &lib.iter().find(|(n, _)| n == "clip.b").expect("clip.b").1;
    assert!(b.hold && !b.looping);
    assert_eq!(
        b.sample(0, Channel::Rotation, 0.0),
        Some(Vec3::splat(5.0)),
        "a uniform value is not flipped"
    );
}

#[test]
fn a_document_without_animations_is_refused() {
    assert!(parse_library("{}", arm).is_err());
    assert!(parse_library("{", arm).is_err());
}

/// Only the pair right before the last key decides whether it steps: an
/// earlier step in the track never leaks onto the end.
#[test]
fn the_last_key_steps_only_when_the_pair_before_it_does() {
    let track = |keys: &str| {
        let text = format!(
            r#"{{ "animations": {{ "c": {{ "bones": {{ "arm": {{ "rotation": {{ {keys} }} }} }} }} }} }}"#
        );
        let lib = parse_library(&text, arm).expect("parses");
        let modes: Vec<Interpolation> = lib[0].1.tracks()[0]
            .rotation
            .iter()
            .map(|k| k.interpolation)
            .collect();
        modes
    };
    use Interpolation::{Linear, Step};
    assert_eq!(
        track(
            r#""0.0": [0, 0, 0], "1.0": { "pre": [0, 0, 0], "post": [2, 2, 2] }, "2.0": [4, 4, 4]"#
        ),
        [Step, Linear, Linear],
        "the step is between the first two keys only"
    );
    assert_eq!(
        track(
            r#""0.0": [0, 0, 0], "1.0": [2, 2, 2], "2.0": { "pre": [2, 2, 2], "post": [4, 4, 4] }"#
        ),
        [Linear, Step, Step],
        "the pair before the last key steps, so the last key does too"
    );
}
