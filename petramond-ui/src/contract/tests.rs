use super::*;
use crate::state::UiState;
use crate::theme::Theme;

struct Tags(&'static [&'static str]);

impl EngineCatalog for Tags {
    fn item_tag_exists(&self, name: &str) -> bool {
        self.0.contains(&name)
    }
}

static FUEL_ONLY: Tags = Tags(&["petramond:fuel"]);

fn doc(kind: &str, class: &str, root: &str) -> Document {
    Document::from_json(&format!(
        r#"{{ "format": 1, "kind": "{kind}", "class": "{class}", "root": {root} }}"#
    ))
    .expect("test document parses")
}

fn check<'a>(
    pack_id: Option<&'a str>,
    image_size: &'a dyn Fn(&str) -> Option<(u32, u32)>,
) -> EngineCheck<'a> {
    EngineCheck {
        styles: None,
        pack_id,
        catalog: &FUEL_ONLY,
        image_size,
    }
}

fn no_images(_: &str) -> Option<(u32, u32)> {
    None
}

fn messages(issues: &[DocIssue]) -> String {
    issues
        .iter()
        .map(|i| i.message.as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn engine_kind_keys_are_unique_and_engine_namespaced() {
    for (i, kind) in ENGINE_KINDS.iter().enumerate() {
        assert_eq!(namespace(kind.key), Some(ENGINE_NAMESPACE), "{}", kind.key);
        assert!(
            ENGINE_KINDS[..i].iter().all(|k| k.key != kind.key),
            "{} listed twice",
            kind.key
        );
    }
}

#[test]
fn every_engine_contract_role_is_listed_once() {
    for kind in ENGINE_KINDS {
        for (i, (role, count)) in kind.slots.iter().enumerate() {
            assert!(*count > 0, "{}: {role} pins zero slots", kind.key);
            assert!(
                kind.slots[..i].iter().all(|(r, _)| r != role),
                "{}: {role} listed twice",
                kind.key
            );
        }
    }
}

#[test]
fn the_creative_screen_pins_its_hotbar() {
    let creative = engine_kind("petramond:creative").expect("creative is an engine kind");
    assert_eq!(
        creative.contract(),
        SlotContract::new(&[("hotbar", HOTBAR_SLOTS)])
    );
    let empty = doc("petramond:creative", "container", r#"{ "type": "column" }"#);
    let issues = validate_for_engine(&empty, &check(None, &no_images));
    assert!(
        messages(&issues).contains("role 'hotbar' missing"),
        "{issues:?}"
    );
}

#[test]
fn kinds_classify_like_the_game_registry() {
    assert!(matches!(
        classify_kind("petramond:chest"),
        Ok(KindClass::Engine(k)) if k.key == "petramond:chest"
    ));
    assert_eq!(classify_kind("wheel:spin"), Ok(KindClass::Mod));
    assert!(classify_kind("petramond:no_such_screen").is_err());
    assert!(classify_kind("bare").is_err());
    assert!(classify_kind(":x").is_err());
}

#[test]
fn a_namespaced_kind_must_ship_from_its_own_pack() {
    assert!(kind_permitted("doctest:owned", Some("doctest")).is_ok());
    assert!(kind_permitted("doctest:owned", Some("otherpack")).is_err());
    assert!(kind_permitted("doctest:owned", None).is_err());
    assert!(kind_permitted("petramond:furnace", None).is_ok());
    assert!(kind_permitted("petramond:title", Some("anypack")).is_ok());

    let d = doc("doctest:owned", "screen", r#"{ "type": "column" }"#);
    assert!(validate_for_engine(&d, &check(Some("doctest"), &no_images)).is_empty());
    let issues = validate_for_engine(&d, &check(Some("otherpack"), &no_images));
    assert!(
        messages(&issues).contains("does not belong to pack"),
        "{issues:?}"
    );
}

#[test]
fn mod_documents_earn_only_the_generic_roles() {
    let grid = |role: &str, cols: u32, rows: u32| {
        doc(
            "doctest:machine",
            "container",
            &format!(
                r#"{{ "type": "slot_grid", "role": "{role}", "cols": {cols}, "rows": {rows} }}"#
            ),
        )
    };
    assert_eq!(
        mod_document_contract(&grid("container", 6, 9)),
        Ok(SlotContract::new(&[("container", MAX_CONTAINER_SLOTS)]))
    );
    let err = mod_document_contract(&grid("container", 11, 5)).unwrap_err();
    assert!(err.contains("the cap is"), "{err}");
    assert!(mod_document_contract(&grid("player_inv", 9, 3)).is_ok());
    let err = mod_document_contract(&grid("player_inv", 9, 2)).unwrap_err();
    assert!(err.contains("the engine grids are"), "{err}");
    let err = mod_document_contract(&grid("craft_result", 1, 1)).unwrap_err();
    assert!(err.contains("not available to mod documents"), "{err}");
}

#[test]
fn slot_semantics_follow_the_engine_rules() {
    let slot = |role: &str, accepts: &str| {
        doc(
            "doctest:machine",
            "container",
            &format!(r#"{{ "type": "slot", "role": "{role}", "accepts": [{accepts}] }}"#),
        )
    };
    let tags = Tags(&["petramond:fuel"]);
    assert!(slot_semantics_issues(&slot("container", r#""petramond:fuel""#), &tags).is_empty());
    let issues = slot_semantics_issues(&slot("container", r#""doctest:no_such_tag""#), &tags);
    assert!(issues[0].contains("unknown item tag"), "{issues:?}");
    let data_key = slot("container", r#"{"data": "doctest:metal"}"#);
    assert!(slot_semantics_issues(&data_key, &tags).is_empty());
    let issues = slot_semantics_issues(&slot("container", r#"{"data": "metal"}"#), &tags);
    assert!(issues[0].contains("namespaced"), "{issues:?}");
    let issues = slot_semantics_issues(&slot("hotbar", r#""petramond:fuel""#), &tags);
    assert!(
        issues[0].contains("apply only to 'container'"),
        "{issues:?}"
    );

    let many: Vec<String> = (0..=MAX_SLOT_FILTERS)
        .map(|i| format!(r#"{{"data": "doctest:f{i}"}}"#))
        .collect();
    let issues = slot_semantics_issues(&slot("container", &many.join(", ")), &tags);
    assert!(
        issues[0].contains("accepts filters; the cap is"),
        "{issues:?}"
    );
}

#[test]
fn image_refs_keep_first_reference_order_and_the_first_grid() {
    let d = doc(
        "doctest:art",
        "screen",
        r#"{ "type": "column", "children": [
              { "type": "image", "image": "a.png" },
              { "type": "button", "id": "go", "image": "b.png" },
              { "type": "image", "image": "a.png", "frames": [2, 1] },
              { "type": "image", "image": "", "bind": { "image": "icon" } }
            ] }"#,
    );
    assert_eq!(
        image_refs(&d),
        vec![
            ImageRef {
                name: "a.png".into(),
                frames: Some([2, 1]),
                path: "root/2(image)".into(),
            },
            ImageRef {
                name: "b.png".into(),
                frames: None,
                path: "root/1(button#go)".into(),
            },
        ]
    );
}

#[test]
fn missing_art_and_bad_sheets_are_document_errors() {
    let d = doc(
        "doctest:art",
        "screen",
        r#"{ "type": "column", "children": [
              { "type": "image", "image": "ok.png", "frames": [2, 2] },
              { "type": "image", "image": "ragged.png", "frames": [4, 1] },
              { "type": "image", "image": "many.png", "frames": [65, 1] },
              { "type": "image", "image": "huge.png", "frames": [7, 1] },
              { "type": "image", "image": "gone.png" }
            ] }"#,
    );
    let sizes = |name: &str| match name {
        "ok.png" => Some((8, 4)),
        "ragged.png" => Some((10, 4)),
        "many.png" => Some((65, 1)),
        "huge.png" => Some((700, 7)),
        _ => None,
    };
    let issues = image_issues(&d, &sizes);
    assert_eq!(issues.len(), 4, "{issues:#?}");
    assert!(issues[0].message.contains("does not divide evenly"));
    assert_eq!(issues[0].path, "root/1(image)");
    assert!(issues[1].message.contains("frames; the cap is"));
    assert!(issues[2].message.contains("side cap"));
    assert!(issues[3].message.contains("missing art gone.png"));
    assert_eq!(issues[3].path, "root/4(image)");
}

#[test]
fn a_panel_taller_than_the_smallest_viewport_is_reported_with_its_path() {
    let theme = Theme::placeholder();
    let fits = doc(
        "doctest:fit",
        "screen",
        r#"{ "type": "column", "children": [ { "type": "spacer", "layout": { "h": 40 } } ] }"#,
    );
    assert!(viewport_overflow(&fits, &theme, &UiState::new(), 1, &|_| None).is_empty());

    let tall = doc(
        "doctest:fit",
        "screen",
        r#"{ "type": "column", "children": [
              { "type": "column", "layout": { "h": 100 }, "children": [
                  { "type": "spacer", "id": "big", "layout": { "h": 400 } }
              ] }
            ] }"#,
    );
    let issues = viewport_overflow(&tall, &theme, &UiState::new(), 1, &|_| None);
    assert!(
        issues.iter().any(|i| i.path == "root/0/0(spacer#big)"),
        "{issues:#?}"
    );

    let scrolled = doc(
        "doctest:fit",
        "screen",
        r#"{ "type": "scroll", "id": "s", "layout": { "h": 100 }, "children": [
              { "type": "spacer", "layout": { "h": 400 } }
            ] }"#,
    );
    assert!(viewport_overflow(&scrolled, &theme, &UiState::new(), 1, &|_| None).is_empty());
}
