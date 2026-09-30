use super::*;
use crate::graph::SamplePoint;

fn base() -> String {
    petramond_world::assets::read_base_text("density/terrain.json")
        .expect("shipped density/terrain.json")
        .0
}

fn sample_points() -> Vec<SamplePoint> {
    let mut points = Vec::new();
    for gx in -12..=12 {
        for gz in -12..=12 {
            let (x, z) = (f64::from(gx) * 1733.0 + 0.5, f64::from(gz) * 1571.0 - 3.25);
            for y in [-64.0, 0.0, 4.0, 40.0, 63.0, 90.0, 180.0] {
                points.push(SamplePoint::new(x, y, z));
            }
        }
    }
    for x in (-48..48).step_by(3) {
        for z in (-48..48).step_by(5) {
            points.push(SamplePoint::new(f64::from(x), 70.0, f64::from(z)));
        }
    }
    points
}

#[test]
fn a_pack_layer_retunes_the_recipe_by_name() {
    let base = base();
    let shipped = parse_layers(&[&base]).expect("shipped recipe loads");
    let flat = r#"{"nodes": {"base_height": 70.0}}"#;
    let flat_nodes = r#"{"nodes": {"base_height": {"constant": 70.0}}}"#;
    let layered = parse_layers(&[&base, flat_nodes]).expect("a node override loads");
    assert!(
        parse_layers(&[&base, flat]).is_err(),
        "a node is an op, not a bare number"
    );
    let graph = layered.build(9);
    for point in sample_points().into_iter().take(50) {
        assert_eq!(
            graph.evaluate_channel(channels::BASE_HEIGHT, point),
            Some(70.0)
        );
    }
    assert_ne!(layered.fingerprint, shipped.fingerprint);
    assert_eq!(
        shipped.fingerprint,
        parse_layers(&[&base]).unwrap().fingerprint
    );

    let rerouted = r#"{"channels": {"base_height": 64.0}}"#;
    let graph = parse_layers(&[&base, rerouted])
        .expect("a channel override loads")
        .build(9);
    let point = SamplePoint::new(10.0, 0.0, 10.0);
    assert_eq!(
        graph.evaluate_channel(channels::BASE_HEIGHT, point),
        Some(64.0)
    );
}

#[test]
fn malformed_recipes_are_refused() {
    let base = base();
    for (bad, why) in [
        (
            r#"{"channels": {"base_height": "nowhere"}}"#,
            "unknown node",
        ),
        (
            r#"{"nodes": {"a": {"abs": "b"}, "b": {"abs": "a"}}, "channels": {"base_height": "a"}}"#,
            "node cycle",
        ),
        (
            r#"{"nodes": {"offset": {"spline": {"spline": "offset", "inputs": {"ridge": "ridge"}}}}}"#,
            "unbound spline axis",
        ),
        (
            r#"{"splines": {"offset": {"axis": "continentality", "points": [[0.5, 0.0], [0.1, 1.0]]}}}"#,
            "knots out of order",
        ),
        (
            r#"{"splines": {"offset": {"axis": "continentality", "points": [[0.0, "offset"]]}}}"#,
            "spline cycle",
        ),
        (
            r#"{"fields": {"crag": {"salt": [1, 2], "first_octave": -1, "amplitudes": [1.0, 1.0, 1.0]}}}"#,
            "octaves past 0",
        ),
        (
            r#"{"fields": {"crag": {"salt": [1, 2], "first_octave": -5, "amplitudes": [0.0]}}}"#,
            "silent field",
        ),
        (
            r#"{"nodes": {"crest": {"field": "moonlight"}}}"#,
            "unknown field",
        ),
        (
            r#"{"channels": {"base_height": {"axis": "y"}}}"#,
            "vertical base height",
        ),
        (r#"{"nodes": {"crest": {"sparkle": 1.0}}}"#, "unknown op"),
        (
            r#"{"nodes": {"crest": {"terrace": {"input": 1.0, "step": 0.0}}}}"#,
            "flat terrace",
        ),
    ] {
        assert!(parse_layers(&[&base, bad]).is_err(), "accepted: {why}");
    }
    let without_density = base.replace(r#""master_density": "master_density","#, "");
    assert!(
        parse_layers(&[&without_density]).is_err(),
        "missing master_density"
    );
}

#[test]
fn the_whole_node_vocabulary_loads() {
    let extra = r#"{"nodes": {"surface_detection": {"range_select": {
        "selector": {"min": [{"axis": "x"}, {"max": [{"axis": "z"}, 0.0]}]},
        "min": -1.0, "max": 1.0,
        "inside": {"vertical_ramp": {"y_min": 0.0, "y_max": 10.0}},
        "outside": 0.0}}},
        "channels": {"surface_detection": "surface_detection"}}"#;
    let graph = parse_layers(&[&base(), extra]).expect("loads").build(1);
    let inside = SamplePoint::new(0.5, 5.0, -3.0);
    assert_eq!(
        graph.evaluate_channel(channels::SURFACE_DETECTION, inside),
        Some(0.5)
    );
    let outside = SamplePoint::new(4.0, 5.0, 2.0);
    assert_eq!(
        graph.evaluate_channel(channels::SURFACE_DETECTION, outside),
        Some(0.0)
    );
}
