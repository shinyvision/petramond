use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use mod_sdk::json::Value;
use mod_sdk::{PackKey, PackKeyKind};

const ROW_FIELDS: &[(&str, &str, PackKeyKind)] = &[
    ("blocks", "block", PackKeyKind::Block),
    ("items", "item", PackKeyKind::Item),
    ("mobs", "mob", PackKeyKind::Mob),
    ("sounds", "sound", PackKeyKind::Sound),
    ("effects", "effect", PackKeyKind::Effect),
    ("conditions", "condition", PackKeyKind::Condition),
    ("loot_tables", "loot", PackKeyKind::Loot),
    ("emitters", "emitter", PackKeyKind::Emitter),
    ("models", "key", PackKeyKind::Model),
    ("shapes", "key", PackKeyKind::Shape),
    ("recipes", "recipe", PackKeyKind::Recipe),
    ("recipes", "class", PackKeyKind::RecipeClass),
    (
        "underground_biomes",
        "underground_biome",
        PackKeyKind::UndergroundBiome,
    ),
];

const SKIPPED_DIRS: &[&str] = &[
    "models",
    "textures",
    "animations",
    "sounds",
    "music",
    "art",
    "palettes",
];

type Entry = (String, String);

pub struct PackIndex {
    entries: HashSet<Entry>,
}

impl PackIndex {
    pub fn shipped() -> Self {
        let mods_src = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("pack-check lives inside mods-src");
        let mut roots = vec![mods_src.join("../assets")];
        let mut packs: Vec<PathBuf> = fs::read_dir(mods_src)
            .expect("mods-src is readable")
            .filter_map(|e| Some(e.ok()?.path().join("pack")))
            .filter(|p| p.is_dir())
            .collect();
        packs.sort();
        roots.extend(packs);
        let mut index = Self {
            entries: HashSet::new(),
        };
        for root in roots {
            index.add_dir(&root);
        }
        index
    }

    pub fn from_documents(docs: &[(&str, &str)]) -> Self {
        let mut index = Self {
            entries: HashSet::new(),
        };
        for (file_name, text) in docs {
            let value = Value::parse(text).unwrap_or_else(|| panic!("{file_name} parses"));
            index.add_document(file_name, &value);
        }
        index
    }

    pub fn with_pack_dir(mut self, pack_dir: &Path) -> Self {
        self.add_dir(pack_dir);
        self
    }

    pub fn declares(&self, key: &PackKey) -> bool {
        self.entries
            .contains(&(label(key.kind), canonical(key.kind, key.key)))
    }

    pub fn assert_declared(&self, groups: &[&[PackKey]]) {
        let missing: Vec<String> = groups
            .iter()
            .flat_map(|g| g.iter())
            .filter(|k| !self.declares(k))
            .map(|k| format!("{:?} {:?}", k.kind, k.key))
            .collect();
        assert!(
            missing.is_empty(),
            "ids declared in code but missing from the shipped pack data:\n  {}",
            missing.join("\n  ")
        );
    }

    fn add_dir(&mut self, dir: &Path) {
        let Ok(read) = fs::read_dir(dir) else {
            return;
        };
        let mut paths: Vec<PathBuf> = read.filter_map(|e| Some(e.ok()?.path())).collect();
        paths.sort();
        for path in paths {
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default()
                .to_owned();
            if path.is_dir() {
                if !SKIPPED_DIRS.contains(&name.as_str()) {
                    self.add_dir(&path);
                }
            } else if name.ends_with(".json") {
                let text = fs::read_to_string(&path)
                    .unwrap_or_else(|e| panic!("{} is readable: {e}", path.display()));
                let value = Value::parse(&text)
                    .unwrap_or_else(|| panic!("{} is valid JSON", path.display()));
                self.add_document(&name, &value);
            }
        }
    }

    fn add_document(&mut self, file_name: &str, value: &Value) {
        if file_name.ends_with(".gui.json") {
            let kind = value
                .get("kind")
                .and_then(Value::as_str)
                .unwrap_or_default();
            self.insert(PackKeyKind::GuiKind, kind);
            self.walk_gui(kind, value);
            return;
        }
        if file_name == "shaders.json" {
            self.walk_shaders(value);
        }
        self.add_rows(value);
        self.walk_catalog(value);
    }

    fn insert(&mut self, kind: PackKeyKind, key: &str) {
        self.entries.insert((label(kind), canonical(kind, key)));
    }

    fn insert_widget(&mut self, doc_kind: &str, id: &str) {
        self.entries.insert((widget_label(doc_kind), id.to_owned()));
    }

    fn walk_gui(&mut self, doc_kind: &str, value: &Value) {
        match value {
            Value::Obj(fields) => {
                for (field, v) in fields {
                    match (field.as_str(), v) {
                        ("id", Value::Str(id)) => self.insert_widget(doc_kind, id),
                        ("bind", Value::Obj(binds)) => {
                            for (_, bound) in binds {
                                if let Some(state) = bound.as_str() {
                                    self.insert(PackKeyKind::GuiState, state);
                                }
                            }
                        }
                        _ => self.walk_gui(doc_kind, v),
                    }
                }
            }
            Value::Arr(items) => items.iter().for_each(|v| self.walk_gui(doc_kind, v)),
            _ => {}
        }
    }

