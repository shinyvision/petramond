use super::*;
use crate::state::{UiMap, UiState, UiValue};
use crate::tree::{InstKey, InstTree};
use std::sync::Arc;

fn doc() -> Document {
    Document::from_json(
        r#"{
        "format": 1, "kind": "petramond:x", "class": "screen",
        "root": { "type": "column", "children": [
            { "type": "label", "id": "title", "bind": { "text": "title" } },
            { "type": "column", "id": "panel", "children": [
                { "type": "label", "bind": { "text": "status" } },
                { "type": "button", "id": "extra", "text": "More",
                  "bind": { "visible": "show_extra" } }
            ] },
            { "type": "list", "id": "rows", "bind": { "items": "rows" }, "children": [
                { "type": "label", "id": "row", "bind": { "text": "name" } }
            ] },
            { "type": "button", "id": "anchor", "text": "?" },
            { "type": "tooltip", "hover": "anchor", "children": [
                { "type": "label", "text": "help" }
            ] }
        ] }
    }"#,
    )
    .unwrap()
}

fn rows(names: &[&str]) -> UiValue {
    UiValue::List(Arc::new(
        names
            .iter()
            .map(|n| {
                let mut m = UiMap::new();
                m.insert("name".into(), UiValue::Str((*n).into()));
                m
            })
            .collect(),
    ))
}

fn summary(tree: &InstTree<'_>) -> Vec<String> {
    tree.insts
        .iter()
        .map(|i| {
            format!(
                "{} {:?} {:?} {:?} {} {:?} {:?} {}",
                i.node_id, i.item, i.key, i.text, i.enabled, i.parent, i.children, i.span
            )
        })
        .collect()
}

fn reexpand(
    state: &mut UiState,
    hover: (Option<&str>, Option<&str>),
    change: impl FnOnce(&mut UiState),
) -> (Vec<String>, Vec<String>, usize) {
    let doc = doc();
    let shape = DocShape::of(&doc);
    let key = |id: Option<&str>| {
        id.map(|id| InstKey {
            id: id.to_owned(),
            item: None,
        })
    };
    let (before, after) = (key(hover.0), key(hover.1));
    let first = InstTree::expand_with(&doc, &shape, state, false, before.as_ref(), None);
    let since = state.revision();
    change(state);
    let changed = changed_deps(state, since, before != after);
    let mut prev = Prev::new(first.into_data(), &changed);
    let reused = InstTree::expand_with(&doc, &shape, state, false, after.as_ref(), Some(&mut prev));
    let fresh = InstTree::expand_with(&doc, &shape, state, false, after.as_ref(), None);
    (summary(&reused), summary(&fresh), prev.adopted())
}

fn base_state() -> UiState {
    let mut s = UiState::new();
    s.set("title", UiValue::Str("Title".into()));
    s.set("status", UiValue::Str("idle".into()));
    s.set("show_extra", UiValue::Bool(false));
    s.set("rows", rows(&["a", "b"]));
    s
}

#[test]
fn an_unrelated_change_reuses_everything_but_what_read_it() {
    let mut state = base_state();
    let (reused, fresh, adopted) = reexpand(&mut state, (None, None), |s| {
        s.set("title", UiValue::Str("Renamed".into()));
    });
    assert_eq!(reused, fresh);
    assert_eq!(adopted, fresh.len() - 2, "{fresh:#?}");
}

#[test]
fn a_hidden_child_appears_even_though_its_parent_read_nothing_else() {
    let mut state = base_state();
    let (reused, fresh, _) = reexpand(&mut state, (None, None), |s| {
        s.set("show_extra", UiValue::Bool(true));
    });
    assert_eq!(reused, fresh);
    assert!(fresh.iter().any(|l| l.contains("\"extra\"")), "{fresh:#?}");
}

#[test]
fn list_rows_follow_their_items() {
    let mut state = base_state();
    let (reused, fresh, _) = reexpand(&mut state, (None, None), |s| {
        s.set("rows", rows(&["x", "y", "z"]));
    });
    assert_eq!(reused, fresh);
    assert_eq!(fresh.iter().filter(|l| l.contains("\"row\"")).count(), 3);
}

#[test]
fn an_anchored_tooltip_follows_the_hover_anchor() {
    let mut state = base_state();
    let (reused, fresh, _) = reexpand(&mut state, (None, Some("anchor")), |_| {});
    assert_eq!(reused, fresh);
    assert!(
        fresh.iter().any(|l| l.contains("Some(\"help\")")),
        "{fresh:#?}"
    );

    let (reused, fresh, _) = reexpand(&mut state, (Some("anchor"), None), |_| {});
    assert_eq!(reused, fresh);
    assert!(!fresh.iter().any(|l| l.contains("Some(\"help\")")));
}

#[test]
fn removing_a_key_is_a_change() {
    let mut state = base_state();
    let (reused, fresh, _) = reexpand(&mut state, (None, None), |s| s.remove("status"));
    assert_eq!(reused, fresh);
}
