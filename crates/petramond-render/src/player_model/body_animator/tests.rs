use std::sync::Arc;

use glam::Vec3;
use petramond::player::RigId;
use petramond_world::animation::library::ClipLibrary;
use petramond_world::animation::test_rig::{clip, rig};
use petramond_world::animation::{Graph, PlaySpec};

use super::{BodyAnimators, LOCAL_BODY};

#[test]
fn a_body_keeps_its_animator_while_it_stays_on_the_roster_drawn_or_not() {
    let m = rig();
    let mut lib = ClipLibrary::new();
    lib.insert(
        "a",
        clip(&m, 1.0, true, &[("leftArm", 0.0, Vec3::X * 10.0)]),
    );
    let graph = Graph::compile(
        r#"{ "slots": ["s"], "layers": [{ "slot": "s" }] }"#,
        &m,
        lib,
    )
    .unwrap_or_else(|e| panic!("{e}"));
    let slot = graph.slot("s").unwrap();
    let clip = graph.clips().id("a").unwrap();
    let mut bodies = BodyAnimators::new(Some((RigId(1), Arc::new(graph))));
    for key in [7, 9, LOCAL_BODY] {
        let animator = &mut bodies.body(key).unwrap().driver.animator;
        assert!(animator.play(slot, &PlaySpec::new(clip)).is_some());
    }

    // Two frames that draw no body, with only player 7 still connected.
    bodies.retain([7]);
    bodies.retain([7]);
    let playing = |bodies: &BodyAnimators, key| {
        bodies
            .bodies
            .get(&key)
            .map(|body| body.driver.animator.playing(slot).is_some())
    };
    assert_eq!(
        playing(&bodies, 7),
        Some(true),
        "an undrawn body keeps what it plays"
    );
    assert_eq!(
        playing(&bodies, LOCAL_BODY),
        Some(true),
        "the local body is always on the roster"
    );
    assert_eq!(
        playing(&bodies, 9),
        None,
        "a body off the roster loses its animator"
    );
}
