//! The per-kind data catalog from `assets/ui/bindings.json`: which `UiState`
//! keys the game populates for each document kind (bindable), what fields
//! each list item carries, and which widget ids the game reacts to. Feeds the
//! inspector's binding pickers, the Screen-data panel, and preview
//! sample-state seeding. Missing file = features hide gracefully.

use petramond_ui::{UiMap, UiState, UiValue};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Debug, Default)]
pub struct Catalog {
    kinds: BTreeMap<String, KindInfo>,
}

#[derive(Debug, Default)]
pub struct KindInfo {
    /// key -> (type, item fields, doc). BTreeMap keeps picker order stable.
    pub state: BTreeMap<String, StateKey>,
    /// widget id -> behavior description.
    pub handles: BTreeMap<String, String>,
    pub notes: Option<String>,
}

#[derive(Debug)]
pub struct StateKey {
    /// `f32` | `i32` | `bool` | `str` | `list` (anything else = open-ended).
    pub ty: String,
    /// For `list` keys: item field name -> type.
    pub item: BTreeMap<String, String>,
    pub doc: String,
}

/// Which `Bindings` field a picker is for (determines the key-type filter).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum BindField {
    Text,
    Value,
    Enabled,
    Visible,
    Items,
    Selected,
    /// Image-name override (`bind.image`): string keys only.
    Image,
}

/// Whether a catalog key of `ty` is offerable for `field`: `items` wants
/// lists, `selected` an i32, enabled/visible bools, `image` a str, text/value
/// any scalar.
pub fn field_matches(field: BindField, ty: &str) -> bool {
    match field {
        BindField::Items => ty == "list",
        BindField::Selected => ty == "i32",
        BindField::Enabled | BindField::Visible => ty == "bool",
        BindField::Image => ty == "str",
        BindField::Text | BindField::Value => matches!(ty, "f32" | "i32" | "bool" | "str"),
    }
}

impl Catalog {
    pub fn parse(json: &str) -> Result<Catalog, String> {
        #[derive(Deserialize)]
        struct Raw {
            format: u32,
            #[serde(default)]
            kinds: BTreeMap<String, RawKind>,
        }
        #[derive(Deserialize)]
        struct RawKind {
            #[serde(default)]
            state: BTreeMap<String, RawKey>,
            #[serde(default)]
            handles: BTreeMap<String, String>,
            #[serde(default)]
            notes: Option<String>,
        }
        #[derive(Deserialize)]
        struct RawKey {
            #[serde(rename = "type")]
            ty: String,
            #[serde(default)]
            item: BTreeMap<String, String>,
            #[serde(default)]
            doc: String,
        }
        let raw: Raw = serde_json::from_str(json).map_err(|e| format!("bindings.json: {e}"))?;
        if raw.format != 1 {
            return Err(format!("bindings.json: unsupported format {}", raw.format));
        }
        Ok(Catalog {
            kinds: raw
                .kinds
                .into_iter()
                .map(|(kind, k)| {
                    (
                        kind,
                        KindInfo {
                            state: k
                                .state
                                .into_iter()
                                .map(|(name, key)| {
                                    (name, StateKey { ty: key.ty, item: key.item, doc: key.doc })
                                })
                                .collect(),
                            handles: k.handles,
                            notes: k.notes,
                        },
                    )
                })
                .collect(),
        })
    }

    /// Load the shipped catalog from the repo (`assets/ui/bindings.json`),
    /// searching the same roots as the theme. `None` = not found/broken.
    pub fn load() -> Option<Catalog> {
        for path in candidates() {
            let Ok(json) = std::fs::read_to_string(&path) else {
                continue;
            };
            match Catalog::parse(&json) {
                Ok(c) => return Some(c),
                Err(e) => {
                    eprintln!("gui-builder: {e} (at {}); binding pickers disabled", path.display());
                    return None;
                }
            }
        }
        None
    }

    pub fn kind(&self, kind: &str) -> Option<&KindInfo> {
        self.kinds.get(kind)
    }
}

fn candidates() -> Vec<PathBuf> {
    let mut out = Vec::new();
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    if let Some(repo) = manifest_dir.parent() {
        out.push(repo.join("assets/ui/bindings.json"));
    }
    out.push(PathBuf::from("assets/ui/bindings.json"));
    out.push(PathBuf::from("../assets/ui/bindings.json"));
    out
}

impl KindInfo {
    /// The catalog keys (with docs) offerable for one bind field.
    pub fn keys_for(&self, field: BindField) -> Vec<(&str, &str)> {
        self.state
            .iter()
            .filter(|(_, k)| field_matches(field, &k.ty))
            .map(|(name, k)| (name.as_str(), k.doc.as_str()))
            .collect()
    }
}

// ---- sample seeding ---------------------------------------------------------------

const ORDINALS: [&str; 3] = ["First", "Second", "Third"];

fn seed_scalar(ty: &str, index: usize) -> Option<UiValue> {
    Some(match ty {
        "str" => {
            if index == usize::MAX {
                UiValue::Str("Sample text".into())
            } else {
                UiValue::Str(format!("{} item", ORDINALS[index % 3]))
            }
        }
        "bool" => UiValue::Bool(true),
        "i32" => UiValue::I32(0),
        "f32" => UiValue::F32(0.5),
        _ => return None,
    })
}

