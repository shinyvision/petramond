use super::*;

/// The shipped catalog gives each presenter the engine draws its rig, and
/// every rig loads its animator.
#[test]
fn the_shipped_rigs_catalog_has_a_rig_per_presenter() {
    for presenter in [Presenter::Body, Presenter::Viewmodel] {
        let (_, rig) = presented(presenter).unwrap_or_else(|| panic!("no {presenter:?} rig"));
        assert!(rig.graph.is_some(), "{}: its animator loads", rig.name);
    }
}

/// root → body → left_item, right_item, camera.
fn model() -> Model {
    Model::load(
        r#"{
        "resolution": { "width": 16, "height": 16 },
        "textures": [{ "uv_width": 16, "uv_height": 16,
            "source": "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==" }],
        "elements": [],
        "groups": [
            { "uuid": "root", "name": "root", "origin": [0, 0, 0] },
            { "uuid": "body", "name": "body", "origin": [0, 12, 0] },
            { "uuid": "li", "name": "left_item", "origin": [5, 10, 0] },
            { "uuid": "ri", "name": "right_item", "origin": [-5, 10, 0] },
            { "uuid": "cam", "name": "camera", "origin": [0, 24, 0] }
        ],
        "outliner": [{ "uuid": "root", "children": [
            { "uuid": "body", "children": [{ "uuid": "li", "children": [] }, { "uuid": "ri", "children": [] }] },
            { "uuid": "cam", "children": [] }
        ] }]
    }"#,
    )
    .expect("test model parses")
}

/// A row is a rig only whole, and only where its presenter is free: a row
/// naming a bone its model lacks, a render kind that is not one, or a
/// second rig for a presenter already drawn is refused with its name — and
/// every other row still loads, its conventions resolved to bone ids.
#[test]
fn a_broken_row_or_a_second_rig_for_a_presenter_is_refused_alone() {
    let row = |presenter: &str, grip: &str| {
        serde_json::json!({
            "model": "m.bbmodel", "animator": "a.json", "observed": true, "presenter": presenter,
            "grips": { "main": grip, "off": "right_item" }, "camera": "camera", "twist": ["body"]
        })
    };
    let mut doc = Map::new();
    doc.insert("a_body".into(), row("body", "left_item"));
    doc.insert("b_body".into(), row("body", "left_item"));
    doc.insert("c_view".into(), row("viewmodel", "left_hand"));
    let mut holds = row("viewmodel", "left_item");
    holds["holds"] = serde_json::json!({ "potion": "x" });
    doc.insert("d_view".into(), holds);
    doc.insert("e_view".into(), row("viewmodel", "right_item"));

    let (rigs, errors) = rigs_from(&doc, |_| Some(model()), |_, _| None);
    let names: Vec<&str> = rigs.iter().map(|r| r.name.as_str()).collect();
    assert_eq!(names, ["a_body", "e_view"], "{errors:?}");
    assert_eq!(errors.len(), 3, "{errors:?}");
    assert!(
        errors[0].starts_with("b_body") && errors[0].contains("presenter"),
        "{}",
        errors[0]
    );
    assert!(
        errors[1].starts_with("c_view") && errors[1].contains("left_hand"),
        "{}",
        errors[1]
    );
    assert!(
        errors[2].starts_with("d_view") && errors[2].contains("potion"),
        "{}",
        errors[2]
    );

    let m = model();
    let body = &rigs[0];
    assert_eq!(
        body.grips,
        [
            m.bone_named("left_item").unwrap(),
            m.bone_named("right_item").unwrap()
        ]
    );
    assert_eq!(body.camera, m.bone_named("camera"));
    assert_eq!(body.twist, [m.bone_named("body").unwrap()]);
}
