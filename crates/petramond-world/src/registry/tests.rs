use super::*;

#[derive(serde::Deserialize, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
struct Row {
    key: String,
    #[serde(default)]
    tiles: Vec<String>,
    #[serde(default)]
    roles: std::collections::BTreeMap<String, String>,
    #[serde(default)]
    hardness: f32,
}

/// A row extending an earlier one starts as that row and replaces WHOLE
/// fields — a list or map it states is exactly what it wrote, never a merge
/// — keeps its own key, and can itself be extended in turn.
#[test]
fn an_extending_row_copies_its_base_and_replaces_whole_fields() {
    let text = r#"{ "rows": [
        { "key": "m:base", "tiles": ["a", "b"], "roles": {"x": "hidden", "y": "hidden"}, "hardness": 2 },
        { "key": "m:one", "extends": "m:base", "roles": {"z": "hitbox"} },
        { "key": "m:two", "extends": "m:one", "tiles": ["c"] }
    ] }"#;
    let rows: Vec<Row> = parse_rows(text, "rows", "key").expect("parses");
    assert_eq!(rows.len(), 3);
    let one = &rows[1];
    assert_eq!(one.key, "m:one");
    assert_eq!(one.tiles, ["a", "b"], "an unstated field is the base's");
    assert_eq!(one.hardness, 2.0);
    assert_eq!(
        one.roles.keys().collect::<Vec<_>>(),
        ["z"],
        "a stated map replaces the base's map outright"
    );
    let two = &rows[2];
    assert_eq!(two.tiles, ["c"]);
    assert_eq!(
        two.roles.keys().collect::<Vec<_>>(),
        ["z"],
        "chains resolve in order"
    );
}

#[test]
fn extending_a_row_that_is_not_earlier_in_the_layer_is_an_error() {
    let later = r#"{ "rows": [
        { "key": "m:one", "extends": "m:base" },
        { "key": "m:base", "hardness": 1 }
    ] }"#;
    let err = parse_rows::<Row>(later, "rows", "key").expect_err("a later base is not a base");
    assert!(err.to_string().contains("m:base"), "{err}");
    let missing = r#"{ "rows": [ { "key": "m:one", "extends": "m:nowhere" } ] }"#;
    assert!(parse_rows::<Row>(missing, "rows", "key").is_err());
}

/// Patch rows ride the same array and keep working beside templates: they
/// are split out after expansion, and a template never sees them as a base.
#[test]
fn patch_rows_split_out_after_templates_expand() {
    let text = r#"{ "rows": [
        { "key": "m:base", "hardness": 3 },
        { "patch": "m:base", "data": {"n:k": 1} },
        { "key": "m:one", "extends": "m:base" }
    ] }"#;
    let mut patches = Vec::new();
    let rows: Vec<Row> =
        parse_rows_with_patches(text, "rows", "key", &mut patches).expect("parses");
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[1].hardness, 3.0);
    assert_eq!(patches.len(), 1);
    assert_eq!(patches[0].patch, "m:base");
}
