use std::path::{Path, PathBuf};

use super::{animator_layers, compile_animator, compile_layers, AnimatorSource};

/// Every shipped rig's animator document compiles against its model with
/// EVERY layer — the loader leaves a broken layer out at runtime, so this is
/// where a broken shipped layer fails loudly instead.
#[test]
fn shipped_player_animators_compile_against_their_rigs() {
    let rigs = crate::player::rigs::all();
    assert!(!rigs.is_empty(), "the rigs catalog loads");
    for rig in rigs {
        assert!(
            !rig.model.bones().is_empty(),
            "{}: the model loads",
            rig.name
        );
        let layers = animator_layers(&rig.animator);
        assert!(!layers.is_empty(), "{} ships", rig.animator);
        compile_animator(&layers, &rig.model, |path| std::fs::read(path).ok())
            .unwrap_or_else(|e| panic!("{}: {e}", rig.animator));
    }
}

/// A layer whose libraries resolve under `<namespace>/`; the engine's own is
/// the `petramond` one.
fn source(text: &str, namespace: &str) -> AnimatorSource {
    AnimatorSource {
        text: text.to_string(),
        namespace: namespace.to_string(),
        origin: namespace.to_string(),
        dir: PathBuf::from(namespace),
        engine: namespace == "petramond",
    }
}

const JAB_LIBRARY: &str = r#"{ "format_version": "1.8.0", "animations": { "jab": {
    "animation_length": 0.5, "bones": { "leftArm": { "rotation": { "0.0": [0, 0, 0], "0.5": [90, 0, 0] } } }
} } }"#;

/// root → leftArm, with one authored clip `wave` on the arm.
fn rig() -> petramond_world::bbmodel::Model {
    petramond_world::bbmodel::Model::load(
        r#"{
        "resolution": { "width": 16, "height": 16 },
        "textures": [{ "uv_width": 16, "uv_height": 16,
            "source": "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==" }],
        "elements": [],
        "groups": [
            { "uuid": "root", "name": "root", "origin": [0, 0, 0] },
            { "uuid": "la", "name": "leftArm", "origin": [5, 22, 0] }
        ],
        "outliner": [{ "uuid": "root", "children": [{ "uuid": "la", "children": [] }] }],
        "animations": [{ "name": "wave", "length": 0.5, "loop": "once", "animators": { "la": {
            "name": "leftArm", "type": "bone", "keyframes": [
                { "channel": "rotation", "time": 0, "interpolation": "linear", "data_points": [{ "x": 0, "y": 0, "z": 0 }] },
                { "channel": "rotation", "time": 0.5, "interpolation": "linear", "data_points": [{ "x": 40, "y": 0, "z": 0 }] }
            ] } } }]
    }"#,
    )
    .expect("test rig parses")
}

/// A pack's copy of the document ADDS to the engine's rather than replacing
/// it: a rule keyed like an engine rule replaces that one in place, a new
/// rule slots before a named one, `"enabled": false` removes one, params and
/// events union, a gate joins by id, and a library's clips take the pack's
/// namespace. Getting the merge wrong means a pack that adds a spear has to
/// restate every engine rule — and silently loses each one the engine
/// changes afterwards.
#[test]
fn a_pack_layer_merges_into_the_engine_document_by_key() {
    let rig = rig();
    let base = r#"{
        "params": { "held": 0 },
        "events": ["swing"],
        "slots": ["main"],
        "layers": [
            { "name": "action", "slot": "main" }
        ],
        "rules": [
            { "id": "punch", "on": "swing", "slot": "main", "play": "petramond:wave" },
            { "id": "gone", "on": "swing", "slot": "main", "play": "petramond:wave" }
        ]
    }"#;
    let pack = r#"{
        "libraries": ["spear.animation.json"],
        "params": { "spear": 0 },
        "events": ["thrust"],
        "gates": [{ "id": "spear_hand", "on": "swing", "slot": "main", "when": "!spear" }],
        "rules": [
            { "id": "spear", "before": "punch", "on": "thrust", "slot": "main", "play": "spears:jab" },
            { "id": "punch", "on": "swing", "slot": "main", "play": "petramond:wave" },
            { "id": "gone", "enabled": false }
        ]
    }"#;
    let graph = compile_animator(
        &[source(base, "petramond"), source(pack, "spears")],
        &rig,
        |path| {
            (path == Path::new("spears/spear.animation.json"))
                .then(|| JAB_LIBRARY.as_bytes().to_vec())
        },
    )
    .expect("the merged document compiles");
    assert!(graph.param("spear").is_some() && graph.param("held").is_some());
    assert_eq!(graph.event_names(), ["swing", "thrust"]);
    assert!(
        graph.clips().id("spears:jab").is_some(),
        "library clips take the pack's namespace"
    );
    assert!(
        graph.clips().id("petramond:wave").is_some(),
        "rig clips take the engine's"
    );

    let mut animator = petramond_world::animation::Animator::new(std::sync::Arc::new(graph), 1);
    let main = animator.graph().slot("main").unwrap();
    animator.set_named("spear", 1.0);
    animator.fire_named("swing");
    animator.update(0.016);
    assert!(
        animator.playing(main).is_none(),
        "the pack's gate stands the replaced punch down, and the removed rule is gone"
    );
    animator.fire_named("thrust");
    animator.update(0.016);
    let playing = animator
        .playing(main)
        .expect("the pack's rule plays its library clip");
    assert_eq!(animator.graph().clips().name(playing.clip), "spears:jab");
}

