//! `.llgui` v2 project format: the petramond-ui [`Document`] verbatim plus
//! editor-only settings that never ship to the game.
//!
//! ```json
//! { "version": 2,
//!   "document": { ...petramond_ui::Document JSON... },
//!   "editor": { "sample_state": {...}, "zoom": 2.0, "preview_scale": 2,
//!               "screen": [1280, 720] } }
//! ```
//!
//! `sample_state` seeds the preview's `UiState` so bound content shows while
//! authoring. Each entry is a tagged JSON value (the codec this module owns):
//!
//! ```json
//! { "f32": 1.5 } | { "i32": 3 } | { "bool": true } | { "str": "hello" }
//! | { "list": [ { "<key>": <tagged value>, ... }, ... ] }
//! ```
//!
//! (list items are string-keyed maps of tagged values, mirroring `UiMap`).

use crate::contracts;
use petramond_ui::{
    Anchor, AnchorEdge, Document, LayoutProps, Node, NodeKind, UiMap, UiState, UiValue,
    FORMAT_VERSION,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::sync::Arc;

pub const PROJECT_VERSION: u32 = 2;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Project {
    pub version: u32,
    pub document: Document,
    #[serde(default)]
    pub editor: EditorSettings,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct EditorSettings {
    /// Tagged-JSON `UiState` seed for the preview (see module docs).
    pub sample_state: Map<String, Value>,
    /// Canvas zoom (screen px per physical preview px).
    pub zoom: f32,
    /// Integer gui scale the preview renders at (1-4).
    pub preview_scale: u32,
    /// Physical preview screen size.
    pub screen: (u32, u32),
    /// Draw the logical-pixel grid at high zoom.
    pub pixel_grid: bool,
}

impl Default for EditorSettings {
    fn default() -> Self {
        EditorSettings {
            sample_state: Map::new(),
            zoom: 1.0,
            preview_scale: 2,
            screen: (1280, 720),
            pixel_grid: false,
        }
    }
}

impl Project {
    /// A fresh project for `kind`: a centered panel scaffolded with every
    /// slot grid its engine contract requires (so it validates immediately).
    pub fn new(kind: &str) -> Project {
        let contract = contracts::contract_for(kind);
        let mut root = Node::leaf(NodeKind::Column);
        root.style = Some("panel.large".into());
        root.layout = LayoutProps {
            pad: [8, 8, 8, 8],
            gap: 6,
            anchor: Some(Anchor {
                h: AnchorEdge::Center,
                v: if kind == "petramond:hotbar" { AnchorEdge::End } else { AnchorEdge::Center },
            }),
            ..LayoutProps::default()
        };
        if kind != "petramond:hotbar" {
            let title = kind.split(':').last().unwrap_or(kind).replace('_', " ").to_uppercase();
            root.children
                .push(Node::leaf(NodeKind::Label { text: Some(title), wrap: false, scale: 1 }));
        }
        for (role, count) in &contract.roles {
            let (cols, rows) = contracts::default_grid(*count);
            root.children.push(Node::leaf(if *count == 1 {
                NodeKind::Slot { role: role.clone(), accepts: Vec::new(), take_only: false }
            } else {
                NodeKind::SlotGrid { role: role.clone(), cols, rows, accepts: Vec::new(), take_only: false }
            }));
        }
        Project {
            version: PROJECT_VERSION,
            document: Document {
                format: FORMAT_VERSION,
                kind: kind.to_owned(),
                class: contracts::class_for(kind),
                compact_below_w: None,
                root,
            },
            editor: EditorSettings::default(),
        }
    }

    pub fn from_json(s: &str) -> Result<Project, String> {
        let v: Value = serde_json::from_str(s).map_err(|e| e.to_string())?;
        if v.get("document").is_none() {
            if v.get("gui_type").is_some() {
                return Err("legacy v1 .llgui (use File > Import Legacy)".into());
            }
            return Err("not a gui-builder project (no 'document' field)".into());
        }
        let p: Project = serde_json::from_value(v).map_err(|e| e.to_string())?;
        if p.version != PROJECT_VERSION {
            return Err(format!("unsupported project version {}", p.version));
        }
        if p.document.format != FORMAT_VERSION {
            return Err(format!("unsupported document format {}", p.document.format));
        }
        Ok(p)
    }

    pub fn to_json_pretty(&self) -> String {
        serde_json::to_string_pretty(self).expect("projects always serialize")
    }

    /// The preview `UiState` decoded from `sample_state` (+ decode errors).
    pub fn sample_ui_state(&self) -> (UiState, Vec<String>) {
        let mut state = UiState::new();
        let mut errors = Vec::new();
        for (k, v) in &self.editor.sample_state {
            match json_to_value(v) {
                Ok(val) => state.set(k.clone(), val),
                Err(e) => errors.push(format!("{k}: {e}")),
            }
        }
        (state, errors)
    }
}

/// Whether raw `.llgui` JSON text is a legacy (layer-compositor v1) project.
pub fn is_legacy_json(s: &str) -> bool {
    serde_json::from_str::<Value>(s)
        .map(|v| v.get("document").is_none() && v.get("gui_type").is_some())
        .unwrap_or(false)
}

// ---- tagged UiValue codec -------------------------------------------------------

pub fn value_to_json(v: &UiValue) -> Value {
    match v {
        UiValue::F32(x) => json!({ "f32": x }),
        UiValue::I32(x) => json!({ "i32": x }),
        UiValue::Bool(x) => json!({ "bool": x }),
        UiValue::Str(s) => json!({ "str": s }),
        UiValue::List(items) => {
            let rows: Vec<Value> = items
                .iter()
                .map(|m| {
                    Value::Object(m.iter().map(|(k, v)| (k.clone(), value_to_json(v))).collect())
                })
                .collect();
            json!({ "list": rows })
        }
    }
}

pub fn json_to_value(v: &Value) -> Result<UiValue, String> {
    let obj = v.as_object().ok_or("expected a tagged object like {\"str\": ...}")?;
    if obj.len() != 1 {
        return Err("tagged value must have exactly one key".into());
    }
    let (tag, val) = obj.iter().next().unwrap();
    match tag.as_str() {
        "f32" => Ok(UiValue::F32(val.as_f64().ok_or("f32 wants a number")? as f32)),
        "i32" => Ok(UiValue::I32(val.as_i64().ok_or("i32 wants an integer")? as i32)),
        "bool" => Ok(UiValue::Bool(val.as_bool().ok_or("bool wants true/false")?)),
        "str" => Ok(UiValue::Str(val.as_str().ok_or("str wants a string")?.to_owned())),
        "list" => {
            let rows = val.as_array().ok_or("list wants an array of item maps")?;
            let mut out: Vec<UiMap> = Vec::with_capacity(rows.len());
            for row in rows {
                let m = row.as_object().ok_or("list item must be an object")?;
                let mut map = UiMap::new();
                for (k, v) in m {
                    map.insert(k.clone(), json_to_value(v)?);
                }
                out.push(map);
            }
            Ok(UiValue::List(Arc::new(out)))
        }
        other => Err(format!("unknown tag '{other}'")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_load_save_is_idempotent() {
        let mut p = Project::new("petramond:furnace");
        p.editor.sample_state.insert("cook01".into(), json!({ "f32": 0.5 }));
        p.editor.zoom = 3.0;
        let s1 = p.to_json_pretty();
        let p2 = Project::from_json(&s1).unwrap();
        assert_eq!(p, p2);
        assert_eq!(s1, p2.to_json_pretty(), "second save is byte-identical");
    }

    #[test]
    fn new_projects_satisfy_their_contract() {
        for kind in crate::contracts::ENGINE_KINDS {
            let p = Project::new(kind);
            let contract = crate::contracts::contract_for(kind);
            let issues = p.document.validate(None, Some(&contract));
            assert!(issues.is_empty(), "{kind}: {issues:?}");
        }
    }

    #[test]
    fn sample_state_codec_round_trips() {
        let mut row = UiMap::new();
        row.insert("name".into(), UiValue::Str("Zombies".into()));
        row.insert("enabled".into(), UiValue::Bool(true));
        for v in [
            UiValue::F32(0.25),
            UiValue::I32(-1),
            UiValue::Bool(false),
            UiValue::Str("hello".into()),
            UiValue::List(Arc::new(vec![row])),
        ] {
            let j = value_to_json(&v);
            assert_eq!(json_to_value(&j).unwrap(), v, "{j}");
        }
        assert!(json_to_value(&json!({ "nope": 1 })).is_err());
        assert!(json_to_value(&json!(3)).is_err());
    }

    #[test]
    fn sample_ui_state_reports_bad_entries_without_dying() {
        let mut p = Project::new("petramond:pause");
        p.editor.sample_state.insert("ok".into(), json!({ "i32": 4 }));
        p.editor.sample_state.insert("bad".into(), json!("untagged"));
        let (state, errors) = p.sample_ui_state();
        assert_eq!(state.get_i32("ok"), Some(4));
        assert_eq!(errors.len(), 1);
    }

    #[test]
    fn legacy_files_are_detected_not_parsed() {
        let legacy = r#"{"version":2,"gui_type":"pause","scale":1,"canvas":{"w":10,"h":10},"nodes":[],"slots":[]}"#;
        assert!(is_legacy_json(legacy));
        assert!(Project::from_json(legacy).unwrap_err().contains("legacy"));
    }

    #[test]
    fn image_fit_and_bind_image_round_trip_through_save_load() {
        use petramond_ui::doc::ImageFit;
        let mut p = Project::new("petramond:pause");
        for fit in [
            ImageFit::Stretch,
            ImageFit::Cover,
            ImageFit::Tile,
            ImageFit::Slice([2, 3, 4, 5]),
        ] {
            p.document
                .root
                .children
                .push(Node::leaf(NodeKind::Image { image: "art.png".into(), fit, interactive: false }));
        }
        p.document.root.children.last_mut().unwrap().bind.image = Some("icon".into());

        let saved = p.to_json_pretty();
        let loaded = Project::from_json(&saved).unwrap();
        assert_eq!(p, loaded, "project save/load keeps fit + bind.image");
        // …and survives export to the runtime format too.
        let doc = Document::from_json(&loaded.document.to_json_pretty()).unwrap();
        assert_eq!(doc, loaded.document);
        match &doc.root.children.last().unwrap().kind {
            NodeKind::Image { fit, .. } => assert_eq!(*fit, ImageFit::Slice([2, 3, 4, 5])),
            other => panic!("{other:?}"),
        }
    }
}