    fn walk_shaders(&mut self, value: &Value) {
        for (_, pipeline) in value.as_object().unwrap_or_default() {
            for param in pipeline
                .get("params")
                .and_then(Value::as_array)
                .unwrap_or_default()
            {
                if let Some(p) = param.as_str() {
                    self.insert(PackKeyKind::ShaderParam, p);
                }
            }
        }
    }

    fn add_rows(&mut self, value: &Value) {
        for (array, rows) in value.as_object().unwrap_or_default() {
            for (name, field, kind) in ROW_FIELDS {
                if *name != array.as_str() {
                    continue;
                }
                for row in rows.as_array().unwrap_or_default() {
                    if let Some(id) = row.get(field).and_then(Value::as_str) {
                        self.insert(*kind, id);
                    }
                }
            }
        }
    }

    fn walk_catalog(&mut self, value: &Value) {
        match value {
            Value::Obj(fields) => {
                for (field, v) in fields {
                    self.catalog_field(field, v);
                }
            }
            Value::Arr(items) => items.iter().for_each(|v| self.walk_catalog(v)),
            _ => {}
        }
    }

    fn catalog_field(&mut self, field: &str, v: &Value) {
        match (field, v) {
            ("tags", Value::Arr(tags)) => {
                for tag in tags.iter().filter_map(Value::as_str) {
                    self.insert(PackKeyKind::Tag, tag);
                }
            }
            ("tags", Value::Obj(tags)) => {
                for (tag, _) in tags {
                    self.insert(PackKeyKind::MobTag, tag);
                }
            }
            ("data", Value::Obj(data)) => {
                for (key, _) in data {
                    self.insert(PackKeyKind::Data, key);
                }
            }
            ("behavior", Value::Str(hook)) => self.insert(PackKeyKind::Behavior, hook),
            ("node", Value::Str(node)) => self.insert(PackKeyKind::AiNode, node),
            (_, Value::Arr(_) | Value::Obj(_)) => self.walk_catalog(v),
            _ => {}
        }
    }
}

fn label(kind: PackKeyKind) -> String {
    match kind {
        PackKeyKind::Widget(doc_kind) => widget_label(doc_kind),
        other => format!("{other:?}"),
    }
}

fn canonical(kind: PackKeyKind, key: &str) -> String {
    match kind {
        PackKeyKind::Tag => key.strip_prefix("petramond:").unwrap_or(key).to_owned(),
        _ => key.to_owned(),
    }
}

fn widget_label(doc_kind: &str) -> String {
    format!("Widget({doc_kind})")
}

pub fn assert_declared(groups: &[&[PackKey]]) {
    PackIndex::shipped().assert_declared(groups);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(kind: PackKeyKind, key: &'static str) -> PackKey {
        PackKey { kind, key }
    }

    #[test]
    fn rows_tags_data_and_hooks_are_indexed_by_kind() {
        let index = PackIndex::from_documents(&[
            (
                "blocks.json",
                r#"{"blocks":[{"block":"a:stone","tags":["a:hard","raw_ore"],"behavior":"a:grow",
                    "data":{"a:heat":1},"drops":[{"item":"a:pebble"}]}]}"#,
            ),
            (
                "mobs.json",
                r#"{"mobs":[{"mob":"a:cow","tags":{"a:milk":1.0},"brain":[{"node":"a:graze"}]}]}"#,
            ),
            ("shaders.json", r#"{"environment":{"params":["a:wind"]}}"#),
        ]);
        for k in [
            key(PackKeyKind::Block, "a:stone"),
            key(PackKeyKind::Tag, "a:hard"),
            key(PackKeyKind::Tag, "petramond:raw_ore"),
            key(PackKeyKind::Tag, "raw_ore"),
            key(PackKeyKind::Behavior, "a:grow"),
            key(PackKeyKind::Data, "a:heat"),
            key(PackKeyKind::Mob, "a:cow"),
            key(PackKeyKind::MobTag, "a:milk"),
            key(PackKeyKind::AiNode, "a:graze"),
            key(PackKeyKind::ShaderParam, "a:wind"),
        ] {
            assert!(index.declares(&k), "{k:?} is declared");
        }
        assert!(
            !index.declares(&key(PackKeyKind::Item, "a:pebble")),
            "a drop REFERENCES an item; it does not declare one"
        );
        assert!(
            !index.declares(&key(PackKeyKind::Item, "a:stone")),
            "kinds do not bleed into each other"
        );
    }

    #[test]
    fn gui_widgets_are_scoped_to_their_document() {
        let index = PackIndex::from_documents(&[(
            "oven.gui.json",
            r#"{"kind":"k:oven","root":{"type":"frame","children":[
                {"type":"gauge","id":"arrow","bind":{"value":"k:cook01"}}]}}"#,
        )]);
        assert!(index.declares(&key(PackKeyKind::GuiKind, "k:oven")));
        assert!(index.declares(&key(PackKeyKind::GuiState, "k:cook01")));
        assert!(index.declares(&key(PackKeyKind::Widget("k:oven"), "arrow")));
        assert!(!index.declares(&key(PackKeyKind::Widget("k:mill"), "arrow")));
    }
}
