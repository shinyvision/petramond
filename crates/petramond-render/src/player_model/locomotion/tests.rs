use super::*;

fn all(_: &str) -> bool {
    true
}

fn weights_of(
    table: &LocomotionTable,
    inputs: &Inputs,
    has: impl Fn(&str) -> bool,
) -> Vec<(String, Phase, f32)> {
    table
        .weights(inputs, has)
        .map(|(clip, phase, w)| (clip.to_owned(), phase, w))
        .collect()
}

/// Factors multiply; `!x` is the complement, `{"not": x, "by": k}` the partial
/// complement; derived names are reusable products; phases pass through.
#[test]
fn layer_weights_are_products_of_named_factors() {
    let table = LocomotionTable::parse_layers(&[r#"{
        "derived": [{"name": "ground", "factors": ["!water", {"not": "landing", "by": 0.5}]}],
        "layers": [
            {"id": "a", "clip": "walk", "phase": "stride", "factors": ["walking", "ground", "!run"]},
            {"id": "b", "clip": "tread", "phase": "swim", "factors": ["water"]},
            {"id": "c", "clip": "land", "phase": "rest", "factors": ["landing"]}
        ]}"#])
    .unwrap();
    let inputs = Inputs {
        walking: 0.8,
        run: 0.25,
        water: 0.0,
        landing: 0.4,
        ..Default::default()
    };
    let got = weights_of(&table, &inputs, all);
    let expect_a = 0.8 * (1.0 - 0.5 * 0.4) * (1.0 - 0.25);
    assert_eq!(got.len(), 2, "a zero-weight layer (tread) is dropped");
    assert_eq!(got[0].0, "walk");
    assert_eq!(got[0].1, Phase::Stride);
    assert!((got[0].2 - expect_a).abs() < 1e-6);
    assert_eq!(got[1], ("land".to_owned(), Phase::Rest, 0.4));
}

/// `requires` zeroes an input the model cannot show, so its weight folds back
/// into the layers whose complements read it; a gated derived slot reads 0.
#[test]
fn requires_folds_missing_clips_back_into_the_base_layer() {
    let table = LocomotionTable::parse_layers(&[r#"{
        "requires": {"run": ["run"], "lateral": ["strafe_left", "strafe_right"]},
        "derived": [{"name": "lateral", "factors": ["strafe"]}],
        "layers": [
            {"id": "walk", "clip": "walk", "phase": "stride", "factors": ["walking", "!run", "!lateral"]},
            {"id": "run", "clip": "run", "phase": "stride", "factors": ["walking", "run"]},
            {"id": "strafe", "clip": "strafe_left", "phase": "stride", "factors": ["walking", "lateral"]}
        ]}"#])
    .unwrap();
    let inputs = Inputs {
        walking: 1.0,
        run: 0.6,
        strafe: 0.5,
        ..Default::default()
    };
    let full = weights_of(&table, &inputs, all);
    assert_eq!(full.len(), 3);
    assert!((full[0].2 - 0.4 * 0.5).abs() < 1e-6);
    let no_run = weights_of(&table, &inputs, |c| c != "run");
    assert_eq!(
        no_run
            .iter()
            .map(|(c, _, _)| c.as_str())
            .collect::<Vec<_>>(),
        ["walk", "strafe_left"]
    );
    assert!((no_run[0].2 - 0.5).abs() < 1e-6, "walk takes the run share");
    let no_strafe = weights_of(&table, &inputs, |c| c != "strafe_right");
    assert_eq!(
        no_strafe
            .iter()
            .map(|(c, _, _)| c.as_str())
            .collect::<Vec<_>>(),
        ["walk", "run"]
    );
    assert!(
        (no_strafe[0].2 - 0.4).abs() < 1e-6,
        "a partially present pair gates the whole slot"
    );
}

/// A later asset layer replaces a row by id (or disables it), appends new
/// rows, and re-defines a derived name in place.
#[test]
fn pack_layers_override_by_id_and_append() {
    let base = r#"{
        "derived": [{"name": "ground", "factors": ["!water"]}],
        "layers": [
            {"id": "walk", "clip": "walk", "phase": "stride", "factors": ["walking", "ground"]},
            {"id": "run", "clip": "run", "phase": "stride", "factors": ["run"]}
        ]}"#;
    let pack = r#"{
        "derived": [{"name": "ground", "factors": ["!water", "!airborne"]}],
        "layers": [
            {"id": "run", "clip": "run", "phase": "stride", "factors": ["run"], "enabled": false},
            {"id": "skip", "clip": "skip", "phase": "stride", "factors": ["walking", "ground"]}
        ]}"#;
    let table = LocomotionTable::parse_layers(&[base, pack]).unwrap();
    let inputs = Inputs {
        walking: 1.0,
        run: 1.0,
        airborne: 0.5,
        ..Default::default()
    };
    let got = weights_of(&table, &inputs, all);
    assert_eq!(
        got.iter().map(|(c, _, _)| c.as_str()).collect::<Vec<_>>(),
        ["walk", "skip"]
    );
    assert!(
        got.iter().all(|(_, _, w)| (*w - 0.5).abs() < 1e-6),
        "the redefined derived applies to both"
    );
}

#[test]
fn unknown_and_forward_names_are_load_errors() {
    let bad = [
        (
            r#"{"layers": [{"id": "a", "clip": "x", "phase": "rest", "factors": ["nope"]}]}"#,
            "not an input",
        ),
        (
            r#"{"derived": [{"name": "a", "factors": ["b"]}, {"name": "b", "factors": ["walking"]}]}"#,
            "earlier",
        ),
        (
            r#"{"derived": [{"name": "walking", "factors": ["run"]}]}"#,
            "shadows",
        ),
        (r#"{"requires": {"nope": ["x"]}}"#, "requires"),
        (
            r#"{"layers": [{"id": "a", "clip": "x", "phase": "rest", "factors": [{"not": "run", "by": 2}]}]}"#,
            "0..=1",
        ),
    ];
    for (text, expect) in bad {
        let err = LocomotionTable::parse_layers(&[text]).err().expect(expect);
        assert!(err.contains(expect), "{err}");
    }
}

/// The shipped table parses whole and names clips only — the sanity that
/// survives re-authoring.
#[test]
fn shipped_table_loads() {
    let layers = petramond_world::assets::read_layers("animations/player_locomotion.json");
    let texts: Vec<&str> = layers.iter().map(|(t, _)| t.as_str()).collect();
    let table = LocomotionTable::parse_layers(&texts).expect("shipped table");
    assert!(!table.layers.is_empty());
}