/// A plausible preview value for every state key of a kind: str -> "Sample
/// text", bool -> true, i32 -> 0, f32 -> 0.5, list -> 3 items with all item
/// fields filled ("First item"/"Second item"/"Third item" for strings).
pub fn seed_values(info: &KindInfo) -> Vec<(String, UiValue)> {
    let mut out = Vec::new();
    for (name, key) in &info.state {
        let value = if key.ty == "list" {
            let rows: Vec<UiMap> = (0..3)
                .map(|i| {
                    key.item
                        .iter()
                        .filter_map(|(field, ty)| Some((field.clone(), seed_scalar(ty, i)?)))
                        .collect()
                })
                .collect();
            UiValue::List(Arc::new(rows))
        } else {
            match seed_scalar(&key.ty, usize::MAX) {
                Some(v) => v,
                None => continue, // open-ended type: nothing sensible to seed
            }
        };
        out.push((name.clone(), value));
    }
    out
}

/// Non-destructively fill `state` with seeds for every catalog key the
/// author hasn't set — opened projects get preview data without dirtying
/// their file.
pub fn apply_seeds(state: &mut UiState, info: &KindInfo) {
    for (key, value) in seed_values(info) {
        if state.get(&key).is_none() {
            state.set(key, value);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{
        "format": 1,
        "kinds": {
            "petramond:world_select": {
                "state": {
                    "worlds": { "type": "list", "item": { "name": "str", "icon": "str" }, "doc": "one row per saved world" },
                    "filter_text": { "type": "str", "doc": "search filter" },
                    "world_sel": { "type": "i32", "doc": "selected row index" },
                    "has_selection": { "type": "bool", "doc": "a world is selected" },
                    "no_worlds": { "type": "bool", "doc": "save list is empty" }
                },
                "handles": { "play": "click: play the selected world" }
            },
            "petramond:furnace": {
                "state": { "cook01": { "type": "f32", "doc": "smelt progress" } },
                "handles": {},
                "notes": "container: slots route by role"
            }
        }
    }"#;

    #[test]
    fn catalog_parses_and_the_shipped_file_loads_when_present() {
        let c = Catalog::parse(SAMPLE).unwrap();
        let ws = c.kind("petramond:world_select").unwrap();
        assert_eq!(ws.state.len(), 5);
        assert_eq!(ws.state["worlds"].ty, "list");
        assert_eq!(ws.state["worlds"].item["name"], "str");
        assert_eq!(ws.handles["play"], "click: play the selected world");
        assert!(c.kind("somemod:wheel").is_none(), "mod kinds are open-ended");
        // The real repo file, when present, must parse too.
        if let Some(real) = Catalog::load() {
            assert!(real.kind("petramond:world_select").is_some());
        }
    }

    #[test]
    fn bind_field_filters_offer_only_matching_types() {
        let c = Catalog::parse(SAMPLE).unwrap();
        let ws = c.kind("petramond:world_select").unwrap();
        let names = |f| ws.keys_for(f).iter().map(|(n, _)| *n).collect::<Vec<_>>();
        assert_eq!(names(BindField::Items), vec!["worlds"]);
        assert_eq!(names(BindField::Selected), vec!["world_sel"]);
        assert_eq!(names(BindField::Enabled), vec!["has_selection", "no_worlds"]);
        assert_eq!(names(BindField::Visible), vec!["has_selection", "no_worlds"]);
        // bind.image takes str keys only.
        assert_eq!(names(BindField::Image), vec!["filter_text"]);
        assert!(field_matches(BindField::Image, "str"), "item icon fields qualify");
        assert!(!field_matches(BindField::Image, "i32"));
        // text/value take any scalar, never the list.
        assert_eq!(
            names(BindField::Text),
            vec!["filter_text", "has_selection", "no_worlds", "world_sel"]
        );
        assert_eq!(names(BindField::Value), names(BindField::Text));
    }

    #[test]
    fn seeding_fills_lists_with_three_complete_items_and_is_non_destructive() {
        let c = Catalog::parse(SAMPLE).unwrap();
        let ws = c.kind("petramond:world_select").unwrap();
        let mut state = UiState::new();
        state.set("world_sel", UiValue::I32(2)); // author's own value survives
        apply_seeds(&mut state, ws);

        assert_eq!(state.get_i32("world_sel"), Some(2));
        assert_eq!(state.get_bool("has_selection"), Some(true));
        let worlds = state.get_list("worlds").expect("list seeded");
        assert_eq!(worlds.len(), 3);
        assert_eq!(worlds[0]["name"], UiValue::Str("First item".into()));
        assert_eq!(worlds[1]["name"], UiValue::Str("Second item".into()));
        assert_eq!(worlds[2]["name"], UiValue::Str("Third item".into()));

        let furnace = c.kind("petramond:furnace").unwrap();
        let mut s2 = UiState::new();
        apply_seeds(&mut s2, furnace);
        assert_eq!(s2.get_f32("cook01"), Some(0.5));
    }
}
