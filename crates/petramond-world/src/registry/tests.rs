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

#[test]
fn tag_table_interns_namespaced_and_rejects_bare_unknowns() {
    let t = TagTable::new(&["fuel", "planks"]);
    assert_eq!(t.resolve("fuel"), Ok(0));
    assert_eq!(
        t.resolve("petramond:planks"),
        Ok(1),
        "an engine tag resolves under its namespaced recipe form too"
    );
    let a = t.resolve("mymod:ores").expect("namespaced tags intern");
    assert_eq!(t.resolve("mymod:ores"), Ok(a), "stable on re-resolution");
    assert_eq!(t.name(a), "mymod:ores");
    assert!(
        t.resolve("orees").is_err(),
        "a bare unknown is a typo'd engine tag, never a silent new tag"
    );
    assert!(
        t.resolve("petramond:fuell").is_err(),
        "the engine namespace is reserved: a typo there must not intern a \
         dead tag that nothing carries and nothing reports"
    );
    assert_eq!(
        t.lookup("petramond:fuell"),
        None,
        "and the query side never sees one either"
    );
}

/// The reason the ids are two bytes: with EVERY shipped pack installed the
/// registries must still have room for more content, not a couple of dozen
/// free ids. This reads the real installed pack set, so it fails the day
/// the shipped packs genuinely crowd the table again.
#[test]
fn the_installed_pack_set_leaves_room_for_more_packs() {
    // Comfortably more than any one content pack registers, and small
    // enough that it is a real bound rather than a restatement of the cap.
    const ROOM: usize = 1024;
    let names = names();
    for (what, used) in [("block", names.blocks.len()), ("item", names.items.len())] {
        assert!(
            used <= WIDE_ID_CAP && WIDE_ID_CAP - used >= ROOM,
            "{what} registry: {used}/{WIDE_ID_CAP} ids used, only {} free — a further \
             content pack would be refused admission",
            WIDE_ID_CAP - used,
        );
    }
}

#[test]
fn namespaced_keys_register_and_bare_unknowns_error() {
    let engine = &["petramond:air", "petramond:stone"];
    // Engine override (known `petramond:*`) + a namespaced addition.
    let table = NameTable::build(
        engine,
        &[vec!["petramond:stone".into(), "mymod:gadget".into()]],
        "block",
        WIDE_ID_CAP,
    )
    .expect("valid layers");
    assert_eq!(table.len(), 3, "override adds no id; the addition does");
    assert_eq!(
        table.id("petramond:stone"),
        Some(1),
        "engine ids never move"
    );
    assert_eq!(table.id("mymod:gadget"), Some(2), "appended after engine");
    assert_eq!(table.name(2), Some("mymod:gadget"));
    // Restating a registered dynamic name in a later layer adds no id.
    let table = NameTable::build(
        engine,
        &[vec!["mymod:gadget".into()], vec!["mymod:gadget".into()]],
        "block",
        WIDE_ID_CAP,
    )
    .unwrap();
    assert_eq!(table.len(), 3);
    // A NEW bare name is an error, not a registration.
    let err = NameTable::build(engine, &[vec!["gadget".into()]], "block", WIDE_ID_CAP)
        .expect_err("bare additions are refused");
    assert!(err.contains("gadget") && err.contains("namespace"), "{err}");
    let err = NameTable::build(
        engine,
        &[vec!["petramond:gadget".into()]],
        "block",
        WIDE_ID_CAP,
    )
    .expect_err("unknown engine-namespace additions are refused");
    assert!(
        err.contains("petramond") && err.contains("reserved"),
        "{err}"
    );
    // Degenerate namespaces are not namespaces.
    for bad in [":gadget", "mymod:", ":"] {
        assert!(!is_namespaced(bad), "{bad}");
    }
    assert!(is_namespaced("mymod:gadget"));
}

#[test]
fn registry_caps_at_its_declared_ceiling() {
    // The wide (block/item) ceiling and the byte ceiling are separate
    // numbers, and each catalog is held to its own.
    let engine = &["petramond:air"];
    let keys: Vec<String> = (0..WIDE_ID_CAP)
        .map(|i| format!("mymod:thing_{i}"))
        .collect();
    let err = NameTable::build(engine, std::slice::from_ref(&keys), "block", WIDE_ID_CAP)
        .expect_err("cap enforced");
    assert!(err.contains(&WIDE_ID_CAP.to_string()), "{err}");
    assert!(
        NameTable::build(
            engine,
            &[keys[..WIDE_ID_CAP - 1].to_vec()],
            "block",
            WIDE_ID_CAP
        )
        .is_ok(),
        "one under the ceiling still loads"
    );
    let byte_keys: Vec<String> = (0..BYTE_ID_CAP)
        .map(|i| format!("mymod:thing_{i}"))
        .collect();
    assert!(
        NameTable::build(engine, &[byte_keys], "mob", BYTE_ID_CAP).is_err(),
        "a byte-capped catalog is held to 256, not to the wide ceiling"
    );
}
