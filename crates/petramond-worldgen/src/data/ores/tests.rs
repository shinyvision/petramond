use super::*;

fn base() -> String {
    petramond_world::assets::read_base_text("ores.json")
        .expect("shipped ores.json")
        .0
}

#[test]
fn malformed_rows_are_refused() {
    let row = |fields: &str| {
        format!(
            r#"{{"ores": [{{"ore": "mymod:x", "block": "petramond:gold_ore", "salt": 1,
                "count": 2, {fields}}}]}}"#
        )
    };
    let base = base();
    let valid = row(r#""shape": {"blob": {"size": 9}}, "y": [0, 10]"#);
    assert!(parse_layers(&[&base, &valid]).is_ok());
    for bad in [
        r#""shape": {"blob": {"size": 9}}, "y": [10, 0]"#,
        r#""shape": {"blob": {"size": 9}}, "y": [-100, 10]"#,
        r#""shape": {"blob": {"size": 0}}, "y": [0, 10]"#,
        r#""shape": {"blob": {"size": 100000}}, "y": [0, 10]"#,
        r#""shape": {"grid3": {"max_ore": 10}}, "y": [0, 10]"#,
        r#""shape": {"blob": {"size": 9}}, "y": [0, 10], "depth_ramp": 2.0"#,
        r#""shape": {"blob": {"size": 9}}, "y": [5, 5], "depth_ramp": 0.5"#,
        r#""shape": {"blob": {"size": 9}}, "y": [0, 10], "hosts": []"#,
        r#""shape": {"blob": {"size": 9}}, "y": [0, 10], "hosts": ["petramond:nope"]"#,
        r#""shape": {"cube": {"size": 9}}, "y": [0, 10]"#,
        r#""shape": {"blob": {"size": 9}}, "y": [0, 10], "glow": true"#,
    ] {
        assert!(parse_layers(&[&base, &row(bad)]).is_err(), "accepted {bad}");
    }
}

#[test]
fn salts_derive_from_names_and_never_repeat() {
    let pack = r#"{"ores": [{"ore": "mymod:tin", "block": "petramond:gold_ore",
        "count": 2, "shape": {"blob": {"size": 9}}, "y": [0, 10]}]}"#;
    let table = parse_layers(&[&base(), pack]).expect("an unsalted row loads");
    let tin = table.veins.last().expect("pack row");
    assert_eq!(tin.salt, crate::salts::named("ore", "mymod:tin"));

    let clash = r#"{"ores": [{"ore": "mymod:tin", "block": "petramond:gold_ore",
        "salt": 10551312, "count": 2, "shape": {"blob": {"size": 9}}, "y": [0, 10]}]}"#;
    let err = parse_layers(&[&base(), clash])
        .err()
        .expect("a row reusing coal's salt is refused");
    assert!(
        err.contains("petramond:coal_ore") && err.contains("mymod:tin"),
        "{err}"
    );
}