/// `before` must name a row that exists, and a row must carry its key;
/// either mistake is a load error with the layer named, never a rule that
/// silently lands somewhere else.
#[test]
fn a_misplaced_or_unkeyed_row_is_refused_naming_the_layer() {
    let rig = rig();
    let base = r#"{ "slots": ["main"], "layers": [{ "name": "action", "slot": "main" }] }"#;
    let bad_before = r#"{ "layers": [{ "name": "x", "before": "nope", "slot": "main" }] }"#;
    let err = compile_animator(
        &[source(base, "petramond"), source(bad_before, "p")],
        &rig,
        |_| None,
    )
    .err()
    .expect("refused");
    assert!(err.starts_with("p:") && err.contains("nope"), "{err}");
    let moved = r#"{ "layers": [{ "name": "action", "before": "action", "slot": "main" }] }"#;
    let err = compile_animator(
        &[source(base, "petramond"), source(moved, "p")],
        &rig,
        |_| None,
    )
    .err()
    .expect("refused");
    assert!(
        err.starts_with("p:") && err.contains("replaces `action`"),
        "{err}"
    );
    let unkeyed = r#"{ "rules": [{ "on": "swing" }] }"#;
    let err = compile_animator(
        &[source(base, "petramond"), source(unkeyed, "p")],
        &rig,
        |_| None,
    )
    .err()
    .expect("refused");
    assert!(err.contains("`id`"), "{err}");
}

/// A pack layer that does not compile is left out ALONE: the engine's
/// document and every other pack's layer still load, and the refusal names
/// the layer. Before, one pack's typo took the whole rig's animator away —
/// the first-person hand with it.
#[test]
fn a_refused_pack_layer_is_left_out_and_the_rest_still_load() {
    let rig = rig();
    let base = r#"{ "events": ["swing"], "slots": ["main"], "layers": [{ "name": "action", "slot": "main" }],
                   "rules": [{ "id": "punch", "on": "swing", "slot": "main", "play": "petramond:wave" }] }"#;
    let broken = r#"{ "rules": [{ "id": "typo", "on": "swing", "slot": "main", "play": "petramond:wvae" }] }"#;
    let good = r#"{ "params": { "spear": 0 } }"#;
    let (graph, refused) = compile_layers(
        &[
            source(base, "petramond"),
            source(broken, "typos"),
            source(good, "spears"),
        ],
        &rig,
        |_| None,
    );
    let graph = graph.expect("the rig still has its animator");
    assert!(
        graph.param("spear").is_some(),
        "the layer after the broken one loads"
    );
    assert_eq!(refused.len(), 1, "{refused:?}");
    assert!(
        refused[0].contains("typos") && refused[0].contains("wvae"),
        "{}",
        refused[0]
    );

    let (graph, refused) = compile_layers(&[source(broken, "petramond")], &rig, |_| None);
    assert!(
        graph.is_none() && refused.len() == 1,
        "a broken own document leaves no animator"
    );
}

/// A library path is relative to the layer that states it: two layers naming
/// the same relative path each read their OWN file, and a pack's library may
/// not restate a clip the rig already has (that is how one pack would
/// silently rewrite another's motion), while the rig's own document may.
#[test]
fn a_library_resolves_under_its_own_layer_and_a_pack_may_not_replace_a_clip() {
    let rig = rig();
    let doc = r#"{ "libraries": ["extra.animation.json"] }"#;
    let library = |clip: &str| {
        format!(
            r#"{{ "format_version": "1.8.0", "animations": {{ "{clip}": {{
                "animation_length": 0.25, "bones": {{ "leftArm": {{ "rotation": {{ "0.0": [0, 0, 0], "0.25": [10, 0, 0] }} }} }}
            }} }} }}"#
        )
    };
    let read = |path: &Path| {
        let dir = path.parent()?.to_str()?;
        let clip = if dir == "petramond" { "wave" } else { "jab" };
        Some(library(clip).into_bytes())
    };
    let graph = compile_animator(
        &[source(doc, "petramond"), source(doc, "a"), source(doc, "b")],
        &rig,
        read,
    )
    .expect("each layer reads its own library");
    assert!(graph.clips().id("a:jab").is_some() && graph.clips().id("b:jab").is_some());
    let wave = graph
        .clips()
        .get(graph.clips().id("petramond:wave").unwrap());
    assert!(
        (wave.length - 0.25).abs() < 1e-6,
        "the rig's own document may restate a rig clip"
    );

    let both = r#"{ "libraries": ["one.animation.json", "two.animation.json"] }"#;
    let err = compile_animator(&[source(both, "spears")], &rig, |_| {
        Some(library("jab").into_bytes())
    })
    .err()
    .expect("a second library restating `jab` is refused");
    assert!(
        err.starts_with("spears: library 'two.animation.json'") && err.contains("`spears:jab`"),
        "{err}"
    );

    let err = compile_animator(&[source(doc, "spears")], &rig, |_| None)
        .err()
        .expect("a missing library");
    assert!(
        err.starts_with("spears: library 'extra.animation.json': not found"),
        "{err}"
    );
}
